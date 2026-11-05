use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AppConfig {
    /// 代理模式的转发规则（镜像站点各自内嵌规则，不读这张表）
    #[serde(alias = "rules")]
    pub proxy_rules: Vec<Rule>,
    /// 代理监听地址
    #[serde(alias = "listen_addr")]
    pub proxy_listen: String,
    /// GUI 主题: system | dark | light
    pub theme: String,
    /// 界面语言: auto | zh | en（auto = 跟随系统）
    pub language: String,
    /// 启动代理时自动设置系统代理
    pub auto_proxy: bool,
    /// headless 模式下是否启动代理监听；GUI 由界面上的启停按钮决定，忽略此项
    pub proxy_enable: bool,
    /// 镜像站点：每个站点一个本地端口，页面取自 upstream，接口按站点内嵌规则转发
    pub sites: Vec<Site>,
}
impl Default for AppConfig {
    fn default() -> Self {
        Self {
            proxy_rules: vec![Rule {
                //默认示例
                matcher: Host {
                    addr: "192.168.120.177:81".to_string(),
                    path_prefix: "/api".to_string(),
                },
                forward: Host {
                    addr: "127.0.0.1:8686".to_string(),
                    path_prefix: "".to_string(),
                },
            }],
            proxy_listen: "127.0.0.1:1080".to_string(),
            theme: "system".to_string(),
            language: "auto".to_string(),
            auto_proxy: false,
            proxy_enable: true,
            sites: vec![Site::default()],
        }
    }
}
impl AppConfig {
    pub(crate) fn init() -> anyhow::Result<Self> {
        let conf_file_path = "config.toml";
        match std::fs::read_to_string(conf_file_path) {
            Ok(content) => Ok(toml::from_str(&content)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // 首次运行生成默认配置
                let config = AppConfig::default();
                config.save_to(conf_file_path)?;
                Ok(config)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// 保存为内联表格式：避免 [[rules]]+[rules.matcher] 重复表头被编辑器 TOML 检查器误报
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to("config.toml")
    }

    fn save_to(&self, path: &str) -> anyhow::Result<()> {
        let mut s = format!(
            "proxy_listen = {}\nproxy_enable = {}\ntheme = {}\nlanguage = {}\nauto_proxy = {}\n",
            toml_basic_str(&self.proxy_listen),
            self.proxy_enable,
            toml_basic_str(&self.theme),
            toml_basic_str(&self.language),
            self.auto_proxy
        );
        // [[sites]] / [[proxy_rules]] 都是数组表，必须写在所有普通键之后；
        // 站点自己的规则是 [[sites.rules]]，必须紧跟在该 [[sites]] 的键之后
        for site in &self.sites {
            s.push_str(&format!(
                "\n[[sites]]\nenable = {}\nname = {}\nlisten_addr = {}\nupstream = {}\ninsecure = {}\n",
                site.enable,
                toml_basic_str(&site.name),
                toml_basic_str(&site.listen_addr),
                toml_basic_str(&site.upstream),
                site.insecure,
            ));
            for r in &site.rules {
                s.push_str(&format!(
                    "\n[[sites.rules]]\npath_prefix = {}\nforward_addr = {}\nforward_prefix = {}\n",
                    toml_basic_str(&r.path_prefix),
                    toml_basic_str(&r.forward_addr),
                    toml_basic_str(&r.forward_prefix),
                ));
            }
        }
        for r in &self.proxy_rules {
            s.push_str(&format!(
                "\n[[proxy_rules]]\nmatcher = {{ addr = {}, path_prefix = {} }}\nforward = {{ addr = {}, path_prefix = {} }}\n",
                toml_basic_str(&r.matcher.addr),
                toml_basic_str(&r.matcher.path_prefix),
                toml_basic_str(&r.forward.addr),
                toml_basic_str(&r.forward.path_prefix),
            ));
        }
        std::fs::write(path, s)?;
        Ok(())
    }
}

/// TOML basic string 转义
fn toml_basic_str(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

/// 由配置值解析实际 locale：zh/en 直用，auto 跟随系统，非中文系统回退英文
pub fn resolve_locale(cfg_lang: &str) -> &'static str {
    match cfg_lang {
        "zh" => "zh",
        "en" => "en",
        _ => {
            let sys = sys_locale::get_locale().unwrap_or_default().to_lowercase();
            if sys.starts_with("zh") { "zh" } else { "en" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_roundtrip() {
        let config = AppConfig {
            proxy_listen: "127.0.0.1:1080".into(),
            theme: "dark".into(),
            language: "en".into(),
            auto_proxy: false,
            proxy_enable: false,
            sites: vec![
                Site {
                    enable: true,
                    name: "调试站".into(),
                    listen_addr: "127.0.0.1:8088".into(),
                    upstream: "https://192.168.120.177:81".into(),
                    insecure: true,
                    rules: vec![
                        SiteRule {
                            path_prefix: "/api".into(),
                            forward_addr: "127.0.0.1:8686".into(),
                            forward_prefix: String::new(),
                        },
                        SiteRule {
                            path_prefix: "/upload".into(),
                            forward_addr: "127.0.0.1:8686".into(),
                            forward_prefix: "/v1".into(),
                        },
                    ],
                },
                Site {
                    enable: false,
                    name: String::new(),
                    listen_addr: "127.0.0.1:8089".into(),
                    upstream: "http://192.168.120.181:81".into(),
                    insecure: false,
                    rules: Vec::new(),
                },
            ],
            proxy_rules: vec![
                Rule {
                    matcher: Host { addr: "a.com:80".into(), path_prefix: "/api".into() },
                    forward: Host { addr: "127.0.0.1:8686".into(), path_prefix: "".into() },
                },
                Rule {
                    matcher: Host { addr: "b.com".into(), path_prefix: "/x\\y\"z".into() },
                    forward: Host { addr: "127.0.0.1:1".into(), path_prefix: "/p".into() },
                },
            ],
        };
        let dir = std::env::temp_dir().join("pf_cfg_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        config.save_to(path.to_str().unwrap()).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        // 编辑器友好的关键断言：没有重复的 [rules.xxx] 表头
        assert!(!content.contains("[rules.matcher]"));
        assert!(!content.contains("[proxy_rules.matcher]"));
        let back: AppConfig = toml::from_str(&content).unwrap();
        assert_eq!(back.proxy_rules.len(), 2);
        assert_eq!(back.proxy_rules[1].matcher.path_prefix, "/x\\y\"z");
        assert_eq!(back.proxy_listen, "127.0.0.1:1080");
        assert_eq!(back.theme, "dark");
        assert_eq!(back.language, "en");
        assert!(!back.auto_proxy);
        assert!(content.contains("auto_proxy = false"));
        assert!(!back.proxy_enable);
        // 数组表必须写在普通键之后，否则 [[sites]] 里的键会被解析到别的表里
        assert!(content.find("proxy_enable").unwrap() < content.find("[[sites]]").unwrap());
        assert!(content.find("[[sites]]").unwrap() < content.find("[[proxy_rules]]").unwrap());
        assert_eq!(back.sites.len(), 2);
        assert!(back.sites[0].enable);
        assert!(back.sites[0].insecure);
        assert_eq!(back.sites[0].upstream, "https://192.168.120.177:81");
        assert!(!back.sites[1].enable);
        // 站点规则必须归到自己站点名下，不能串到下一个 [[sites]]
        assert_eq!(back.sites[0].rules.len(), 2);
        assert_eq!(back.sites[0].rules[1].path_prefix, "/upload");
        assert_eq!(back.sites[0].rules[1].forward_prefix, "/v1");
        assert!(back.sites[1].rules.is_empty());
        assert_eq!(back.proxy_rules[0].matcher.addr, "a.com:80");
        assert_eq!(resolve_locale("auto"), if sys_locale::get_locale().unwrap_or_default().to_lowercase().starts_with("zh") { "zh" } else { "en" });
        assert_eq!(resolve_locale("zh"), "zh");
        assert_eq!(resolve_locale("en"), "en");
    }

    #[test]
    fn reads_legacy_key_names() {
        // 旧版配置的 listen_addr / [[rules]] 仍能读进来，站点没有内嵌规则时为空
        let legacy = r#"
listen_addr = "127.0.0.1:1080"
theme = "light"
language = "zh"
auto_proxy = true

[[sites]]
enable = true
name = "旧站"
listen_addr = "127.0.0.1:8088"
upstream = "http://192.168.120.177:81"
insecure = false

[[rules]]
matcher = { addr = "192.168.120.177:81", path_prefix = "/api" }
forward = { addr = "127.0.0.1:8686", path_prefix = "" }
"#;
        let c: AppConfig = toml::from_str(legacy).unwrap();
        assert_eq!(c.proxy_listen, "127.0.0.1:1080");
        assert_eq!(c.proxy_rules.len(), 1);
        assert_eq!(c.proxy_rules[0].matcher.path_prefix, "/api");
        assert_eq!(c.sites.len(), 1);
        assert!(c.sites[0].rules.is_empty());
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rule {
    /// 匹配配置
    pub matcher: Host,
    /// 转发配置
    pub forward: Host,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Host {
    /// host:port
    pub addr: String,
    /// 路径前缀
    pub path_prefix: String,
}

/// 镜像站点：本地开一个端口，页面取自上游站点，接口按站点自己的规则转发到本地服务。
/// 浏览器访问本地地址即为「上游页面 + 本地接口」，不依赖系统代理，也不会有 CORS/Cookie 问题。
///
/// 规则内嵌在站点里（`[[sites.rules]]`），只对这个站点的请求生效，
/// 和代理模式的 `[[proxy_rules]]` 互不干扰 —— 两种用法可以独立启用。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Site {
    /// 是否启用这个站点
    pub enable: bool,
    /// 站点备注名（仅界面/日志展示用）
    pub name: String,
    /// 本地访问入口，浏览器访问这个地址
    pub listen_addr: String,
    /// 上游站点地址，支持 http:// 与 https://
    pub upstream: String,
    /// 跳过上游证书校验（内网自签证书时用）
    pub insecure: bool,
    /// 本站点的转发规则：路径前缀命中就转到本地服务，否则照常取上游。
    /// 字段级 default：旧配置没有这段时是空规则，而不是 Site::default() 里的示例规则
    #[serde(default)]
    pub rules: Vec<SiteRule>,
}

/// 站点内嵌规则：站点已经绑定了上游，所以只需按路径前缀匹配
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SiteRule {
    /// 匹配的路径前缀
    pub path_prefix: String,
    /// 转发到本地服务 host:port
    pub forward_addr: String,
    /// 转发前缀：替换掉匹配到的 path_prefix
    pub forward_prefix: String,
}

impl Default for SiteRule {
    fn default() -> Self {
        Self {
            path_prefix: "/api".to_string(),
            forward_addr: "127.0.0.1:8686".to_string(),
            forward_prefix: String::new(),
        }
    }
}

impl Default for Site {
    fn default() -> Self {
        Self {
            enable: false,
            name: "调试站".to_string(),
            listen_addr: "127.0.0.1:8088".to_string(),
            upstream: "http://192.168.120.177:81".to_string(),
            insecure: false,
            rules: vec![SiteRule::default()],
        }
    }
}
