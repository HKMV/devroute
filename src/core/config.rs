use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AppConfig {
    /// 规则列表
    pub rules: Vec<Rule>,
    pub listen_addr: String,
    /// GUI 主题: system | dark | light
    pub theme: String,
    /// 界面语言: auto | zh | en（auto = 跟随系统）
    pub language: String,
    /// 启动代理时自动设置系统代理
    pub auto_proxy: bool,
}
impl Default for AppConfig {
    fn default() -> Self {
        Self {
            rules: vec![Rule {
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
            listen_addr: "127.0.0.1:1080".to_string(),
            theme: "system".to_string(),
            language: "auto".to_string(),
            auto_proxy: false,
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
            "listen_addr = {}\ntheme = {}\nlanguage = {}\nauto_proxy = {}\n",
            toml_basic_str(&self.listen_addr),
            toml_basic_str(&self.theme),
            toml_basic_str(&self.language),
            self.auto_proxy
        );
        for r in &self.rules {
            s.push_str(&format!(
                "\n[[rules]]\nmatcher = {{ addr = {}, path_prefix = {} }}\nforward = {{ addr = {}, path_prefix = {} }}\n",
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
            listen_addr: "127.0.0.1:1080".into(),
            theme: "dark".into(),
            language: "en".into(),
            auto_proxy: false,
            rules: vec![
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
        let back: AppConfig = toml::from_str(&content).unwrap();
        assert_eq!(back.rules.len(), 2);
        assert_eq!(back.rules[1].matcher.path_prefix, "/x\\y\"z");
        assert_eq!(back.listen_addr, "127.0.0.1:1080");
        assert_eq!(back.theme, "dark");
        assert_eq!(back.language, "en");
        assert!(!back.auto_proxy);
        assert!(content.contains("auto_proxy = false"));
        assert_eq!(resolve_locale("auto"), if sys_locale::get_locale().unwrap_or_default().to_lowercase().starts_with("zh") { "zh" } else { "en" });
        assert_eq!(resolve_locale("zh"), "zh");
        assert_eq!(resolve_locale("en"), "en");
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
