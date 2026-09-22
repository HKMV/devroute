use crate::core::route::{RouteEngine, RouteRule};
use crate::core::stats::Stats;
use anyhow::{Context, Result, anyhow};
use httparse::Status;
use std::sync::Arc;
use tokio::io::{self, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{debug, error, info};

pub(crate) async fn forward_handle(
    client: TcpStream,
    server: TcpStream,
    rule: &RouteRule,
    stats: &Arc<Stats>,
) -> Result<()> {
    // spawn 的任务需要 'static，先把引用转成 owned Arc
    let stats = stats.clone();
    // 创建缓冲区用于协议检测
    let mut peek_buf = [0u8; 256];
    let peek_size = client.peek(&mut peek_buf).await?;
    let is_http = is_http(peek_buf.as_ref(), peek_size);

    // 拆分客户端和服务器流为读写两半
    let (mut client_reader, mut client_writer) = tokio::io::split(client);
    let (mut server_reader, mut server_writer) = tokio::io::split(server);

    let forward = match TcpStream::connect(&rule.forward.host).await {
        Ok(ts) => {
            ts.set_nodelay(true).ok();
            Some(ts)
        }
        Err(e) => {
            if rule.forward.connect_fail_use_original_host {
                error!("Connect to forward host failed, use original host: {}", e);
                let client_to_target = io::copy(&mut client_reader, &mut server_writer);
                let target_to_client = io::copy(&mut server_reader, &mut client_writer);
                let (up, down) = tokio::try_join!(client_to_target, target_to_client)?;
                stats.up(up as usize);
                stats.down(down as usize);
                return Ok(());
            }

            if let Some(path) = parse_path(peek_buf.as_ref())
                && path.starts_with(rule.match_.prefix.as_str())
            {
                //转发服务连接不上，终止需要转发的请求
                error!("Connect to forward host failed, stop access: {}", e);

                if let Err(e) = client_writer.write_all(service_unavailable()).await
                {
                    error!("Forward write error: {}", e);
                }

                server_writer.shutdown().await.unwrap_or(());
                client_writer.shutdown().await.unwrap_or(());
                return Err(e.into());
            }

            // 不进行修改转发的路径继续访问
            None
        }
    };
    let (mut forward_reader, mut forward_writer) = match forward {
        None => (None, None),
        Some(f) => {
            let (reader, writer) = io::split(f);
            (Some(reader), Some(writer))
        }
    };

    // ponytail: tokio::Mutex 持锁跨 await 只为序列化两个写方向的 chunk（持锁期间只有写操作，不会死锁）；吞吐成瓶颈再改单写者 + mpsc
    let client_writer = Arc::new(Mutex::new(client_writer));
    let rule = rule.clone();

    let client_writer_c = client_writer.clone();
    let stats_c = stats.clone();
    let client_to_server = tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            let n = match client_reader.read(&mut buf).await {
                Ok(0) => break, // EOF
                Ok(n) => {
                    stats_c.up(n);
                    n
                }
                Err(e) => {
                    error!("Client read error: {}", e);
                    break;
                }
            };

            // 如果是HTTP流量，可以进行修改
            if !is_http {
                if let Err(e) = server_writer.write_all(&buf[..n]).await {
                    error!("Server write error: {}", e);
                    break;
                }
                continue;
            }
            match modify_http_data(&buf[..n], &rule) {
                None => {
                    if let Err(e) = server_writer.write_all(&buf[..n]).await {
                        error!("Server write error: {}", e);
                        break;
                    }
                }
                Some(d) => {
                    if let Some(fw) = &mut forward_writer {
                        debug!("forward_writer: {}", &rule.forward.host);
                        if let Err(e) = fw.write_all(&d).await {
                            error!("Forward write error: {}", e);
                            break;
                        }
                    } else if let Err(e) = client_writer_c
                        .lock()
                        .await
                        .write_all(service_unavailable())
                        .await
                    {
                        error!("Forward write error: {}", e);
                        break;
                    }
                }
            }
        }
        server_writer.shutdown().await.unwrap_or(());
        if let Some(fw) = &mut forward_writer {
            fw.shutdown().await.unwrap_or(());
        }
    });

    let server_to_client = tokio::spawn(async move {
        let client_writer0 = client_writer.clone();
        let stats0 = stats.clone();
        let handle0 = tokio::spawn(async move {
            let mut buf = [0u8; 8192];
            loop {
                let n = match server_reader.read(&mut buf).await {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        stats0.down(n);
                        n
                    }
                    Err(e) => {
                        error!("Server read error: {}", e);
                        break;
                    }
                };

                if let Err(e) = client_writer0.lock().await.write_all(&buf[..n]).await {
                    error!("Client write error: {}", e);
                    break;
                }
            }
        });

        let client_writer1 = client_writer.clone();
        let stats1 = stats.clone();
        let handle1 = tokio::spawn(async move {
            if let Some(fr) = &mut forward_reader {
                let mut buf = [0u8; 8192];
                loop {
                    let n = match fr.read(&mut buf).await {
                        Ok(0) => break, // EOF
                        Ok(n) => {
                            stats1.down(n);
                            n
                        }
                        Err(e) => {
                            error!("Forward read error: {}", e);
                            break;
                        }
                    };
                    if let Err(e) = client_writer1.lock().await.write_all(&buf[..n]).await {
                        error!("Client write error: {}", e);
                        break;
                    }
                }
            }
        });

        let _ = tokio::try_join!(handle0, handle1);
        client_writer.lock().await.shutdown().await.unwrap_or(());
    });

    // 等待两个方向的任务完成
    let _ = tokio::try_join!(client_to_server, server_to_client);
    debug!("Request handle finished");
    Ok(())
}

// ponytail: 假定请求头在首个 8KB 读内到齐；分片到达的头部会原样透传不改写，需要时加缓冲重组
fn modify_http_data(data: &[u8], rule: &RouteRule) -> Option<Vec<u8>> {
    if data.starts_with(b"HTTP/") {
        // 响应数据，不改写
        return None;
    }

    let mut headers = [httparse::EMPTY_HEADER; 16];
    let mut req = httparse::Request::new(&mut headers);
    let path = match req.parse(data) {
        Ok(Status::Complete(_)) => req.path?,
        _ => return None, // 非完整HTTP请求或不支持的格式
    };

    let prefix = &rule.match_.prefix;
    if !rule.forward.rewrite || prefix.is_empty() || prefix == "/" || !path.starts_with(prefix) {
        return None;
    }
    debug!("Rewrite URL path: {} -> {}", path, path.replacen(prefix, &rule.forward.prefix, 1));
    let new_path = path.replacen(prefix, &rule.forward.prefix, 1);

    // 只改写请求行(ASCII)，正文原样拼接，避免二进制 body 触发 UTF-8 panic
    let line_end = data.windows(2).position(|w| w == *b"\r\n")?;
    let line = std::str::from_utf8(&data[..line_end]).ok()?;
    let mut out = Vec::with_capacity(data.len() + new_path.len());
    out.extend_from_slice(line.replacen(path, &new_path, 1).as_bytes());
    out.extend_from_slice(&data[line_end..]);
    Some(out)
}

fn is_http(data: &[u8], size: usize) -> bool {
    size >= 4
        && (data.starts_with(b"GET ")
            || data.starts_with(b"POST")
            || data.starts_with(b"PUT ")
            || data.starts_with(b"PATCH ")
            || data.starts_with(b"DELETE ")
            || data.starts_with(b"HEAD ")
            || data.starts_with(b"OPTIONS ")
            || data.starts_with(b"HTTP/"))
}

fn parse_path(data: &[u8]) -> Option<&str> {
    // 找到请求行的结束位置\r\n
    let end = data.windows(2).position(|w| w == *b"\r\n")?;
    let request_line = &data[..end];

    // 按空格分割并过滤空字段
    let mut parts = request_line
        .split(|&b| b == b' ')
        .filter(|part| !part.is_empty());

    // 跳过方法（第一个字段），取路径（第二个字段）
    parts.next()?; // 方法
    let path_bytes = parts.next()?; // 路径
    parts.next()?; // 协议版本（可选检查，确保存在）

    std::str::from_utf8(path_bytes).ok()
}

/// 双向隧道转发 + 字节计数
async fn tunnel(client: TcpStream, server: TcpStream, stats: &Stats) -> Result<()> {
    let (mut client_reader, mut client_writer) = tokio::io::split(client);
    let (mut server_reader, mut server_writer) = tokio::io::split(server);
    // ponytail: 字节数在 copy 结束后一次性累计，非实时
    let (up, down) = tokio::try_join!(
        io::copy(&mut client_reader, &mut server_writer),
        io::copy(&mut server_reader, &mut client_writer)
    )?;
    stats.up(up as usize);
    stats.down(down as usize);
    Ok(())
}

/// HTTP 代理请求处理（CONNECT / absolute-form）
///
/// 浏览器把系统代理设为 HTTP 代理后：HTTPS 走 CONNECT 隧道，HTTP 走绝对路径请求
pub(crate) async fn handle_http_proxy(
    mut client: TcpStream,
    route_engine: Arc<RouteEngine>,
    stats: Arc<Stats>,
) -> Result<()> {
    // 读请求头到 \r\n\r\n
    let mut buf = Vec::with_capacity(4096);
    let head_end = loop {
        let mut chunk = [0u8; 4096];
        let n = client.read(&mut chunk).await?;
        if n == 0 {
            return Err(anyhow!("connection closed before headers"));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == *b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 64 * 1024 {
            return Err(anyhow!("http proxy header too large"));
        }
    };

    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut req = httparse::Request::new(&mut headers);
    if req.parse(&buf[..head_end])?.is_partial() {
        return Err(anyhow!("incomplete http proxy request"));
    }
    let method = req.method.unwrap_or("");
    let raw_path = req.path.unwrap_or("");
    let header = |name: &str| {
        req.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .and_then(|h| std::str::from_utf8(h.value).ok())
    };
    client.set_nodelay(true).ok();

    // CONNECT host:port → 纯隧道（流量加密，无法改写）
    if method.eq_ignore_ascii_case("CONNECT") {
        let server = TcpStream::connect(raw_path)
            .await
            .with_context(|| format!("connect {raw_path}"))?;
        server.set_nodelay(true).ok();
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        debug!("HTTP proxy CONNECT {raw_path}");
        return tunnel(client, server, &stats).await;
    }

    // absolute-form: GET http://host[:port]/path；少数客户端发 origin-form，取 Host 头
    let (host, port, path) = if let Some(rest) = raw_path.strip_prefix("http://") {
        let (authority, p) = match rest.split_once('/') {
            Some((a, p)) => (a, format!("/{p}")),
            None => (rest, "/".to_string()),
        };
        match authority.split_once(':') {
            Some((h, po)) => (h.to_string(), po.to_string(), p),
            None => (authority.to_string(), "80".to_string(), p),
        }
    } else {
        let host = header("host").unwrap_or("");
        match host.split_once(':') {
            Some((h, po)) => (h.to_string(), po.to_string(), raw_path.to_string()),
            None => (host.to_string(), "80".to_string(), raw_path.to_string()),
        }
    };
    if host.is_empty() {
        return Err(anyhow!("http proxy request missing target host"));
    }
    let target = format!("{host}:{port}");

    // 应用路由规则（按 host 匹配，路径前缀重写）
    let rule = route_engine.resolve_target_by_host(&target).await;
    let (connect_addr, new_path) = plan_request(&target, &path, rule.as_ref());
    if connect_addr != target {
        info!("转发命中: {method} http://{target}{path} → {connect_addr}{new_path}");
    } else {
        debug!("HTTP proxy {method} {target}{path} 直连");
    }
    let path = new_path;

    let mut server = TcpStream::connect(&connect_addr)
        .await
        .with_context(|| format!("connect {connect_addr}"))?;
    server.set_nodelay(true).ok();

    // 重建请求头：origin-form + Connection: close（一跳一答，简单可靠）
    let mut out = format!("{method} {path} HTTP/1.1\r\n");
    for h in req.headers.iter() {
        if h.name.eq_ignore_ascii_case("proxy-connection")
            || h.name.eq_ignore_ascii_case("connection")
        {
            continue;
        }
        out.push_str(&format!("{}: {}\r\n", h.name, String::from_utf8_lossy(h.value)));
    }
    out.push_str("Connection: close\r\n\r\n");
    server.write_all(out.as_bytes()).await?;
    drop(rule);
    // 头部之后可能已有 body 字节（如 POST），原样转发
    server.write_all(&buf[head_end..]).await?;
    stats.up(out.len() + buf.len() - head_end);

    tunnel(client, server, &stats).await
}

/// 路由决策：返回 (实际连接地址, 重写后的路径)。
/// host 和路径前缀都命中才转发；只命中 host 但路径不匹配 → 直连原始地址
fn plan_request(target: &str, path: &str, rule: Option<&RouteRule>) -> (String, String) {
    let Some(rule) = rule else {
        return (target.to_string(), path.to_string());
    };
    if !path.starts_with(&rule.match_.prefix) {
        return (target.to_string(), path.to_string());
    }
    let new_path = if rule.forward.rewrite
        && !rule.match_.prefix.is_empty()
        && rule.match_.prefix != "/"
    {
        path.replacen(&rule.match_.prefix, &rule.forward.prefix, 1)
    } else {
        path.to_string()
    };
    (rule.forward.host.clone(), new_path)
}

pub(crate) async fn parse_http_header(stream: &TcpStream) -> Option<(String, String)> {
    let mut buf = [0u8; 4096];
    let n = stream.peek(&mut buf).await.ok()?;
    if !is_http(buf.as_ref(), n) {
        //非HTTP请求
        return None;
    }

    let mut headers = [httparse::EMPTY_HEADER; 16];
    let mut req = httparse::Request::new(&mut headers);

    let _status = req.parse(&buf[..n]).ok()?;
    let path = req.path?.to_string();
    let host = req
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("host"))
        .and_then(|h| std::str::from_utf8(h.value).ok())?
        .to_string();
    Some((host, path))
}

fn service_unavailable() -> &'static [u8] {
    b"HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: 19\r\nConnection: close\r\n\r\nService Unavailable"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_without_rule() {
        let (addr, path) = plan_request("example.com:80", "/api/x", None);
        assert_eq!(addr, "example.com:80");
        assert_eq!(path, "/api/x");
    }

    #[test]
    fn plan_with_rule_rewrites() {
        let rule = rule();
        let (addr, path) = plan_request("example.com:80", "/api/users", Some(&rule));
        assert_eq!(addr, "127.0.0.1:8686");
        assert_eq!(path, "/users");
    }

    #[test]
    fn plan_with_rule_unmatched_prefix() {
        // host 命中但路径前缀不匹配：直连原始地址
        let rule = rule();
        let (addr, path) = plan_request("example.com:80", "/other", Some(&rule));
        assert_eq!(addr, "example.com:80");
        assert_eq!(path, "/other");
    }

    fn rule() -> RouteRule {
        RouteRule::new("example.com", "/api", "127.0.0.1:8686", "")
    }

    #[test]
    fn rewrites_path() {
        let data = b"GET /api/users?x=1 HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let out = modify_http_data(data, &rule()).unwrap();
        assert!(out.starts_with(b"GET /users?x=1 "));
    }

    #[test]
    fn binary_body_no_panic() {
        let mut data = b"POST /api HTTP/1.1\r\nHost: example.com\r\nContent-Length: 3\r\n\r\n".to_vec();
        data.extend_from_slice(&[0xFF, 0xFE, 0x00]);
        let out = modify_http_data(&data, &rule()).unwrap();
        assert!(out.ends_with(&[0xFF, 0xFE, 0x00]));
    }

    #[test]
    fn unmatched_prefix_passthrough() {
        let data = b"GET /other HTTP/1.1\r\nHost: example.com\r\n\r\n";
        assert!(modify_http_data(data, &rule()).is_none());
    }
}
