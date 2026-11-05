use crate::core::config::{AppConfig, Rule, Site, SiteRule};
use crate::core::mirror::{self, SiteCtx};
use crate::core::route::{RouteEngine, RouteRule};
use crate::core::stats::Stats;
use anyhow::Context;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{RwLock, mpsc};
use tracing::{error, info};

/// 两种模式各跑一个独立运行时，各自收自己的命令（不适用的一侧直接忽略）
pub enum Command {
    /// 代理模式：热更新规则
    SetProxyRules(Vec<Rule>),
    /// 镜像站点：热更新各站点内嵌规则（站点结构没变时才发这个）
    SetSiteRules(Vec<Site>),
    Shutdown,
}

/// 在后台线程起一个 tokio runtime 跑 SOCKS5/HTTP 代理监听，返回命令通道。
/// 发送 Shutdown（或 drop 通道）后线程自行退出；再次启动就再调一次。
pub fn spawn_proxy(config: AppConfig, stats: Arc<Stats>) -> mpsc::UnboundedSender<Command> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async move {
            if let Err(e) = run_proxy(config, rx, stats).await {
                error!("代理模式退出: {e:#}");
            }
        });
    });
    tx
}

/// 在后台线程起一个 tokio runtime 跑所有启用的镜像站点，返回命令通道
pub fn spawn_sites(sites: Vec<Site>, stats: Arc<Stats>) -> mpsc::UnboundedSender<Command> {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async move {
            if let Err(e) = run_sites(sites, rx, stats).await {
                error!("镜像站点退出: {e:#}");
            }
        });
    });
    tx
}

fn to_route_rules(rules: &[Rule]) -> Vec<RouteRule> {
    rules
        .iter()
        .map(|r| {
            RouteRule::new(
                &r.matcher.addr,
                &r.matcher.path_prefix,
                &r.forward.addr,
                &r.forward.path_prefix,
            )
        })
        .collect()
}

/// 站点规则转成内部规则：host 固定 `*`（站点已绑定上游，只按路径前缀匹配）。
/// 转发地址为空的规则直接丢掉 —— 空地址连不上，空前缀还会把整个站点兜底走本地
fn to_site_rules(rules: &[SiteRule]) -> Vec<RouteRule> {
    rules
        .iter()
        .filter(|r| !r.forward_addr.trim().is_empty())
        .map(|r| {
            RouteRule::new(
                "*",
                &r.path_prefix,
                &r.forward_addr,
                &r.forward_prefix,
            )
        })
        .collect()
}

fn site_engine(site: &Site) -> Arc<RouteEngine> {
    Arc::new(RouteEngine {
        rules: Arc::new(RwLock::new(to_site_rules(&site.rules))),
    })
}

/// 为一个镜像站点起监听；站点自己的 engine 决定哪些路径转本地
async fn spawn_site(site: &Site, engine: Arc<RouteEngine>, stats: Arc<Stats>) -> anyhow::Result<()> {
    let ctx = Arc::new(SiteCtx::new(site)?);
    let listener = TcpListener::bind(&site.listen_addr)
        .await
        .with_context(|| format!("镜像站点 [{}] 监听 {} 失败", site.name, site.listen_addr))?;
    info!(
        "镜像站点 [{}] listening on {} → 页面来自 {}{}（{} 条转发规则）",
        ctx.name,
        ctx.local_origin,
        ctx.upstream_origin,
        if ctx.insecure { "（已跳过证书校验）" } else { "" },
        site.rules.len()
    );
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((socket, _)) => {
                    let engine = engine.clone();
                    let stats = stats.clone();
                    let ctx = ctx.clone();
                    let name = ctx.name.clone();
                    tokio::spawn(async move {
                        if let Err(e) = mirror::handle(socket, engine, stats, ctx).await {
                            error!("镜像站点 [{name}] 连接处理失败: {e}");
                        }
                    });
                }
                Err(e) => {
                    error!("镜像站点 [{}] accept 失败: {e}", ctx.name);
                    break;
                }
            }
        }
    });
    Ok(())
}

async fn run_proxy(
    config: AppConfig,
    mut cmd_rx: mpsc::UnboundedReceiver<Command>,
    stats: Arc<Stats>,
) -> anyhow::Result<()> {
    let engine = Arc::new(RouteEngine {
        rules: Arc::new(RwLock::new(to_route_rules(&config.proxy_rules))),
    });
    let listener = TcpListener::bind(&config.proxy_listen)
        .await
        .with_context(|| format!("代理监听 {} 失败", config.proxy_listen))?;
    info!("SOCKS5/HTTP 代理 listening on {}", config.proxy_listen);

    loop {
        tokio::select! {
            accept = listener.accept() => {
                let (socket, _) = accept?;
                let engine = engine.clone();
                let stats = stats.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::core::socks::handle_client(socket, engine, stats).await
                    {
                        error!("Error handling client: {}", e);
                    }
                });
            }
            cmd = cmd_rx.recv() => match cmd {
                Some(Command::SetProxyRules(rules)) => {
                    let n = rules.len();
                    engine.update_rules(to_route_rules(&rules)).await;
                    info!("代理规则已更新（{n} 条）");
                }
                Some(Command::Shutdown) | None => break,
                _ => {}
            }
        }
    }
    info!("代理模式已停止");
    Ok(())
}

async fn run_sites(
    sites: Vec<Site>,
    mut cmd_rx: mpsc::UnboundedReceiver<Command>,
    stats: Arc<Stats>,
) -> anyhow::Result<()> {
    // https 上游要用 rustls 的默认加密实现（只编译了 ring，必须显式装一次）
    mirror::install_crypto_provider();

    // 监听地址 → engine：规则热更新时按地址找回对应站点的 engine
    let mut engines: Vec<(String, Arc<RouteEngine>)> = Vec::new();
    for site in sites.iter().filter(|s| s.enable) {
        let engine = site_engine(site);
        spawn_site(site, engine.clone(), stats.clone()).await?;
        engines.push((site.listen_addr.clone(), engine));
    }
    if engines.is_empty() {
        info!("没有启用的镜像站点");
    }

    loop {
        tokio::select! {
            cmd = cmd_rx.recv() => match cmd {
                Some(Command::SetSiteRules(sites)) => {
                    let mut n = 0;
                    for (addr, engine) in &engines {
                        if let Some(site) = sites.iter().find(|s| &s.listen_addr == addr) {
                            n += site.rules.len();
                            engine.update_rules(to_site_rules(&site.rules)).await;
                        }
                    }
                    info!("站点规则已更新（{n} 条）");
                }
                Some(Command::Shutdown) | None => break,
                _ => {}
            }
        }
    }
    info!("镜像站点已停止");
    Ok(())
}
