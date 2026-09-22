#[derive(Debug, Clone)]
pub(crate) struct RouteRule {
    /// 匹配条件
    pub(crate) match_: Match,
    /// 转发信息
    pub(crate) forward: Forward,
}

impl RouteRule {
    pub(crate) fn new(
        match_host: &str,
        match_path_prefix: &str,
        forward_host: &str,
        forward_path_prefix: &str,
    ) -> Self {
        Self {
            match_: Match {
                host: match_host.to_string(),
                prefix: match_path_prefix.to_string(),
            },
            forward: Forward {
                host: forward_host.to_string(),
                prefix: forward_path_prefix.to_string(),
                rewrite: true,
                connect_fail_use_original_host: false,
            },
        }
    }
    fn matches(&self, host: &str, prefix: &str) -> bool {
        prefix.starts_with(&self.match_.prefix) && self.match_host(host)
    }
    /// 规则不写端口时默认匹配 80/443；写了端口则精确匹配。域名大小写不敏感。
    // ponytail: 只支持 IPv4/域名，IPv6 字面量的冒号会误判为端口分隔
    fn match_host(&self, target: &str) -> bool {
        if self.match_.host == "*" {
            return true;
        }
        let (t_host, t_port) = match target.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (target, None),
        };
        match self.match_.host.rsplit_once(':') {
            Some((r_host, r_port)) => {
                r_host.eq_ignore_ascii_case(t_host) && (t_port.is_none() || Some(r_port) == t_port)
            }
            // 不写端口：默认兼容 80/443
            None => {
                self.match_.host.eq_ignore_ascii_case(t_host)
                    && matches!(t_port, None | Some("80") | Some("443"))
            }
        }
    }
}

/// 匹配
#[derive(Clone, Debug)]
pub(crate) struct Match {
    /// 匹配目标域名或IP:PORT，也可以是 * 匹配所有
    pub(crate) host: String,
    /// 匹配目标请求地址前缀
    pub(crate) prefix: String,
}

/// 转发
#[derive(Clone, Debug)]
pub struct Forward {
    /// 转发到目标地址
    pub(crate) host: String,
    /// 转发到目标地址的前缀
    pub(crate) prefix: String,
    /// 自动替换前缀 match.prefix替换为forward.prefix
    pub(crate) rewrite: bool,
    /// 转发地址连接失败时使用原始地址
    pub(crate) connect_fail_use_original_host: bool,
}

use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::debug;

pub(crate) struct RouteEngine {
    pub(crate) rules: Arc<RwLock<Vec<RouteRule>>>,
}

impl RouteEngine {
    #[allow(unused)]
    pub(crate) async fn resolve_target(&self, host: &str, path: &str) -> Option<RouteRule> {
        debug!("Resolving target {host}{path}");
        let rules = self.rules.read().await;
        for rule in rules.iter() {
            // 匹配IP:PORT + 路径前缀
            if rule.matches(host, path) {
                return Some(rule.clone());
            }
        }
        None
    }

    pub(crate) async fn resolve_target_by_host(&self, host: &str) -> Option<RouteRule> {
        let rules = self.rules.read().await;
        for rule in rules.iter() {
            if rule.match_host(host) {
                return Some(rule.clone());
            }
        }
        None
    }

    // 动态更新规则
    pub(crate) async fn update_rules(&self, new_rules: Vec<RouteRule>) {
        let mut rules = self.rules.write().await;
        *rules = new_rules;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(host: &str) -> RouteRule {
        RouteRule::new(host, "/api", "127.0.0.1:8686", "")
    }

    #[test]
    fn host_matching() {
        // 精确匹配
        assert!(rule("example.com:81").match_host("example.com:81"));
        assert!(!rule("example.com:81").match_host("example.com:80"));
        // 不写端口：默认 80/443
        assert!(rule("example.com").match_host("example.com:80"));
        assert!(rule("example.com").match_host("example.com:443"));
        assert!(!rule("example.com").match_host("example.com:81"));
        // 大小写不敏感
        assert!(rule("Example.COM").match_host("example.com:80"));
        // 通配
        assert!(rule("*").match_host("anything:1234"));
        // 域名不混淆
        assert!(!rule("example.com").match_host("notexample.com:80"));
    }
}
