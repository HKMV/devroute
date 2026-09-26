//! 业务逻辑：规则模型、代理进程控制、配置持久化、定时刷新。

use crate::core::config::{AppConfig, Host, Rule};
use crate::core::stats::Stats;
use crate::daemon::{self, Command};
use crate::ui::view::{self, View};
use rust_i18n::t;
use rust_i18n::set_locale;
use fltk::enums::CallbackTrigger;
use fltk::{app, frame::Frame, group::Pack, input::Input, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc::UnboundedSender;

/// 一行规则：id 做稳定标识（删除后索引会错位，id 不会）
pub struct RuleRow {
    pub id: u64,
    pub rule: Rule,
    pub widget: fltk::group::Flex,
    pub inputs: [Input; 4],
    pub arrow: Frame,
    pub del_btn: fltk::button::Button,
}

pub type Model = Rc<RefCell<Vec<RuleRow>>>;
/// (命令通道, 运行中的监听地址)
type DaemonSlot = Rc<RefCell<Option<(UnboundedSender<Command>, String)>>>;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn new_model() -> Model {
    Rc::new(RefCell::new(Vec::new()))
}

pub fn new_daemon_slot() -> DaemonSlot {
    Rc::new(RefCell::new(None))
}

pub fn rules_of(model: &Model) -> Vec<Rule> {
    model.borrow().iter().map(|e| e.rule.clone()).collect()
}

/// 追加一行规则（增量，不做整表重建——重建会在鼠标按住时删控件导致卡死）
pub fn add_rule(pack: &Pack, model: &Model, rule: Rule) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let idx = model.borrow().len() as i32;
    let mut w = view::add_rule_row_widgets(pack, view::current_pal(), idx, &rule);

    // 编辑写模型（按 id 定位，删除后不错位）
    for (inp, field) in [
        (&w.m_addr, 0),
        (&w.m_prefix, 1),
        (&w.f_addr, 2),
        (&w.f_prefix, 3),
    ] {
        let m = model.clone();
        let mut inp = inp.clone();
        inp.set_trigger(CallbackTrigger::Changed);
        inp.set_callback(move |i| model_set(&m, id, field, &i.value()));
    }

    // 删除：delete_widget 是 FLTK 官方的安全延迟删除，鼠标按住时也不会崩
    {
        let m = model.clone();
        let pack = pack.clone();
        w.del_btn.set_callback(move |_| {
            let entry = {
                let mut m = m.borrow_mut();
                m.iter().position(|e| e.id == id).map(|pos| m.remove(pos))
            };
            let Some(entry) = entry else { return };
            app::delete_widget(entry.widget);
            // 剩余行上移
            {
                let m = m.borrow();
                for (idx, e) in m.iter().enumerate() {
                    let mut w = e.widget.clone();
                    w.set_pos(0, idx as i32 * (view::ROW_H + view::ROW_GAP));
                }
            }
            resize_pack(&pack, &m);
        });
    }

    model.borrow_mut().push(RuleRow {
        id,
        rule,
        widget: w.row,
        inputs: [w.m_addr, w.m_prefix, w.f_addr, w.f_prefix],
        arrow: w.arrow,
        del_btn: w.del_btn,
    });
}

/// field: 0=match_addr 1=match_prefix 2=fwd_addr 3=fwd_prefix
fn model_set(model: &Model, id: u64, field: usize, val: &str) {
    let mut m = model.borrow_mut();
    let Some(entry) = m.iter_mut().find(|e| e.id == id) else {
        return;
    };
    let rule = &mut entry.rule;
    match field {
        0 => rule.matcher.addr = val.to_string(),
        1 => rule.matcher.path_prefix = val.to_string(),
        2 => rule.forward.addr = val.to_string(),
        _ => rule.forward.path_prefix = val.to_string(),
    }
}

pub fn resize_pack(pack: &Pack, model: &Model) {
    let mut pack = pack.clone();
    pack.set_size(
        view::ROW_W,
        model.borrow().len() as i32 * (view::ROW_H + view::ROW_GAP),
    );
    // pack 缩小后，被删行残留像素在 pack 新边界外，需要全量重绘
    app::redraw();
}

/// 接线所有按钮回调
pub fn wire(
    v: &mut View,
    model: &Model,
    daemon_slot: &DaemonSlot,
    stats: &Arc<Stats>,
) {
    // 启停
    {
        let daemon_slot = daemon_slot.clone();
        let model = model.clone();
        let stats = stats.clone();
        let listen_input = v.listen_input.clone();
        let auto_proxy = v.auto_proxy.clone();
        let mut status = v.status.clone();
        let mut icon_win = v.win.clone();
        let mut titlebar_icon = v.icon.clone();
        // 任务栏图标 + 界面左上角图标随状态切换：运行中 = 换色版，停止 = 原图标
        let set_icon = |win: &mut fltk::window::Window, frame: &mut fltk::frame::Frame, running: bool| {
            let data: &[u8] = if running {
                include_bytes!("../../assets/app-icon-running-64.png")
            } else {
                include_bytes!("../../assets/app-icon-64.png")
            };
            if let Ok(img) = fltk::image::PngImage::from_data(data) {
                win.set_icon(Some(img));
            }
            if let Ok(mut img) = fltk::image::PngImage::from_data(data) {
                img.scale(20, 20, true, true);
                frame.set_image(Some(img));
                frame.redraw();
            }
        };
        v.start_btn.set_callback(move |b| {
            let mut slot = daemon_slot.borrow_mut();
            if let Some((tx, _)) = slot.take() {
                let _ = tx.send(Command::Shutdown);
                if auto_proxy.is_checked() {
                    match sysproxy::disable() {
                        Ok(()) => tracing::info!("{}", t!("log.sys_proxy_off")),
                        Err(e) => tracing::error!("{}", t!("log.sys_proxy_off_fail", e = e.to_string())),
                    }
                }
                status.set_label(&t!("status_stopped"));
                status.set_label_color(view::current_pal().subtext);
                b.set_label(&t!("start"));
                set_icon(&mut icon_win, &mut titlebar_icon, false);
            } else {
                let config = AppConfig {
                    listen_addr: listen_input.value(),
                    rules: rules_of(&model),
                    ..Default::default()
                };
                let addr = config.listen_addr.clone();
                *slot = Some((daemon::spawn(config, stats.clone()), addr.clone()));
                if auto_proxy.is_checked() {
                    match sysproxy::enable(&addr) {
                        Ok(()) => tracing::info!("{}", t!("log.sys_proxy_on", addr = addr)),
                        Err(e) => tracing::error!("{}", t!("log.sys_proxy_on_fail", e = e.to_string())),
                    }
                }
                status.set_label(&t!("status_running"));
                status.set_label_color(view::current_pal().ok);
                b.set_label(&t!("stop"));
                set_icon(&mut icon_win, &mut titlebar_icon, true);
            }
        });
    }

    // 系统代理复选框：状态持久化
    {
        let auto_proxy = v.auto_proxy.clone();
        v.auto_proxy.set_callback(move |_| {
            let mut config = AppConfig::init().unwrap_or_default();
            config.auto_proxy = auto_proxy.is_checked();
            if let Err(e) = config.save() {
                tracing::error!("{}", t!("log.save_pref_fail", e = e.to_string()));
            }
        });
    }

    // 添加规则
    {
        let model = model.clone();
        let pack = v.rows_pack.clone();
        v.add_btn.set_callback(move |_| {
            add_rule(
                &pack,
                &model,
                Rule {
                    matcher: Host {
                        addr: String::new(),
                        path_prefix: "/".to_string(),
                    },
                    forward: Host {
                        addr: String::new(),
                        path_prefix: String::new(),
                    },
                },
            );
            resize_pack(&pack, &model);
        });
    }

    // 保存并生效
    {
        let model = model.clone();
        let daemon_slot = daemon_slot.clone();
        let listen_input = v.listen_input.clone();
        let auto_proxy = v.auto_proxy.clone();
        let stats = stats.clone();
        v.save_btn.set_callback(move |_| {
            let mut config = AppConfig::init().unwrap_or_default();
            config.listen_addr = listen_input.value();
            config.rules = rules_of(&model);
            config.auto_proxy = auto_proxy.is_checked();
            if let Err(e) = config.save() {
                tracing::error!("{}", t!("log.save_fail", e = e.to_string()));
                return;
            }
            tracing::info!("{}", t!("log.saved"));
            let mut slot = daemon_slot.borrow_mut();
            if let Some((tx, addr)) = slot.as_mut() {
                if *addr == config.listen_addr {
                    let _ = tx.send(Command::SetRules(config.rules));
                } else {
                    tracing::info!("{}", t!("log.addr_changed_restart"));
                    let _ = tx.send(Command::Shutdown);
                    *slot = Some((
                        daemon::spawn(config.clone(), stats.clone()),
                        config.listen_addr.clone(),
                    ));
                }
            }
        });
    }

    // 关窗（✕ 按钮 / 任务栏右键关闭）：恢复系统代理后退出
    {
        let cleanup: Rc<dyn Fn()> = Rc::new({
            let daemon_slot = daemon_slot.clone();
            let auto_proxy = v.auto_proxy.clone();
            move || {
                let running = daemon_slot.borrow().is_some();
                if running && auto_proxy.is_checked() {
                    match sysproxy::disable() {
                        Ok(()) => tracing::info!("{}", t!("log.sys_proxy_off")),
                        Err(e) => tracing::error!("{}", t!("log.sys_proxy_off_fail", e = e.to_string())),
                    }
                }
            }
        });
        let c = cleanup.clone();
        v.close_btn.set_callback(move |_| {
            c();
            app::quit();
        });
        v.win.set_callback(move |_| {
            cleanup();
            app::quit();
        });
    }

    // 主题分段控件：立即切换 + 持久化
    for idx in 0..v.theme_btns.len() {
        let v2 = v.clone_ref();
        let model = model.clone();
        let daemon_slot = daemon_slot.clone();
        v.theme_btns[idx].set_callback(move |_| {
            let theme = ["system", "dark", "light"][idx];
            view::set_theme_sel(idx);
            let dark = match theme {
                "dark" => true,
                "light" => false,
                _ => matches!(dark_light::detect(), Ok(dark_light::Mode::Dark)),
            };
            apply_theme(&v2, &model, &daemon_slot, dark);
            let mut config = AppConfig::init().unwrap_or_default();
            if config.theme != theme {
                config.theme = theme.to_string();
                if let Err(e) = config.save() {
                    tracing::error!("{}", t!("log.save_theme_fail", e = e.to_string()));
                }
            }
            tracing::info!("{}", t!("log.theme_switched"));
        });
    }

    // 语言切换：zh↔en 互切，存配置，全量重刷文案
    {
        let model = model.clone();
        let daemon_slot = daemon_slot.clone();
        let stats = stats.clone();
        let v2 = v.clone_ref();
        v.lang_btn.set_callback(move |_| {
            let next = if &*rust_i18n::locale() == "zh" { "en" } else { "zh" };
            set_locale(next);
            let running = daemon_slot.borrow().is_some();
            refresh_i18n(&v2, &model, running, &stats.snapshot());
            let mut config = AppConfig::init().unwrap_or_default();
            if config.language != next {
                config.language = next.to_string();
                if let Err(e) = config.save() {
                    tracing::error!("{}", t!("log.save_lang_fail", e = e.to_string()));
                }
            }
        });
    }
}

/// 输入框光标闪烁（FLTK 光标设计上不闪，自己实现）
pub fn start_cursor_blink(model: Model, listen_input: Input) {
    let mut visible = true;
    app::add_timeout3(0.53, move |h| {
        let pal = view::current_pal();
        visible = !visible;
        let m = model.borrow();
        let fp = app::focus().map(|w| w.as_widget_ptr() as usize).unwrap_or(0);
        // 只触摸当前仍在模型里的输入框，删除后的行天然跳过，无悬垂指针
        let inputs = m
            .iter()
            .flat_map(|e| e.inputs.iter().cloned())
            .chain([listen_input.clone()]);
        for mut inp in inputs {
            let focused = inp.as_widget_ptr() as usize == fp;
            let want = if focused && !visible { pal.input_bg } else { pal.text };
            if inp.cursor_color() != want {
                inp.set_cursor_color(want);
                inp.redraw();
            }
        }
        app::repeat_timeout3(0.53, h);
    });
}

/// 系统代理设置
pub mod sysproxy {
    /// 启动时设置系统 SOCKS5 代理
    pub fn enable(addr: &str) -> anyhow::Result<()> {
        set(addr, true)
    }

    /// 停止时取消系统代理
    pub fn disable() -> anyhow::Result<()> {
        set("", false)
    }

    /// 程序启动时调用一次：清掉上次意外退出残留的系统代理
    /// （能确认指纹是我们写的那套才动手；正常退出由 disable 恢复原配置）
    pub fn cleanup_leftover() {
        #[cfg(target_os = "windows")]
        if let Err(e) = cleanup_leftover_win() {
            tracing::warn!("清理残留系统代理失败: {e}");
        }
        // macOS/Linux 无独立指纹可判（用户可能真的在用回环 socks 代理），
        // 依赖 set() 启用时对即将写入地址的精确比对，不在启动时盲清
    }

    #[cfg(target_os = "windows")]
    fn cleanup_leftover_win() -> anyhow::Result<()> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let run = |args: &[&str]| {
            std::process::Command::new("reg")
                .args(args)
                .creation_flags(CREATE_NO_WINDOW)
                .output()
        };
        let mut content = String::new();
        for name in ["ProxyEnable", "ProxyServer", "ProxyOverride"] {
            let out = run(&["query", KEY, "/v", name])?;
            if out.status.success() {
                if let Some(v) = read_value(&out, name) {
                    content.push_str(&format!("{name}={v}\n"));
                    continue;
                }
            }
            content.push_str(&format!("{name}=\n"));
        }
        if is_self_leftover(&content) {
            tracing::info!("检测到上次退出残留的系统代理，直接清除");
            // BACKUP 为空 → set 走“关闭代理”分支并通知系统刷新
            set("", false)?;
        }
        Ok(())
    }

/// 原代理配置备份（进程被杀才会丢；残留场景由 is_self_leftover 兜底识别，见 set）
/// 内容格式各平台自定义（key=value 行）
static BACKUP: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

#[cfg(target_os = "windows")]
    const KEY: &str = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings";

    /// 我们写入的代理豁免串，也是识别自残留的指纹之一
    #[cfg(target_os = "windows")]
    pub(super) const OVERRIDE: &str = "localhost;127.*;<local>";

    #[cfg(target_os = "windows")]
    fn set(addr: &str, on: bool) -> anyhow::Result<()> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let run = |args: &[&str]| {
            std::process::Command::new("reg")
                .args(args)
                .creation_flags(CREATE_NO_WINDOW)
                .output()
        };

        if on {
            // 备份原代理配置到内存（用户可能本来就有代理，如 Clash）
            backup(&run)?;
            // 上次意外退出没清掉的残留就是我们自己写的那套：不是“原配置”，
            // 不备份，停止时直接关闭代理，避免把死代理“还原”回去
            let stale = BACKUP.lock().unwrap().as_deref().is_some_and(is_self_leftover);
            if stale {
                *BACKUP.lock().unwrap() = None;
                tracing::info!("检测到上次退出残留的系统代理，停止时将直接清除");
            }
            run(&["add", KEY, "/v", "ProxyEnable", "/t", "REG_DWORD", "/d", "1", "/f"])?;
            // HTTP 代理（daemon 同时支持 SOCKS5 和 HTTP 代理协议）
            run(&["add", KEY, "/v", "ProxyServer", "/t", "REG_SZ", "/d", &addr, "/f"])?;
            // 只豁免本机回环；内网网段（如 192.168.*）恰恰是调试目标，不能豁免
            run(&["add", KEY, "/v", "ProxyOverride", "/t", "REG_SZ", "/d", OVERRIDE, "/f"])?;
        } else if BACKUP.lock().unwrap().is_some() {
            // 恢复备份的原配置
            restore(&run)?;
        } else {
            run(&["add", KEY, "/v", "ProxyEnable", "/t", "REG_DWORD", "/d", "0", "/f"])?;
        }

        // 通知运行中的程序（浏览器等）代理设置已变
        #[link(name = "wininet")]
        unsafe extern "C" {
            fn InternetSetOptionW(h: isize, opt: u32, buf: *const u8, len: u32) -> i32;
        }
        unsafe {
            InternetSetOptionW(0, 39, std::ptr::null(), 0); // SETTINGS_CHANGED
            InternetSetOptionW(0, 37, std::ptr::null(), 0); // REFRESH
        }
        Ok(())
    }

    /// 读一个注册表值；不存在返回 None
    #[cfg(target_os = "windows")]
    fn read_value(out: &std::process::Output, name: &str) -> Option<String> {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix(name) {
                let rest = rest.trim_start();
                if let Some((_, value)) = rest.split_once(|c: char| c.is_whitespace()) {
                    // REG_SZ    value / REG_DWORD    0x1
                    let value = value.trim_start();
                    let value = value.strip_prefix("0x").unwrap_or(value);
                    return Some(value.to_string());
                }
            }
        }
        None
    }

    #[cfg(target_os = "windows")]
    fn backup(run: &dyn Fn(&[&str]) -> std::io::Result<std::process::Output>) -> anyhow::Result<()> {
        let mut content = String::new();
        for name in ["ProxyEnable", "ProxyServer", "ProxyOverride"] {
            let out = run(&["query", KEY, "/v", name])?;
            if out.status.success() {
                if let Some(v) = read_value(&out, name) {
                    content.push_str(&format!("{name}={v}\n"));
                    continue;
                }
            }
            content.push_str(&format!("{name}=\n")); // 原本不存在
        }
        *BACKUP.lock().unwrap() = Some(content);
        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn restore(run: &dyn Fn(&[&str]) -> std::io::Result<std::process::Output>) -> anyhow::Result<()> {
        let content = BACKUP.lock().unwrap().take().unwrap_or_default();
        for line in content.lines() {
            let Some((name, value)) = line.split_once('=') else { continue };
            if value.is_empty() {
                // 原本不存在：删掉我们写入的值
                let _ = run(&["delete", KEY, "/v", name, "/f"]);
            } else if name == "ProxyEnable" {
                run(&["add", KEY, "/v", name, "/t", "REG_DWORD", "/d", value, "/f"])?;
            } else {
                run(&["add", KEY, "/v", name, "/t", "REG_SZ", "/d", value, "/f"])?;
            }
        }
        Ok(())
    }

    /// 备份内容是否恰是本程序上次意外退出留下的残留（自己写的那套）
    /// 指纹：ProxyEnable=1 + ProxyOverride 是我们的豁免串 + ProxyServer 指向本机回环
    /// （用户真实代理恰好也是回环地址且豁免串完全相同才误判，代价只是停用一次，可重开）
    #[cfg(target_os = "windows")]
    pub(super) fn is_self_leftover(content: &str) -> bool {
        let (mut enable, mut server, mut over) = (String::new(), String::new(), String::new());
        for line in content.lines() {
            if let Some((k, v)) = line.split_once('=') {
                match k {
                    "ProxyEnable" => enable = v.to_string(),
                    "ProxyServer" => server = v.to_string(),
                    "ProxyOverride" => over = v.to_string(),
                    _ => {}
                }
            }
        }
        enable == "1"
            && over == OVERRIDE
            && (server.starts_with("127.0.0.1:") || server.starts_with("localhost:"))
    }

    #[cfg(target_os = "macos")]
    fn set(addr: &str, on: bool) -> anyhow::Result<()> {
        let sh = |args: &[&str]| std::process::Command::new("networksetup").args(args).output();
        let get = |args: &[&str]| -> anyhow::Result<String> {
            let out = sh(args)?;
            Ok(String::from_utf8_lossy(&out.stdout).to_string())
        };
        if on {
            let (host, port) = addr.rsplit_once(':').unwrap_or((addr, "1080"));
            // 备份当前 socks 代理状态
            let info = get(&["-getsocksfirewallproxy", "Wi-Fi"])?;
            let mut b = String::new();
            for line in info.lines() {
                if let Some((k, v)) = line.split_once(':') {
                    let k = k.trim();
                    if matches!(k, "Enabled" | "Server" | "Port") {
                        b.push_str(&format!("{}={}\n", k.to_lowercase(), v.trim()));
                    }
                }
            }
            // 上次意外退出留下的残留（正是本程序写的 server/port）不当原配置，停止时直接 off
            let server_line = format!("server={host}\n");
            let port_line = format!("port={port}\n");
            let stale = b.contains("enabled=Yes\n")
                && b.contains(server_line.as_str())
                && b.contains(port_line.as_str());
            *BACKUP.lock().unwrap() = if stale { None } else { Some(b) };

            sh(&["-setsocksfirewallproxy", "Wi-Fi", host, port])?;
            sh(&["-setsocksfirewallproxystate", "Wi-Fi", "on"])?;
        } else if let Some(b) = BACKUP.lock().unwrap().take() {
            // 恢复原配置
            let mut enabled = "off".to_string();
            let mut server = String::new();
            let mut port = String::new();
            for line in b.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    match k {
                        "enabled" => enabled = if v == "Yes" { "on".into() } else { "off".into() },
                        "server" => server = v.to_string(),
                        "port" => port = v.to_string(),
                        _ => {}
                    }
                }
            }
            if enabled == "on" && !server.is_empty() {
                sh(&["-setsocksfirewallproxy", "Wi-Fi", &server, &port])?;
            }
            sh(&["-setsocksfirewallproxystate", "Wi-Fi", &enabled])?;
        } else {
            sh(&["-setsocksfirewallproxystate", "Wi-Fi", "off"])?;
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn set(addr: &str, on: bool) -> anyhow::Result<()> {
        let sh = |args: &[&str]| std::process::Command::new("gsettings").args(args).output();
        let get = |schema: &str, key: &str| -> String {
            sh(&["get", schema, key])
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_matches('\'').to_string())
                .unwrap_or_default()
        };
        if on {
            let (host, port) = addr.rsplit_once(':').unwrap_or((addr, "1080"));
            // 备份当前 GNOME 代理配置
            let mode = get("org.gnome.system.proxy", "mode");
            let b = format!(
                "mode={}\nhost={}\nport={}\n",
                mode,
                get("org.gnome.system.proxy.socks", "host"),
                get("org.gnome.system.proxy.socks", "port"),
            );
            // 上次意外退出留下的残留（正是本程序写的 host/port）不当原配置，停止时直接 none
            let host_line = format!("host={host}\n");
            let port_line = format!("port={port}\n");
            let stale = mode == "manual"
                && b.contains(host_line.as_str())
                && b.contains(port_line.as_str());
            *BACKUP.lock().unwrap() = if stale { None } else { Some(b) };

            sh(&["set", "org.gnome.system.proxy.socks", "host", host])?;
            sh(&["set", "org.gnome.system.proxy.socks", "port", port])?;
            sh(&["set", "org.gnome.system.proxy", "mode", "manual"])?;
        } else if let Some(b) = BACKUP.lock().unwrap().take() {
            // 恢复原配置
            let mut mode = "none".to_string();
            let mut host = String::new();
            let mut port = String::new();
            for line in b.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    match k {
                        "mode" => mode = v.to_string(),
                        "host" => host = v.to_string(),
                        "port" => port = v.to_string(),
                        _ => {}
                    }
                }
            }
            if !host.is_empty() {
                sh(&["set", "org.gnome.system.proxy.socks", "host", &host])?;
                sh(&["set", "org.gnome.system.proxy.socks", "port", &port])?;
            }
            sh(&["set", "org.gnome.system.proxy", "mode", &mode])?;
        } else {
            sh(&["set", "org.gnome.system.proxy", "mode", "none"])?;
        }
        Ok(())
    }
}

/// 运行中切换主题：全量重刷所有控件配色
pub fn apply_theme(v: &View, model: &Model, daemon_slot: &DaemonSlot, dark: bool) {
    // 先重刷 fltk-theme 的全局色（勾选框底色用 FL_BACKGROUND2_COLOR）
    fltk_theme::WidgetTheme::new(if dark {
        fltk_theme::ThemeType::Dark
    } else {
        fltk_theme::ThemeType::Greybird
    })
    .apply();
    let pal = view::palette(dark);
    view::set_current_pal(pal);
    let sel = view::current_theme_sel();

    v.win.clone().set_color(pal.border);
    v.col.clone().set_color(pal.bg);
    v.title_bar.clone().set_color(pal.bg);
    v.title.clone().set_label_color(pal.text);
    for c in [&v.header, &v.stats_card, &v.rules_card, &v.log_card] {
        c.clone().set_color(pal.card);
    }
    v.addr_label.clone().set_label_color(pal.subtext);
    view::style_input(&mut v.listen_input.clone(), pal);
    view::style_primary_btn(&mut v.start_btn.clone(), pal);
    // 状态灯颜色跟随运行状态
    let running = daemon_slot.borrow().is_some();
    v.status
        .clone()
        .set_label_color(if running { pal.ok } else { pal.subtext });
    {
        let mut ap = v.auto_proxy.clone();
        ap.set_label_color(pal.text);
        ap.set_color(pal.input_bg);
        ap.set_selection_color(pal.accent);
    }
    // 标题栏按钮恢复常态色（hover 配色事件里现读 current_pal，不用管）
    for b in [&v.min_btn, &v.close_btn] {
        let mut b = b.clone();
        b.set_color(pal.bg);
        b.set_label_color(pal.subtext);
        b.redraw();
    }
    // 主题分段控件
    v.seg.clone().set_color(pal.input_bg);
    view::refresh_theme_btns(&mut v.theme_btns.clone(), sel, pal);
    // 统计栏
    for f in &v.stats {
        f.clone().set_label_color(pal.subtext);
    }
    // 规则区
    v.rule_cap.clone().set_label_color(pal.text);
    for f in &v.col_header_frames {
        f.clone().set_label_color(pal.subtext);
    }
    let mut scroll = v.scroll.clone();
    scroll.set_color(pal.card);
    for mut sb in [scroll.scrollbar(), scroll.hscrollbar()] {
        sb.set_color(pal.card);
        sb.set_selection_color(pal.border);
        sb.set_label_color(pal.border);
    }
    view::style_ghost_btn(&mut v.add_btn.clone(), pal);
    view::style_primary_btn(&mut v.save_btn.clone(), pal);
    view::style_ghost_btn(&mut v.lang_btn.clone(), pal);
    // 规则行
    for e in model.borrow().iter() {
        e.widget.clone().set_color(pal.card);
        for inp in &e.inputs {
            view::style_input(&mut inp.clone(), pal);
        }
        e.arrow.clone().set_label_color(pal.subtext);
        let mut d = e.del_btn.clone();
        d.set_color(pal.input_bg);
        d.set_label_color(pal.danger);
    }
    // 日志区
    v.log_cap.clone().set_label_color(pal.text);
    let mut ld = v.log_disp.clone();
    ld.set_color(pal.input_bg);
    ld.set_text_color(pal.text);
    app::redraw();
}

/// 运行中切换语言：重刷所有静态文案 + 当前动态文案
fn refresh_i18n(v: &View, model: &Model, running: bool, s: &crate::core::stats::StatsSnapshot) {
    v.win.clone().set_label(&t!("app_title"));
    v.title.clone().set_label(&t!("app_title"));
    v.addr_label.clone().set_label(&t!("listen_addr"));
    v.start_btn.clone().set_label(&t!(if running { "stop" } else { "start" }));
    v.status
        .clone()
        .set_label(&t!(if running { "status_running" } else { "status_stopped" }));
    v.auto_proxy.clone().set_label(&t!("sys_proxy"));
    for (b, key) in v.theme_btns.iter().zip(["theme_system", "theme_dark", "theme_light"]) {
        b.clone().set_label(&t!(key));
    }
    view::refresh_theme_btns(&mut v.theme_btns.clone(), view::current_theme_sel(), view::current_pal());
    v.lang_btn.clone().set_label(&t!("lang_switch"));
    let stats = [(&v.stats[0], "stats_active", format!("{}", s.active)), (&v.stats[1], "stats_total", format!("{}", s.total))];
    for (f, key, val) in stats {
        f.clone().set_label(&format!("{}  {}", t!(key), val));
    }
    v.stats[2].clone().set_label(&format!("{}  {}", t!("stats_up"), fmt_bytes(s.bytes_up)));
    v.stats[3].clone().set_label(&format!("{}  {}", t!("stats_down"), fmt_bytes(s.bytes_down)));
    v.rule_cap.clone().set_label(&t!("rules_title"));
    for (f, key) in v.col_header_frames.iter().zip([
        "col_match_addr",
        "col_match_prefix",
        "",
        "col_fwd_addr",
        "col_fwd_prefix",
    ]) {
        if !key.is_empty() {
            f.clone().set_label(&t!(key));
        }
    }
    v.add_btn.clone().set_label(&t!("add_rule"));
    v.save_btn.clone().set_label(&t!("save_apply"));
    v.log_cap.clone().set_label(&t!("log_title"));
    for e in model.borrow().iter() {
        e.del_btn.clone().set_label(&t!("rule_delete"));
    }
    app::redraw();
}

/// 统计栏每秒刷新
pub fn start_stats_timer(stats: Arc<Stats>, frames: [Frame; 4]) {
    let [mut active, mut total, mut up, mut down] = frames;
    app::add_timeout3(1.0, move |h| {
        let s = stats.snapshot();
        active.set_label(&format!("{}  {}", t!("stats_active"), s.active));
        total.set_label(&format!("{}  {}", t!("stats_total"), s.total));
        up.set_label(&format!("{}  {}", t!("stats_up"), fmt_bytes(s.bytes_up)));
        down.set_label(&format!("{}  {}", t!("stats_down"), fmt_bytes(s.bytes_down)));
        app::repeat_timeout3(1.0, h);
    });
}

fn fmt_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if n >= GB {
        format!("{:.1} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.1} KB", n as f64 / KB as f64)
    } else {
        format!("{n} B")
    }
}

#[cfg(all(test, windows))]
mod sysproxy_tests {
    use super::sysproxy;

    // 会真实修改系统代理，默认不跑：cargo test -- --ignored sysproxy
    #[test]
    #[cfg(target_os = "windows")]
    fn self_leftover_detection() {
        let leftover = |server: &str| {
            format!("ProxyEnable=1\nProxyServer={server}\nProxyOverride={}\n", sysproxy::OVERRIDE)
        };
        // 上次意外退出留下的：指向我们监听的回环地址 + 我们的豁免串
        assert!(sysproxy::is_self_leftover(&leftover("127.0.0.1:1080")));
        assert!(sysproxy::is_self_leftover(&leftover("localhost:1080")));
        // 用户真实代理：非回环地址
        assert!(!sysproxy::is_self_leftover(&leftover("192.168.120.177:7890")));
        // 用户真实代理：回环但豁免串不同（如 Clash）
        assert!(!sysproxy::is_self_leftover("ProxyEnable=1\nProxyServer=127.0.0.1:7890\nProxyOverride=<local>\n"));
        // 原本没开代理
        assert!(!sysproxy::is_self_leftover("ProxyEnable=0\nProxyServer=\nProxyOverride=\n"));
    }

    #[test]
    #[ignore = "真实修改系统代理"]
    fn backup_restore_roundtrip() {
        // 启用前原值
        let before = reg_query("ProxyServer");
        sysproxy::enable("127.0.0.1:9999").unwrap();
        assert_eq!(reg_query("ProxyServer").as_deref(), Some("127.0.0.1:9999"));
        assert_eq!(reg_query("ProxyEnable").as_deref(), Some("1"));
        sysproxy::disable().unwrap();
        assert_eq!(reg_query("ProxyServer"), before);
    }

    fn reg_query(name: &str) -> Option<String> {
        let out = std::process::Command::new("reg")
            .args(["query", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings", "/v", name])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        text.lines()
            .find(|l| l.trim_start().starts_with(name))
            .and_then(|l| l.split_whitespace().last())
            .map(|s| s.strip_prefix("0x").unwrap_or(s).to_string())
    }
}
