//! 镜像站点：本地开一个端口，页面取自上游站点，接口按站点自己的规则转发到本地服务。
//!
//! 浏览器访问 `http://127.0.0.1:8088`，页面和接口同源 —— 没有 CORS、Cookie 天然可用、
//! 也不用改系统代理。代价是页面里凡是写死上游 origin 的地方都要改写到本地 origin：
//! 响应体里的绝对地址、`Set-Cookie` 的 Domain、`Location` 重定向、CSP 和 SRI 校验。
//!
//! 规则内嵌在站点里（`[[sites.rules]]`），每个站点各自一份 RouteEngine，只按路径前缀匹配 ——
//! 站点已经绑定了上游，不需要再靠 host 区分，也就不会和代理模式的 `[[proxy_rules]]` 互相干扰。

use crate::core::config::Site;
use crate::core::route::RouteEngine;
use crate::core::stats::Stats;
use anyhow::{Context, Result, anyhow};
use httparse::Status;
use rust_i18n::t;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tracing::{debug, info, warn};

/// 请求/响应头上限
const MAX_HEAD: usize = 64 * 1024;
/// 单次改写缓冲上限：超过就原样透传，避免大文件/视频把内存打爆
const MAX_REWRITE: usize = 8 * 1024 * 1024;
/// 连接上游（含 TLS 握手）超时
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 上游可能是明文也可能是 TLS，统一按 trait object 用
trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
type BoxStream = Box<dyn Stream>;

/// 一个镜像站点的固定参数（启动时算好，全程只读）
pub(crate) struct SiteCtx {
    /// 站点备注名
    pub(crate) name: String,
    /// 本地入口 host:port
    pub(crate) local_host: String,
    /// 本地 origin，如 http://127.0.0.1:8088
    pub(crate) local_origin: String,
    /// 上游站点 host:port（规则的虚拟 host）
    pub(crate) upstream_host: String,
    /// 上游 origin，如 https://192.168.120.177:81
    pub(crate) upstream_origin: String,
    /// Some = 上游走 https
    tls: Option<Arc<rustls::ClientConfig>>,
    /// 跳过证书校验（内网自签）
    pub(crate) insecure: bool,
}

impl SiteCtx {
    pub(crate) fn new(site: &Site) -> Result<Self> {
        let (upstream_origin, upstream_host, https) = parse_upstream(&site.upstream)?;
        // 监听 0.0.0.0 时浏览器要用回环地址访问
        let local_host = match site.listen_addr.rsplit_once(':') {
            Some((host, port)) if host.is_empty() || host == "0.0.0.0" || host == "::" => {
                format!("127.0.0.1:{port}")
            }
            _ => site.listen_addr.clone(),
        };
        let tls = if https {
            Some(tls_config(site.insecure)?)
        } else {
            None
        };
        Ok(Self {
            name: site.name.clone(),
            local_origin: format!("http://{local_host}"),
            local_host,
            upstream_host,
            upstream_origin,
            tls,
            insecure: site.insecure,
        })
    }

    /// 连接上游；https 站点在这里做 TLS 握手。
    /// 整体加超时：上游握手卡住时不能让浏览器标签页一直转圈
    async fn connect(&self) -> Result<BoxStream> {
        let connect = async {
            let tcp = TcpStream::connect(&self.upstream_host)
                .await
                .with_context(|| format!("连接上游 {} 失败", self.upstream_host))?;
            tcp.set_nodelay(true).ok();
            let Some(cfg) = self.tls.clone() else {
                return Ok(Box::new(tcp) as BoxStream);
            };
            let host = split_host(&self.upstream_host);
            let server_name = ServerName::try_from(host.to_string())
                .map_err(|_| anyhow!("上游地址无法作为 TLS 服务器名: {host}"))?;
            let stream = TlsConnector::from(cfg)
                .connect(server_name, tcp)
                .await
                .with_context(|| format!("TLS 握手失败: {}", self.upstream_host))?;
            Ok(Box::new(stream) as BoxStream)
        };
        tokio::time::timeout(CONNECT_TIMEOUT, connect)
            .await
            .map_err(|_| anyhow!("连接上游 {} 超时", self.upstream_host))?
    }
}

/// 安装 rustls 的默认加密实现（只编译了 ring，必须显式装一次）
pub(crate) fn install_crypto_provider() {
    // 重复安装会返回 Err，忽略即可
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// 解析上游地址，返回 (origin, host:port, 是否 https)
fn parse_upstream(url: &str) -> Result<(String, String, bool)> {
    let url = url.trim().trim_end_matches('/');
    let (rest, https) = if let Some(rest) = url.strip_prefix("http://") {
        (rest, false)
    } else if let Some(rest) = url.strip_prefix("https://") {
        (rest, true)
    } else {
        return Err(anyhow!("站点 upstream 需要以 http:// 或 https:// 开头，当前为 {url}"));
    };
    if rest.is_empty() || rest.contains('/') {
        return Err(anyhow!("站点 upstream 只接受 host:port 形式，当前为 {url}"));
    }
    let host = if rest.contains(':') {
        rest.to_string()
    } else if https {
        format!("{rest}:443")
    } else {
        format!("{rest}:80")
    };
    let scheme = if https { "https" } else { "http" };
    Ok((format!("{scheme}://{host}"), host, https))
}

fn split_host(host_port: &str) -> &str {
    host_port.rsplit_once(':').map(|(h, _)| h).unwrap_or(host_port)
}

fn tls_config(insecure: bool) -> Result<Arc<rustls::ClientConfig>> {
    let cfg = if insecure {
        // 跳过校验要在 WantsVerifier 阶段换掉验证器，此时还没挂根证书
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify::new()?))
            .with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth()
    };
    Ok(Arc::new(cfg))
}

/// 跳过证书校验：只在站点显式打开 insecure 时使用（内网自签证书）
#[derive(Debug)]
struct NoVerify(Arc<rustls::crypto::CryptoProvider>);

impl NoVerify {
    fn new() -> Result<Self> {
        let provider = rustls::crypto::CryptoProvider::get_default()
            .cloned()
            .ok_or_else(|| anyhow!("rustls 加密实现未安装"))?;
        Ok(Self(provider))
    }
}

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// 处理一个镜像站点的连接
pub(crate) async fn handle(
    mut client: TcpStream,
    engine: Arc<RouteEngine>,
    stats: Arc<Stats>,
    ctx: Arc<SiteCtx>,
) -> Result<()> {
    let _guard = stats.conn_open();
    client.set_nodelay(true).ok();

    let (head, rest) = read_head(&mut client).await?;
    let req = match Request::parse(&head) {
        Ok(r) => r,
        Err(e) => {
            client
                .write_all(&simple_response("400 Bad Request", "bad request"))
                .await
                .ok();
            return Err(e);
        }
    };

    // 路由：命中站点自己的规则 → 本地服务（路径按规则重写），否则 → 上游站点
    let (target, path, mirrored) = match engine.resolve_by_path(&req.path).await {
        Some(rule) => {
            let path = rewrite_prefix(&rule, &req.path);
            debug!("[{}] 转发命中: {} {} → {}{}", ctx.name, req.method, req.path, rule.forward.host, path);
            (rule.forward.host.clone(), path, false)
        }
        None => (ctx.upstream_host.clone(), req.path.clone(), true),
    };

    // 命中规则的请求发往本地服务（明文 TCP），其余走上游（可能带 TLS）
    let mut upstream: BoxStream = if mirrored {
        match ctx.connect().await {
            Ok(s) => s,
            Err(e) => {
                warn!("[{}] {e}", ctx.name);
                client
                    .write_all(&simple_response("502 Bad Gateway", &e.to_string()))
                    .await
                    .ok();
                return Err(e);
            }
        }
    } else {
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&target)).await {
            Ok(Ok(s)) => {
                s.set_nodelay(true).ok();
                Box::new(s)
            }
            Ok(Err(e)) => {
                let msg = format!("转发目标 {target} 连接失败: {e}");
                warn!("[{}] {msg}", ctx.name);
                client
                    .write_all(&simple_response("503 Service Unavailable", &msg))
                    .await
                    .ok();
                return Err(e).with_context(|| msg);
            }
            Err(_) => {
                let msg = format!("转发目标 {target} 连接超时");
                warn!("[{}] {msg}", ctx.name);
                client
                    .write_all(&simple_response("503 Service Unavailable", &msg))
                    .await
                    .ok();
                return Err(anyhow!(msg));
            }
        }
    };

    let ws = req.is_websocket();
    // chunked 请求体（fetch + ReadableStream 上传）无法按 Content-Length 精确转发，
    // 也没法边转发边解析响应 → 整条连接纯隧道透传，牺牲这次响应的改写换取请求不被打断
    let tunnel_only = ws || req.chunked_body();
    let up_head = build_upstream_head(&req, &path, mirrored, tunnel_only, &ctx);
    upstream.write_all(up_head.as_bytes()).await?;
    let mut up_bytes = up_head.len();
    if !rest.is_empty() {
        upstream.write_all(&rest).await?;
        up_bytes += rest.len();
    }

    // WebSocket / chunked 上传：握手头已透传，之后纯隧道（内容不能改写）
    if tunnel_only {
        if ws {
            info!("[{}] WebSocket 隧道: {}", ctx.name, req.path);
        } else {
            debug!("[{}] chunked 请求体，按隧道透传: {} {}", ctx.name, req.method, req.path);
        }
        let (mut cr, mut cw) = tokio::io::split(client);
        let (mut ur, mut uw) = tokio::io::split(upstream);
        let (up, down) = tokio::try_join!(
            copy_lenient(&mut cr, &mut uw),
            copy_lenient(&mut ur, &mut cw)
        )?;
        stats.up(up_bytes + up as usize);
        stats.down(down as usize);
        return Ok(());
    }

    // 请求体按 Content-Length 精确转发：客户端连接不会关，不能直接 io::copy
    let want = req.content_length().saturating_sub(rest.len());
    if want > 0 {
        copy_exact(&mut client, &mut upstream, want).await?;
        up_bytes += want;
    }
    stats.up(up_bytes);

    let (rhead, rrest) = read_head(&mut upstream).await?;
    let resp = Response::parse(&rhead)?;
    let headers = transform_headers(&resp.headers, &ctx);

    if should_rewrite_body(&req, &resp, mirrored) {
        // 给上游发了 Connection: close，读到 EOF 就是完整响应体
        let mut body = rrest;
        read_to_end_lenient(&mut upstream, &mut body).await?;
        let body = if resp.chunked() {
            crate::core::http::dechunk(&body)
        } else {
            body
        };
        let body = if body.len() <= MAX_REWRITE {
            // 只给顶层文档挂署名条：iframe / embed 里那份会挤在小框里挡内容
            rewrite_body(body, &resp.content_type(), &ctx, is_top_level(&req))
        } else {
            body
        };
        let head = rebuild_head(&resp.status_line, &headers, Some(body.len()));
        client.write_all(head.as_bytes()).await?;
        client.write_all(&body).await?;
        stats.down(head.len() + body.len());
        debug!("[{}] 改写响应: {} {} ({} 字节)", ctx.name, req.method, req.path, body.len());
    } else {
        // SSE / Range / 大文件 / 二进制：流式透传，绝不缓冲
        let head = rebuild_head(&resp.status_line, &headers, None);
        client.write_all(head.as_bytes()).await?;
        let mut down = head.len() + rrest.len();
        client.write_all(&rrest).await?;
        let n = copy_lenient(&mut upstream, &mut client).await?;
        down += n as usize;
        stats.down(down);
    }

    client.shutdown().await.ok();
    Ok(())
}

/// 命中规则时的路径前缀替换（与代理模式语义一致）
fn rewrite_prefix(rule: &crate::core::route::RouteRule, path: &str) -> String {
    if rule.forward.rewrite && !rule.match_.prefix.is_empty() && rule.match_.prefix != "/" {
        path.replacen(&rule.match_.prefix, &rule.forward.prefix, 1)
    } else {
        path.to_string()
    }
}

/// 重建发给上游的请求头。`tunnel` = 这条连接之后要纯隧道透传（WebSocket / chunked 上传），
/// 此时逐跳头必须原样保留，否则上游会按错误的 framing 解析请求体
fn build_upstream_head(
    req: &Request,
    path: &str,
    mirrored: bool,
    tunnel: bool,
    ctx: &SiteCtx,
) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str(&format!("{} {} HTTP/1.1\r\n", req.method, path));
    for (name, value) in &req.headers {
        let l = name.to_ascii_lowercase();
        match l.as_str() {
            // Host 必须还原成上游地址，否则 nginx 之类按 vhost 直接 404
            "host" | "proxy-connection" | "keep-alive" => continue,
            "transfer-encoding" if !tunnel => continue,
            // 剥掉压缩协商：拿到明文才能改写响应体（局域网带宽无所谓）
            "accept-encoding" if !tunnel => continue,
            // 隧道模式保留 Connection/Upgrade 原样，普通请求统一由下面重建成 close
            "connection" if !tunnel => continue,
            "origin" if mirrored => {
                out.push_str(&format!("Origin: {}\r\n", ctx.upstream_origin));
                continue;
            }
            "referer" if mirrored => {
                let v = value.replacen(&ctx.local_origin, &ctx.upstream_origin, 1);
                out.push_str(&format!("Referer: {v}\r\n"));
                continue;
            }
            _ => {}
        }
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value);
        out.push_str("\r\n");
    }
    out.push_str(&format!("Host: {}\r\n", ctx.upstream_host));
    if !tunnel {
        out.push_str("Connection: close\r\n");
    }
    out.push_str("\r\n");
    out
}

/// 响应头改写：Location 指向本地、Cookie 去掉 Domain、丢掉会拦下本地 origin 的安全策略
fn transform_headers(headers: &[(String, String)], ctx: &SiteCtx) -> Vec<(String, String)> {
    let mut out = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        match name.to_ascii_lowercase().as_str() {
            // 上游 origin 的 CSP 会把本地 origin 判成外链；Alt-Svc 会把浏览器引去上游的 h2/h3
            "content-security-policy" | "content-security-policy-report-only" | "alt-svc" => continue,
            "location" => out.push((name.clone(), rewrite_origin(value, ctx))),
            "set-cookie" => out.push((name.clone(), rewrite_cookie(value))),
            _ => out.push((name.clone(), value.clone())),
        }
    }
    out
}

/// 绝对地址重定向改成本地 origin，相对地址原样保留
fn rewrite_origin(value: &str, ctx: &SiteCtx) -> String {
    if let Some(rest) = value.strip_prefix(&ctx.upstream_origin) {
        return format!("{}{rest}", ctx.local_origin);
    }
    if let Some(rest) = value.strip_prefix(&format!("//{}", ctx.upstream_host)) {
        return format!("//{}{rest}", ctx.local_host);
    }
    value.to_string()
}

/// Cookie：去掉 Domain（否则浏览器认为域名不匹配直接丢弃）、Secure 和 SameSite=None
/// （本地是 http，带 Secure 的 Cookie 发不出去；SameSite=None 又必须配 Secure）
fn rewrite_cookie(value: &str) -> String {
    let kept: Vec<&str> = value
        .split(';')
        .map(str::trim)
        .filter(|part| {
            let p = part.to_ascii_lowercase();
            !p.is_empty() && !p.starts_with("domain=") && p != "secure" && p != "samesite=none"
        })
        .collect();
    kept.join("; ")
}

/// 只有文本类、未压缩、非流式、非分段下载的响应才值得缓冲改写
fn should_rewrite_body(req: &Request, resp: &Response, mirrored: bool) -> bool {
    if !mirrored {
        return false;
    }
    if req.method.eq_ignore_ascii_case("HEAD") || matches!(resp.status, 204 | 304 | 206) {
        return false;
    }
    // 压缩过的响应解不开（我们已剥掉 Accept-Encoding，正常不会走到这里）
    if resp.header("content-encoding").is_some() {
        return false;
    }
    is_textual(&resp.content_type())
        && resp
            .header("content-length")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .is_none_or(|l| l <= MAX_REWRITE)
}

fn is_textual(content_type: &str) -> bool {
    // SSE 必须流式，缓冲会让页面卡住
    if content_type.contains("text/event-stream") {
        return false;
    }
    [
        "text/html",
        "application/xhtml",
        "javascript",
        "ecmascript",
        "text/css",
        "json",
        "xml",
        "text/plain",
        "graphql",
    ]
    .iter()
    .any(|k| content_type.contains(k))
}

/// 把响应体里写死的上游 origin 换成本地 origin
fn rewrite_body(body: Vec<u8>, content_type: &str, ctx: &SiteCtx, badge: bool) -> Vec<u8> {
    let body = replace_origin(&body, ctx);
    if content_type.contains("html") {
        // 改写了脚本内容，SRI 哈希必然对不上，浏览器会拒绝执行 → 剥掉 integrity 属性
        // 再在页面底部注入署名条：从本地端口看上游页面时，一眼能看出这页是 devroute 镜像的
        let body = strip_integrity(body);
        if badge { inject_footer(body) } else { body }
    } else {
        body
    }
}

/// 顶层文档才挂署名条。浏览器带 `Sec-Fetch-Dest`，iframe/embed 里的那份注入会挤在小框里
/// 挡住上游内容；老客户端不发这个头时按顶层处理
fn is_top_level(req: &Request) -> bool {
    req.header("sec-fetch-dest").is_none_or(|d| {
        !matches!(
            d.trim().to_ascii_lowercase().as_str(),
            "iframe" | "frame" | "embed" | "object"
        )
    })
}

/// 页面底部居中的署名条：纯内联样式，不依赖任何外部资源，也不会挡住页面点击
fn footer_html() -> String {
    let text = t!("banner.footer");
    format!(
        r#"<div id="devroute-footer" style="position:fixed;left:50%;bottom:10px;transform:translateX(-50%);z-index:2147483647;background:rgba(0,0,0,0.55);color:#fff;padding:5px 12px;border-radius:6px;font:12px/1.5 system-ui,-apple-system,'Segoe UI',sans-serif;pointer-events:none;white-space:nowrap;">{text}</div>"#
    )
}

/// 只往完整文档里注入：AJAX 拿到的 HTML 片段（没有 html/body 结构）插进去会破坏页面布局
fn inject_footer(body: Vec<u8>) -> Vec<u8> {
    let head = &body[..body.len().min(2048)];
    let is_document = [b"<html".as_slice(), b"<body"]
        .iter()
        .any(|marker| head.windows(marker.len()).any(|w| w.eq_ignore_ascii_case(marker)));
    if !is_document {
        return body;
    }
    crate::core::http::inject_into_html(body, &footer_html())
}

fn replace_origin(body: &[u8], ctx: &SiteCtx) -> Vec<u8> {
    // 顺序要紧：`//host` 是 https/wss/http/ws 的子串，必须最后处理
    let up = ctx.upstream_host.as_str();
    let local = ctx.local_host.as_str();
    let pairs: [(&str, &str); 5] = [
        (&format!("https://{up}"), &format!("http://{local}")),
        (&format!("wss://{up}"), &format!("ws://{local}")),
        (&format!("http://{up}"), &format!("http://{local}")),
        (&format!("ws://{up}"), &format!("ws://{local}")),
        (&format!("//{up}"), &format!("//{local}")),
    ];
    let mut out = body.to_vec();
    for (needle, repl) in pairs {
        if let Some(next) = replace_all(&out, needle.as_bytes(), repl.as_bytes()) {
            out = next;
        }
    }
    out
}

fn replace_all(hay: &[u8], needle: &[u8], repl: &[u8]) -> Option<Vec<u8>> {
    if needle.is_empty() || !hay.windows(needle.len()).any(|w| w == needle) {
        return None;
    }
    let mut out = Vec::with_capacity(hay.len() + 32);
    let mut i = 0;
    while i < hay.len() {
        if hay[i..].starts_with(needle) {
            out.extend_from_slice(repl);
            i += needle.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    Some(out)
}

/// 剥掉 HTML 里的 `integrity="..."`（只处理带引号的标准写法）
fn strip_integrity(html: Vec<u8>) -> Vec<u8> {
    const NEEDLE: &[u8] = b"integrity=";
    if !html.windows(NEEDLE.len()).any(|w| w.eq_ignore_ascii_case(NEEDLE)) {
        return html;
    }
    let mut out = Vec::with_capacity(html.len());
    let mut i = 0;
    while i < html.len() {
        if html.len() - i >= NEEDLE.len() && html[i..i + NEEDLE.len()].eq_ignore_ascii_case(NEEDLE) {
            let mut j = i + NEEDLE.len();
            if j < html.len() && (html[j] == b'"' || html[j] == b'\'') {
                let quote = html[j];
                j += 1;
                while j < html.len() && html[j] != quote {
                    j += 1;
                }
                j += 1; // 跳过结尾引号
                while out.last() == Some(&b' ') {
                    out.pop();
                }
                i = j.min(html.len());
                continue;
            }
        }
        out.push(html[i]);
        i += 1;
    }
    out
}

/// 重建响应头：content_len 为 Some 时按新长度改写（丢弃 chunked），None 时原样保留
fn rebuild_head(
    status_line: &str,
    headers: &[(String, String)],
    content_len: Option<usize>,
) -> String {
    let mut out = String::with_capacity(512);
    out.push_str(status_line);
    out.push_str("\r\n");
    for (name, value) in headers {
        let l = name.to_ascii_lowercase();
        if l == "connection" || l == "keep-alive" {
            continue;
        }
        if content_len.is_some() && (l == "content-length" || l == "transfer-encoding") {
            continue;
        }
        out.push_str(name);
        out.push_str(": ");
        out.push_str(value);
        out.push_str("\r\n");
    }
    if let Some(n) = content_len {
        out.push_str(&format!("Content-Length: {n}\r\n"));
    }
    out.push_str("Connection: close\r\n\r\n");
    out
}

fn simple_response(status: &str, msg: &str) -> Vec<u8> {
    let body = msg.as_bytes();
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

async fn read_head<S: AsyncRead + Unpin>(stream: &mut S) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::with_capacity(4096);
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let end = pos + 4;
            let rest = buf.split_off(end);
            return Ok((buf, rest));
        }
        if buf.len() > MAX_HEAD {
            return Err(anyhow!("header too large"));
        }
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(anyhow!("connection closed before headers"));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// 读到 EOF 为止。`Connection: close` 语义下 EOF 就是响应体结束，
/// 但不少服务端（含部分 TLS 实现）不发 `close_notify` 直接断开，
/// rustls 会把这种 EOF 报成 UnexpectedEof —— 当正常结束处理，否则整段响应会被丢掉。
async fn read_to_end_lenient<S: AsyncRead + Unpin>(stream: &mut S, buf: &mut Vec<u8>) -> Result<()> {
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) => return Ok(()),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }
}

/// 流式拷贝，同样把 UnexpectedEof 当作正常结束
async fn copy_lenient<R, W>(from: &mut R, to: &mut W) -> Result<u64>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut chunk = [0u8; 8192];
    let mut total = 0u64;
    loop {
        match from.read(&mut chunk).await {
            Ok(0) => return Ok(total),
            Ok(n) => {
                to.write_all(&chunk[..n]).await?;
                total += n as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(total),
            Err(e) => return Err(e.into()),
        }
    }
}

async fn copy_exact<R, W>(from: &mut R, to: &mut W, mut left: usize) -> Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = [0u8; 8192];
    while left > 0 {
        let want = left.min(buf.len());
        let n = from.read(&mut buf[..want]).await?;
        if n == 0 {
            return Err(anyhow!("client closed while sending body"));
        }
        to.write_all(&buf[..n]).await?;
        left -= n;
    }
    Ok(())
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn parse(head: &[u8]) -> Result<Self> {
        let mut buf = [httparse::EMPTY_HEADER; 64];
        let mut req = httparse::Request::new(&mut buf);
        if !matches!(req.parse(head), Ok(Status::Complete(_))) {
            return Err(anyhow!("malformed request head"));
        }
        Ok(Self {
            method: req.method.unwrap_or("GET").to_string(),
            path: req.path.unwrap_or("/").to_string(),
            headers: req
                .headers
                .iter()
                .map(|h| (h.name.to_string(), String::from_utf8_lossy(h.value).into_owned()))
                .collect(),
        })
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn content_length(&self) -> usize {
        self.header("content-length")
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    }

    fn is_websocket(&self) -> bool {
        self.header("upgrade")
            .is_some_and(|v| v.to_ascii_lowercase().contains("websocket"))
    }

    /// 请求体是 chunked（长度未知）：无法按 Content-Length 精确转发
    fn chunked_body(&self) -> bool {
        self.header("transfer-encoding")
            .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    }
}

struct Response {
    status_line: String,
    status: u16,
    headers: Vec<(String, String)>,
}

impl Response {
    fn parse(head: &[u8]) -> Result<Self> {
        let mut buf = [httparse::EMPTY_HEADER; 64];
        let mut res = httparse::Response::new(&mut buf);
        if !matches!(res.parse(head), Ok(Status::Complete(_))) {
            return Err(anyhow!("malformed response head"));
        }
        let status = res.code.unwrap_or(0);
        let status_line = head
            .windows(2)
            .position(|w| w == b"\r\n")
            .map(|p| String::from_utf8_lossy(&head[..p]).into_owned())
            .unwrap_or_else(|| format!("HTTP/1.1 {status}"));
        Ok(Self {
            status_line,
            status,
            headers: res
                .headers
                .iter()
                .map(|h| (h.name.to_string(), String::from_utf8_lossy(h.value).into_owned()))
                .collect(),
        })
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn content_type(&self) -> String {
        self.header("content-type").unwrap_or("").to_ascii_lowercase()
    }

    fn chunked(&self) -> bool {
        self.header("transfer-encoding")
            .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(upstream: &str) -> Site {
        Site {
            enable: true,
            name: "测试站".into(),
            listen_addr: "127.0.0.1:8088".into(),
            upstream: upstream.into(),
            insecure: false,
            rules: vec![],
        }
    }

    fn ctx() -> SiteCtx {
        SiteCtx::new(&site("http://192.168.120.177:81")).unwrap()
    }

    #[test]
    fn parses_upstream() {
        let c = ctx();
        assert_eq!(c.upstream_host, "192.168.120.177:81");
        assert_eq!(c.upstream_origin, "http://192.168.120.177:81");
        assert_eq!(c.local_origin, "http://127.0.0.1:8088");
        assert!(c.tls.is_none());
        // 不带端口按协议补默认端口
        let c = SiteCtx::new(&site("http://example.com/")).unwrap();
        assert_eq!(c.upstream_host, "example.com:80");
        let c = SiteCtx::new(&site("https://example.com")).unwrap();
        assert_eq!(c.upstream_host, "example.com:443");
        assert_eq!(c.upstream_origin, "https://example.com:443");
        assert!(c.tls.is_some());
        // 协议写错明确报错，而不是悄悄跑出个坏结果
        assert!(SiteCtx::new(&site("ftp://example.com")).is_err());
        assert!(SiteCtx::new(&site("example.com:81")).is_err());
        assert!(SiteCtx::new(&site("http://example.com/a/b")).is_err());
    }

    #[test]
    fn wildcard_listen_maps_to_loopback() {
        let mut s = site("http://a.com:81");
        s.listen_addr = "0.0.0.0:8088".into();
        let c = SiteCtx::new(&s).unwrap();
        assert_eq!(c.local_origin, "http://127.0.0.1:8088");
    }

    #[test]
    fn insecure_site_builds_tls_config() {
        install_crypto_provider();
        let mut s = site("https://192.168.120.177:81");
        s.insecure = true;
        let c = SiteCtx::new(&s).unwrap();
        assert!(c.insecure);
        assert!(c.tls.is_some());
    }

    #[test]
    fn rewrites_absolute_urls_in_body() {
        let c = ctx();
        let js = br#"var base="http://192.168.120.177:81/api";var w="ws://192.168.120.177:81/ws";"#;
        let out = String::from_utf8(rewrite_body(js.to_vec(), "application/javascript", &c, true)).unwrap();
        assert!(out.contains("http://127.0.0.1:8088/api"));
        assert!(out.contains("ws://127.0.0.1:8088/ws"));
        assert!(!out.contains("192.168.120.177"));
    }

    #[test]
    fn rewrites_protocol_relative_and_https_forms() {
        let c = ctx();
        let html = br#"<a href="//192.168.120.177:81/x">a</a><b>https://192.168.120.177:81/y</b>"#;
        let out = String::from_utf8(rewrite_body(html.to_vec(), "text/html", &c, true)).unwrap();
        assert!(out.contains("//127.0.0.1:8088/x"));
        // 上游写 https 但本地是 http，协议也要跟着降
        assert!(out.contains("http://127.0.0.1:8088/y"));
        assert!(!out.contains("192.168.120.177"));
    }

    #[test]
    fn rewrites_https_upstream_urls() {
        let c = SiteCtx::new(&site("https://a.com")).unwrap();
        let js = br#"fetch("https://a.com:443/api")"#;
        let out = String::from_utf8(rewrite_body(js.to_vec(), "application/javascript", &c, true)).unwrap();
        assert_eq!(out, r#"fetch("http://127.0.0.1:8088/api")"#);
    }

    #[test]
    fn strips_integrity_after_rewrite() {
        let c = ctx();
        let html = br#"<script src="/a.js" integrity="sha384-abc" crossorigin></script>"#;
        let out = String::from_utf8(rewrite_body(html.to_vec(), "text/html", &c, true)).unwrap();
        assert!(!out.contains("integrity"));
        assert!(out.contains(r#"src="/a.js" crossorigin>"#));
    }

    #[test]
    fn strips_cookie_domain_and_secure() {
        assert_eq!(
            rewrite_cookie("sid=1; Domain=192.168.120.177; Path=/; Secure; HttpOnly"),
            "sid=1; Path=/; HttpOnly"
        );
        // SameSite=None 必须配 Secure，Secure 被去掉后它也得走
        assert_eq!(rewrite_cookie("sid=1; SameSite=None"), "sid=1");
        assert_eq!(rewrite_cookie("sid=1; SameSite=Lax"), "sid=1; SameSite=Lax");
    }

    #[test]
    fn rewrites_location() {
        let c = ctx();
        assert_eq!(
            rewrite_origin("http://192.168.120.177:81/login", &c),
            "http://127.0.0.1:8088/login"
        );
        assert_eq!(rewrite_origin("//192.168.120.177:81/x", &c), "//127.0.0.1:8088/x");
        // 相对地址和第三方地址不动
        assert_eq!(rewrite_origin("/login", &c), "/login");
        assert_eq!(rewrite_origin("https://other.com/a", &c), "https://other.com/a");
    }

    #[test]
    fn drops_csp_and_alt_svc() {
        let c = ctx();
        let headers = vec![
            ("Content-Security-Policy".to_string(), "default-src http://192.168.120.177:81".to_string()),
            ("Alt-Svc".to_string(), "h3=\":443\"".to_string()),
            ("Content-Type".to_string(), "text/html".to_string()),
        ];
        let out = transform_headers(&headers, &c);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "Content-Type");
    }

    #[test]
    fn rewrites_only_textual_bodies() {
        let req = Request {
            method: "GET".into(),
            path: "/a.js".into(),
            headers: vec![],
        };
        let resp = |ct: &str| Response {
            status_line: "HTTP/1.1 200 OK".into(),
            status: 200,
            headers: vec![("Content-Type".to_string(), ct.to_string())],
        };
        assert!(should_rewrite_body(&req, &resp("text/html; charset=utf-8"), true));
        assert!(should_rewrite_body(&req, &resp("application/javascript"), true));
        // 二进制/流式/分段下载不改写
        assert!(!should_rewrite_body(&req, &resp("image/png"), true));
        assert!(!should_rewrite_body(&req, &resp("text/event-stream"), true));
        assert!(!should_rewrite_body(&req, &resp("video/mp4"), true));
        // 转发到本地服务的不改写（那是接口数据，不是页面）
        assert!(!should_rewrite_body(&req, &resp("text/html"), false));
        // HEAD 没有响应体
        let head_req = Request {
            method: "HEAD".into(),
            path: "/".into(),
            headers: vec![],
        };
        assert!(!should_rewrite_body(&head_req, &resp("text/html"), true));
    }

    #[test]
    fn compressed_response_passes_through() {
        let req = Request {
            method: "GET".into(),
            path: "/".into(),
            headers: vec![],
        };
        let resp = Response {
            status_line: "HTTP/1.1 200 OK".into(),
            status: 200,
            headers: vec![
                ("Content-Type".to_string(), "text/html".to_string()),
                ("Content-Encoding".to_string(), "gzip".to_string()),
            ],
        };
        assert!(!should_rewrite_body(&req, &resp, true));
    }

    #[test]
    fn builds_upstream_head_with_host_and_close() {
        let c = ctx();
        let req = Request {
            method: "GET".into(),
            path: "/api/x".into(),
            headers: vec![
                ("Host".to_string(), "127.0.0.1:8088".to_string()),
                ("Accept-Encoding".to_string(), "gzip, br".to_string()),
                ("Origin".to_string(), "http://127.0.0.1:8088".to_string()),
                ("Cookie".to_string(), "sid=1".to_string()),
            ],
        };
        let head = build_upstream_head(&req, "/api/x", true, false, &c);
        assert!(head.starts_with("GET /api/x HTTP/1.1\r\n"));
        assert!(head.contains("Host: 192.168.120.177:81\r\n"));
        assert!(head.contains("Origin: http://192.168.120.177:81\r\n"));
        assert!(head.contains("Cookie: sid=1\r\n"));
        assert!(!head.to_ascii_lowercase().contains("accept-encoding"));
        assert!(head.contains("Connection: close\r\n"));
    }

    #[test]
    fn websocket_keeps_upgrade_headers() {
        let c = ctx();
        let req = Request {
            method: "GET".into(),
            path: "/ws".into(),
            headers: vec![
                ("Connection".to_string(), "Upgrade".to_string()),
                ("Upgrade".to_string(), "websocket".to_string()),
            ],
        };
        assert!(req.is_websocket());
        let head = build_upstream_head(&req, "/ws", true, true, &c);
        assert!(head.contains("Connection: Upgrade\r\n"));
        assert!(head.contains("Upgrade: websocket\r\n"));
        assert!(!head.contains("Connection: close"));
    }

    #[test]
    fn chunked_request_body_goes_through_tunnel() {
        let c = ctx();
        let req = Request {
            method: "POST".into(),
            path: "/upload".into(),
            headers: vec![("Transfer-Encoding".to_string(), "chunked".to_string())],
        };
        assert!(req.chunked_body());
        // 隧道模式必须保留 chunked framing，否则上游会按 Content-Length=0 处理
        let head = build_upstream_head(&req, "/upload", true, true, &c);
        assert!(head.to_ascii_lowercase().contains("transfer-encoding: chunked"));
        // 普通请求则剥掉，统一走 Connection: close 一跳一答
        let head = build_upstream_head(&req, "/upload", true, false, &c);
        assert!(!head.to_ascii_lowercase().contains("transfer-encoding"));
    }

    #[test]
    fn injects_footer_into_mirrored_page() {
        let c = ctx();
        let html = br#"<html><body><h1>hi</h1></body></html>"#;
        let out = String::from_utf8(rewrite_body(html.to_vec(), "text/html; charset=utf-8", &c, true)).unwrap();
        // 注入在 </body> 之前，且只出现一次
        assert_eq!(out.matches("id=\"devroute-footer\"").count(), 1);
        assert!(out.contains("id=\"devroute-footer\""));
        assert!(out.find("devroute-footer").unwrap() < out.find("</body>").unwrap());
        // 中英文案都带品牌名，界面语言切换后仍能识别
        assert!(out.contains("devroute"));
    }

    #[test]
    fn footer_skipped_for_html_fragment() {
        // AJAX 拿到的 HTML 片段没有文档结构，插进去会破坏布局
        let c = ctx();
        let out = rewrite_body(b"<div>row</div>".to_vec(), "text/html", &c, true);
        assert_eq!(out, b"<div>row</div>".to_vec());
        // 非 HTML 的文本资源不注入（JS / JSON 里插 div 会直接语法错误）
        let js = rewrite_body(b"var a=1;".to_vec(), "application/javascript", &c, true);
        assert_eq!(js, b"var a=1;".to_vec());
    }

    #[test]
    fn footer_follows_ui_language() {
        // 署名条文案跟界面语言走：英文界面注入英文，中文界面注入中文
        rust_i18n::set_locale("en");
        assert!(footer_html().contains("Powered by devroute"));
        rust_i18n::set_locale("zh");
        assert!(footer_html().contains("由 devroute 提供支持"));
    }

    #[test]
    fn iframe_documents_skip_badge() {
        let c = ctx();
        let html = || b"<html><body>x</body></html>".to_vec();
        // badge=false：只改写 origin / 剥 integrity，不挂署名条
        let out = String::from_utf8(rewrite_body(html(), "text/html", &c, false)).unwrap();
        assert!(!out.contains("devroute-footer"));
        // 浏览器发 Sec-Fetch-Dest: iframe 的请求不算顶层文档
        let req = |dest: Option<&str>| Request {
            method: "GET".into(),
            path: "/".into(),
            headers: dest
                .map(|d| vec![("Sec-Fetch-Dest".to_string(), d.to_string())])
                .unwrap_or_default(),
        };
        assert!(!is_top_level(&req(Some("iframe"))));
        assert!(!is_top_level(&req(Some("embed"))));
        assert!(is_top_level(&req(Some("document"))));
        // 老客户端不带这个头，按顶层处理
        assert!(is_top_level(&req(None)));
    }

    #[test]
    fn footer_html_uses_escape_safe_inline_markup() {
        let f = footer_html();
        assert!(f.starts_with("<div id=\"devroute-footer\""));
        assert!(!f.contains("<script")); // 不改动页面脚本环境
    }

    #[test]
    fn rebuild_head_keeps_length_when_streaming() {        let headers = vec![
            ("Content-Type".to_string(), "text/plain".to_string()),
            ("Content-Length".to_string(), "3".to_string()),
        ];
        let out = rebuild_head("HTTP/1.1 200 OK", &headers, None);
        assert!(out.contains("Content-Length: 3\r\n"));
        // 改写后按新长度重建，chunked 一并丢掉
        let headers = vec![("Transfer-Encoding".to_string(), "chunked".to_string())];
        let out = rebuild_head("HTTP/1.1 200 OK", &headers, Some(7));
        assert!(out.contains("Content-Length: 7\r\n"));
        assert!(!out.to_ascii_lowercase().contains("transfer-encoding"));
    }
}
