// GUI 程序不弹控制台黑窗口；--headless 时手动挂回父控制台保留日志输出
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod core;
mod daemon;
mod libs;
mod ui;

rust_i18n::i18n!("locales", fallback = "zh");

/// headless 模式下挂到父控制台，让日志能输出到终端
#[cfg(target_os = "windows")]
fn attach_parent_console() {
    #[link(name = "kernel32")]
    unsafe extern "C" {
        fn AttachConsole(pid: u32) -> i32;
        fn GetStdHandle(std: u32) -> isize;
        fn SetStdHandle(std: u32, handle: isize) -> i32;
        fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *const u8, disp: u32, attr: u32, tmpl: isize) -> isize;
    }
    const ATTACH_PARENT: u32 = 0xFFFF_FFFF;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const OPEN_EXISTING: u32 = 3;
    unsafe {
        // stdout 已被重定向（管道/文件）时不处理，保证 > log.txt 可用
        let existing = GetStdHandle(STD_OUTPUT_HANDLE);
        if existing != 0 && existing != -1 {
            return;
        }
        if AttachConsole(ATTACH_PARENT) != 0 {
            let name: Vec<u16> = "CONOUT$".encode_utf16().chain([0]).collect();
            let h = CreateFileW(name.as_ptr(), GENERIC_WRITE, 3, std::ptr::null(), OPEN_EXISTING, 0, 0);
            if h != 0 && h != -1 {
                SetStdHandle(STD_OUTPUT_HANDLE, h);
                SetStdHandle(STD_ERROR_HANDLE, h);
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn attach_parent_console() {}

fn main() -> anyhow::Result<()> {
    // 语言：两种模式统一在入口初始化（读取失败回退 auto → 系统语言）
    let lang = core::config::AppConfig::init()
        .map(|c| c.language)
        .unwrap_or_else(|_| "auto".into());
    rust_i18n::set_locale(core::config::resolve_locale(&lang));

    // 上次意外退出可能残留了系统代理（备份的是死代理），启动即识别并清掉
    ui::controller::sysproxy::cleanup_leftover();

    if std::env::args().any(|a| a == "--headless") {
        attach_parent_console();
        libs::logs::init_default()?;
        let config = core::config::AppConfig::init()?;
        let stats = core::stats::Stats::shared();
        tokio::runtime::Runtime::new()?.block_on(async move {
            let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
            tokio::select! {
                r = daemon::run(config, rx, stats) => r?,
                _ = tokio::signal::ctrl_c() => {}
            }
            Ok(())
        })
    } else {
        ui::run()
    }
}
