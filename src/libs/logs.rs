use crate::libs::{APP_NAME, TIME_MILLISECOND_FORMAT};
use std::fs;
use std::sync::LazyLock;

/// UI 日志接收端
pub type UiSink = Box<dyn Fn(String) + Send + Sync>;
use tracing::Level;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use tracing_subscriber::util::SubscriberInitExt;

/// 时间格式只解析一次
static TS_FORMAT: LazyLock<Vec<time::format_description::FormatItem<'static>>> =
    LazyLock::new(|| time::format_description::parse(TIME_MILLISECOND_FORMAT).unwrap());

/// 初始化日志，ui_sink 存在时同时把日志行推到 GUI
pub fn init(log_file: String, level: LevelFilter, ui_sink: Option<UiSink>) -> anyhow::Result<()> {
    let current_dir = crate::libs::app_dir();
    let logs_dir = current_dir + "/logs/";
    fs::create_dir_all(logs_dir.clone())?;

    hook_panic_handler(logs_dir.clone(), log_file.clone());
    init_tracing(logs_dir, log_file, level, ui_sink);
    Ok(())
}

pub fn init_default() -> anyhow::Result<()> {
    init(APP_NAME.to_owned(), LevelFilter::DEBUG, None)
}

pub fn init_default_with_ui(sink: impl Fn(String) + Send + Sync + 'static) -> anyhow::Result<()> {
    init(APP_NAME.to_owned(), LevelFilter::DEBUG, Some(Box::new(sink)))
}

/// 把 tracing 事件格式化成单行推到 UI 线程
struct UiLayer {
    sink: UiSink,
}

impl<S: tracing::Subscriber> Layer<S> for UiLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut msg = String::new();
        event.record(&mut MsgVisitor(&mut msg));
        let ts = time::OffsetDateTime::now_utc()
            .to_offset(time::macros::offset!(+8))
            .format(&TS_FORMAT)
            .unwrap_or_default();
        (self.sink)(format!("{ts} {} {msg}", event.metadata().level()));
    }
}

struct MsgVisitor<'a>(&'a mut String);

impl tracing::field::Visit for MsgVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            use std::fmt::Write;
            let _ = write!(self.0, "{value:?}");
        }
    }
}

/// 拦截panic处理，保存panic信息到panic日志中
///
/// # Arguments
///
/// * `logs_dir`: 日志保存位置
/// * `app_name`: 应用名称
fn hook_panic_handler(logs_dir: String, app_name: String) {
    use std::backtrace;
    use std::fs::OpenOptions;
    use std::io::Write;
    use time::OffsetDateTime;
    use time::macros::offset;

    std::panic::set_hook(Box::new(move |info| {
        let backtrace = backtrace::Backtrace::force_capture();
        let payload = info.payload();
        let payload_str: Option<&str> = if let Some(s) = payload.downcast_ref::<&str>() {
            Some(s)
        } else if let Some(s) = payload.downcast_ref::<String>() {
            Some(s)
        } else {
            None
        };

        if let Some(payload_str) = payload_str {
            println!(
                "panic occurred: payload:{}, location: {:?}",
                payload_str,
                info.location()
            );
        } else {
            println!("panic occurred: location: {:?}", info.location());
        }

        let current_time = OffsetDateTime::now_utc()
            .to_offset(offset!(+8))
            .format(&TS_FORMAT)
            .unwrap_or_else(|e| {
                println!("get current time error: {:?}", e);
                "".to_string()
            });

        let _ = OpenOptions::new()
            .append(true)
            .create(true) // 如果文件不存在，则创建文件
            .open(format!("{}{}.panic.log", logs_dir, app_name))
            .and_then(|mut f| {
                f.write_all(format!("{} {:?}\n{:#?}\n", current_time, info, backtrace).as_bytes())
            });
        println!("panic backtrace saved");
        // 注意: 不 exit, tokio task 的 panic 是隔离的, 单个连接出错不应杀掉整个代理进程
    }));
}

fn init_tracing(logs_dir: String, log_file: String, level: LevelFilter, ui_sink: Option<UiSink>) {
    let tracing_level = level.into_level().unwrap();
    let timer = || {
        tracing_subscriber::fmt::time::OffsetTime::new(
            time::macros::offset!(+8),
            TS_FORMAT.clone(),
        )
    };
    let ui_layer = ui_sink.map(|sink| UiLayer { sink });
    // 三方库降噪：https 上游会让 rustls 每个请求刷一串 DEBUG 握手日志，把界面日志淹掉
    let quiet = tracing_subscriber::filter::Targets::new()
        .with_default(LevelFilter::from_level(tracing_level))
        .with_target("rustls", Level::WARN)
        .with_target("tokio_rustls", Level::WARN)
        .with_target("hyper", Level::WARN)
        .with_target("h2", Level::WARN);

    if cfg!(debug_assertions) {
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_level(true)
            .with_target(false)
            .with_file(true)
            .with_line_number(true)
            .with_thread_names(true)
            .with_thread_ids(true)
            .with_timer(timer())
            .with_ansi(false)
            //调试模式输出到控制台, ERROR 及以上走 stderr, 其他走 stdout
            .with_writer(
                std::io::stdout
                    .with_filter(|meta| meta.level() > &Level::ERROR)
                    .or_else(std::io::stderr),
            )
            .with_filter(LevelFilter::from_level(tracing_level));
        tracing_subscriber::registry()
            .with(quiet)
            .with(fmt_layer)
            .with(ui_layer)
            .init();
    } else {
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_level(true)
            .with_target(false)
            .with_file(true)
            .with_line_number(true)
            .with_thread_names(true)
            .with_thread_ids(true)
            .with_timer(timer())
            .with_ansi(false)
            //非调试模式输出到日志文件
            .with_writer(
                tracing_appender::rolling::Builder::new()
                    .filename_prefix(log_file.clone())
                    .filename_suffix("log")
                    .max_log_files(7)
                    .rotation(tracing_appender::rolling::Rotation::DAILY)
                    .build(logs_dir.clone())
                    .unwrap(),
            )
            .with_filter(LevelFilter::from_level(tracing_level));
        tracing_subscriber::registry()
            .with(quiet)
            .with(fmt_layer)
            .with(ui_layer)
            .init();
    }
}
