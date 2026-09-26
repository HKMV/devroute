//! 纯 UI：配色、样式、控件构建。不含任何业务逻辑。

use crate::core::config::{AppConfig, Rule};
use rust_i18n::t;
use fltk::enums::{Align, Color, Event, FrameType};
use fltk::app;
use fltk::group::{Flex, Pack, PackType, Scroll};
use fltk::text::{TextBuffer, TextDisplay};
use fltk::{button::{Button, CheckButton}, frame::Frame, input::Input, prelude::*, window::Window};
use std::cell::Cell;

thread_local! {
    static CURRENT_PAL: Cell<Palette> = Cell::new(palette(false));
    static THEME_SEL: Cell<usize> = Cell::new(0);
}

/// 当前配色（事件回调里现读，支持运行中切换主题）
pub fn current_pal() -> Palette {
    CURRENT_PAL.with(|p| p.get())
}

pub fn set_current_pal(pal: Palette) {
    CURRENT_PAL.with(|p| p.set(pal));
}

pub fn current_theme_sel() -> usize {
    THEME_SEL.with(|s| s.get())
}

pub fn set_theme_sel(sel: usize) {
    THEME_SEL.with(|s| s.set(sel));
}

pub const ROW_H: i32 = 40;
pub const ROW_GAP: i32 = 8;
/// 行内容宽：边距4*2 + 列宽190+110+24+190+110+52 + 间距10*5
pub const ROW_CONTENT_W: i32 = 4 + 190 + 10 + 110 + 10 + 24 + 10 + 190 + 10 + 110 + 10 + 52 + 4;
/// Pack 宽度（略宽于行，留呼吸空间）
pub const ROW_W: i32 = ROW_CONTENT_W + 10;

/// 扁平化配色
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color,       // 窗口底色
    pub card: Color,     // 卡片
    pub text: Color,     // 主文本
    pub subtext: Color,  // 次要文本
    pub input_bg: Color, // 输入框
    pub accent: Color,   // 主按钮
    pub danger: Color,   // 删除
    pub ok: Color,       // 运行状态
    pub border: Color,   // 窗口边框
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: Color::from_hex(0x111827),
            card: Color::from_hex(0x1f2937),
            text: Color::from_hex(0xf9fafb),
            subtext: Color::from_hex(0x9ca3af),
            input_bg: Color::from_hex(0x374151),
            accent: Color::from_hex(0x3b82f6),
            danger: Color::from_hex(0xef4444),
            ok: Color::from_hex(0x34d399),
            border: Color::from_hex(0x374151),
        }
    } else {
        Palette {
            bg: Color::from_hex(0xf3f4f6),
            card: Color::from_hex(0xffffff),
            text: Color::from_hex(0x111827),
            subtext: Color::from_hex(0x6b7280),
            input_bg: Color::from_hex(0xf3f4f6),
            accent: Color::from_hex(0x2563eb),
            danger: Color::from_hex(0xdc2626),
            ok: Color::from_hex(0x059669),
            border: Color::from_hex(0xe5e7eb),
        }
    }
}

/// 全局圆角半径（RFlatBox 的绘制被重写为小圆角）
const CORNER_R: i32 = 6;

/// 自定义小圆角绘制，注册到 app::set_frame_type_cb
pub fn draw_flat_round(x: i32, y: i32, w: i32, h: i32, c: Color) {
    fltk::draw::set_draw_color(c);
    fltk::draw::draw_rounded_rectf(x, y, w, h, CORNER_R);
}

/// 微圆角（复选框等小控件用）：借用从未使用的 RShadowBox 框型
const CORNER_R_SMALL: i32 = 2;

pub fn draw_flat_round_small(x: i32, y: i32, w: i32, h: i32, c: Color) {
    fltk::draw::set_draw_color(c);
    fltk::draw::draw_rounded_rectf(x, y, w, h, CORNER_R_SMALL);
}



fn style_card<G: GroupExt>(g: &mut G, pal: Palette) {
    g.set_frame(FrameType::RFlatBox);
    g.set_color(pal.card);
}

pub(crate) fn style_input(inp: &mut Input, pal: Palette) {
    inp.set_frame(FrameType::RFlatBox);
    inp.set_color(pal.input_bg);
    inp.set_text_color(pal.text);
    inp.set_text_size(14);
}

pub(crate) fn style_primary_btn(b: &mut Button, pal: Palette) {
    b.set_frame(FrameType::RFlatBox);
    b.set_down_frame(FrameType::RFlatBox);
    b.set_color(pal.accent);
    b.set_label_color(Color::White);
    b.set_label_size(14);
    b.visible_focus(false); // 焦点虚线框丑，但全局关掉会导致输入框光标不闪
}

pub(crate) fn style_ghost_btn(b: &mut Button, pal: Palette) {
    b.set_frame(FrameType::RFlatBox);
    b.set_down_frame(FrameType::RFlatBox);
    b.set_color(pal.input_bg);
    b.set_label_color(pal.text);
    b.visible_focus(false);
}

fn caption(text: &str, pal: Palette) -> Frame {
    let mut f = Frame::default().with_label(text);
    f.set_label_color(pal.text);
    f.set_label_size(16);
    f.set_align(Align::Left | Align::Inside);
    f
}

/// 主窗口所有需要被逻辑层操作的控件句柄
pub struct View {
    pub win: Window,
    pub col: Flex,
    pub title_bar: Flex,
    pub icon: Frame,
    pub title: Frame,
    pub header: Flex,
    pub addr_label: Frame,
    pub listen_input: Input,
    pub start_btn: Button,
    pub status: Frame,
    pub auto_proxy: CheckButton,
    pub seg: Flex,
    pub theme_btns: Vec<Button>,
    pub lang_btn: Button,
    pub stats_card: Flex,
    pub stats: [Frame; 4],
    pub rules_card: Flex,
    pub rule_cap: Frame,
    pub col_header_frames: Vec<Frame>,
    pub scroll: Scroll,
    pub rows_pack: Pack,
    pub add_btn: Button,
    pub save_btn: Button,
    pub log_card: Flex,
    pub log_cap: Frame,
    pub log_disp: TextDisplay,
    pub log_buf: TextBuffer,
    pub close_btn: Button,
    pub min_btn: Button,
}

impl View {
    /// 控件句柄克隆（fltk 句柄是廉价引用），供回调里持有
    pub fn clone_ref(&self) -> View {
        View {
            win: self.win.clone(),
            col: self.col.clone(),
            title_bar: self.title_bar.clone(),
            icon: self.icon.clone(),
            title: self.title.clone(),
            header: self.header.clone(),
            addr_label: self.addr_label.clone(),
            listen_input: self.listen_input.clone(),
            start_btn: self.start_btn.clone(),
            status: self.status.clone(),
            auto_proxy: self.auto_proxy.clone(),
            seg: self.seg.clone(),
            theme_btns: self.theme_btns.clone(),
            lang_btn: self.lang_btn.clone(),
            stats_card: self.stats_card.clone(),
            stats: self.stats.clone(),
            rules_card: self.rules_card.clone(),
            rule_cap: self.rule_cap.clone(),
            col_header_frames: self.col_header_frames.clone(),
            scroll: self.scroll.clone(),
            rows_pack: self.rows_pack.clone(),
            add_btn: self.add_btn.clone(),
            save_btn: self.save_btn.clone(),
            log_card: self.log_card.clone(),
            log_cap: self.log_cap.clone(),
            log_disp: self.log_disp.clone(),
            log_buf: self.log_buf.clone(),
            close_btn: self.close_btn.clone(),
            min_btn: self.min_btn.clone(),
        }
    }
}

/// 主题初始选中项：0=跟随系统 1=深色 2=浅色
pub fn theme_index(config: &AppConfig) -> usize {
    match config.theme.as_str() {
        "dark" => 1,
        "light" => 2,
        _ => 0,
    }
}

/// 标题栏按钮：ghost 静止态 + hover 变色（重写 handle 实现，点击转发给 callback）
/// 配色每次事件现读 current_pal，主题切换后自动正确
fn chrome_btn(label: &str, danger_hover: bool) -> Button {
    let mut b = Button::default().with_label(label);
    b.set_frame(FrameType::RFlatBox);
    b.set_label_size(12);
    b.visible_focus(false);
    let pal = current_pal();
    b.set_color(pal.bg);
    b.set_label_color(pal.subtext);
    b.handle(move |b, ev| {
        let pal = current_pal();
        match ev {
            Event::Enter => {
                let (bg, fg) = if danger_hover {
                    (pal.danger, Color::White)
                } else {
                    (pal.border, pal.text)
                };
                b.set_color(bg);
                b.set_label_color(fg);
                b.redraw();
                true
            }
            Event::Leave | Event::Released => {
                if ev == Event::Released {
                    b.do_callback();
                    // 回调可能导致窗口最小化/关闭，Leave 事件不一定能到，主动恢复常态
                }
                b.set_color(pal.bg);
                b.set_label_color(pal.subtext);
                // 圆角绘制不覆盖四角像素，只重绘按钮会残留 hover 色的角，需要重绘父级背景
                if let Some(mut p) = b.parent() {
                    p.redraw();
                }
                true
            }
            Event::Push => true,
            _ => false,
        }
    });
    b
}

/// 无边框窗口的拖动条：按下记位置，拖动移动窗口
fn attach_drag<W: WidgetBase>(w: &mut W, win: &Window) {
    let mut win = win.clone();
    let mut last = (0, 0);
    w.handle(move |_, ev| match ev {
        Event::Push => {
            last = (app::event_x_root(), app::event_y_root());
            true
        }
        Event::Drag => {
            let (x, y) = (app::event_x_root(), app::event_y_root());
            win.set_pos(win.x() + x - last.0, win.y() + y - last.1);
            last = (x, y);
            true
        }
        _ => false,
    });
}

/// 构建整个窗口（不 show，由调用方决定时机）
pub fn build(config: &AppConfig, pal: Palette) -> View {
    let mut win = Window::default().with_size(800, 780).center_screen();
    win.set_label(&t!("app_title"));
    win.set_border(false); // 无边框
    // 窗口/任务栏图标（内嵌，无外部文件依赖）
    if let Ok(icon) = fltk::image::PngImage::from_data(include_bytes!("../../assets/app-icon-64.png")) {
        win.set_icon(Some(icon));
    }
    // 窗口底色当边框用，内容区内缩 1px 形成浅色边框
    win.set_color(pal.border);

    // ---- 标题栏：贴顶独立一行（无边框窗口：拖动移动 + 关闭） ----
    let mut title_bar = Flex::new(1, 1, 798, 32, None).row();
    title_bar.set_frame(FrameType::FlatBox);
    title_bar.set_color(pal.bg);
    title_bar.set_margin(2);
    // 标题栏图标（Frame 不会自动缩放，手动缩到 16px）
    let mut icon = Frame::default();
    if let Ok(mut img) = fltk::image::PngImage::from_data(include_bytes!("../../assets/app-icon-64.png")) {
        img.scale(20, 20, true, true);
        icon.set_image(Some(img));
        icon.set_frame(FrameType::NoBox);
    }
    title_bar.fixed(&icon, 26);
    let mut title = Frame::default().with_label(&t!("app_title"));
    title.set_label_size(16);
    title.set_label_color(pal.text);
    title.set_align(Align::Left | Align::Inside);
    let mut filler = Frame::default();
    // 最小化 / 关闭：ghost 风格，hover 才显色
    let mut min_btn = chrome_btn("—", false);
    title_bar.fixed(&min_btn, 32);
    let close_btn = chrome_btn("✕", true);
    title_bar.fixed(&close_btn, 32);
    title_bar.end();
    // 拖动挂在标题栏的每个非按钮控件上（FLTK 事件不向父级冒泡）
    attach_drag(&mut title_bar, &win);
    attach_drag(&mut title, &win);
    attach_drag(&mut filler, &win);
    {
        let mut w = win.clone();
        min_btn.set_callback(move |_| w.iconize());
    }
    // close_btn 的回调由 controller 接线（关窗前要清理系统代理）

    // ---- 内容区 ----
    let mut col = Flex::new(1, 33, 798, 746, None).column();
    col.set_frame(FrameType::FlatBox);
    col.set_color(pal.bg);
    col.set_margin(12);
    col.set_pad(10);

    // ---- 顶栏卡片 ----
    let mut header = Flex::default().row();
    style_card(&mut header, pal);
    header.set_pad(9);
    header.set_margin(6);

    let mut addr_label = Frame::default().with_label(&t!("listen_addr"));
    addr_label.set_label_color(pal.subtext);
    addr_label.set_align(Align::Right | Align::Inside);
    header.fixed(&addr_label, 62);
    let mut listen_input = Input::default();
    style_input(&mut listen_input, pal);
    listen_input.set_value(&config.listen_addr);
    header.fixed(&listen_input, 180);

    let mut start_btn = Button::default();
    style_primary_btn(&mut start_btn, pal);
    start_btn.set_label(&t!("start"));
    header.fixed(&start_btn, 76);

    let mut status = Frame::default().with_label(&t!("status_stopped"));
    status.set_label_color(pal.subtext);
    header.fixed(&status, 70);

    let mut auto_proxy = CheckButton::default().with_label(&t!("sys_proxy"));
    auto_proxy.set_label_color(pal.text);
    auto_proxy.set_label_size(13);
    auto_proxy.visible_focus(false);
    auto_proxy.set_checked(config.auto_proxy);
    // 勾选框：DownBox 触发 FLTK 的标准「框+✓」绘制路径，其浮雕绘制已被全局重写为微圆角平底；
    // 勾默认黑色在深色下看不见，改主色
    auto_proxy.set_down_frame(FrameType::DownBox);
    auto_proxy.set_color(pal.input_bg);
    auto_proxy.set_selection_color(pal.accent);
    header.fixed(&auto_proxy, 100);

    Frame::default(); // 弹性填充

    // 主题分段控件组：灰底圆角容器 + 选中项高亮块
    let mut seg = Flex::default().row();
    seg.set_frame(FrameType::RFlatBox);
    seg.set_color(pal.input_bg);
    seg.set_pad(0);
    seg.set_margin(3);
    let mut theme_btns: Vec<Button> = Vec::new();
    for label in [t!("theme_system"), t!("theme_dark"), t!("theme_light")] {
        let mut b = Button::default().with_label(&label);
        b.visible_focus(false);
        theme_btns.push(b);
    }
    seg.end();
    header.fixed(&seg, 196);
    // 语言切换按钮：显示目标语言（点击切过去）
    let mut lang_btn = Button::default().with_label(&t!("lang_switch"));
    style_ghost_btn(&mut lang_btn, pal);
    lang_btn.visible_focus(false);
    header.fixed(&lang_btn, 56);
    header.end();
    col.fixed(&header, 52);

    // ---- 统计卡片 ----
    let mut stats_card = Flex::default().row();
    style_card(&mut stats_card, pal);
    stats_card.set_margin(10);
    let mut stat_frames = Vec::new();
    for key in ["stats_active", "stats_total", "stats_up", "stats_down"] {
        let mut f = Frame::default().with_label(&format!("{}  -", t!(key)));
        f.set_label_color(pal.subtext);
        f.set_label_size(13);
        stat_frames.push(f);
    }
    stats_card.end();
    col.fixed(&stats_card, 44);
    let stats: [Frame; 4] = stat_frames.try_into().unwrap();

    // ---- 规则卡片 ----
    let mut rules_card = Flex::default().column();
    style_card(&mut rules_card, pal);
    rules_card.set_pad(6);
    rules_card.set_margin(12);
    let cap = caption(&t!("rules_title"), pal);
    rules_card.fixed(&cap, 26);
    // 列表头：与下方输入框逐列对齐（同几何参数：margin 4 / pad 10 / 固定列宽）
    let mut col_header = Flex::default().row();
    col_header.set_margin(4);
    col_header.set_pad(10);
    let mut col_header_frames = Vec::new();
    for (key, w) in [
        ("col_match_addr", 190),
        ("col_match_prefix", 110),
        ("", 24),
        ("col_fwd_addr", 190),
        ("col_fwd_prefix", 110),
    ] {
        let mut f = Frame::default().with_label(&t!(key));
        f.set_label_color(pal.subtext);
        f.set_label_size(12);
        f.set_align(Align::Left | Align::Inside);
        col_header.fixed(&f, w);
        col_header_frames.push(f);
    }
    let mut del_spacer = Frame::default();
    del_spacer.set_frame(FrameType::NoBox);
    col_header.fixed(&del_spacer, 52);
    col_header.end();
    rules_card.fixed(&col_header, 20);

    let mut scroll = Scroll::default();
    scroll.set_frame(FrameType::NoBox);
    scroll.set_color(pal.card);
    // 扁平滚动条：细条 + 平槽隐形 + 圆角滑块；箭头 glyph 与滑块同色使其隐形
    //（FLTK 首尾箭头按钮和滑块共用配色，8px 下只剩两个不可见的微点）
    scroll.set_scrollbar_size(8);
    for mut sb in [scroll.scrollbar(), scroll.hscrollbar()] {
        sb.set_frame(FrameType::FlatBox);
        sb.set_color(pal.card);
        sb.set_selection_color(pal.border);
        sb.set_slider_frame(FrameType::RFlatBox);
        sb.set_label_color(pal.border);
    }
    let mut rows_pack = Pack::new(0, 0, ROW_W, 0, None).with_type(PackType::Vertical);
    rows_pack.set_spacing(ROW_GAP);
    rows_pack.end();
    scroll.end();

    let mut rules_bar = Flex::default().row();
    rules_bar.set_pad(6);
    Frame::default(); // 弹性填充，把按钮推到右侧
    let mut add_btn = Button::default().with_label(&t!("add_rule"));
    style_ghost_btn(&mut add_btn, pal);
    rules_bar.fixed(&add_btn, 96);
    let mut save_btn = Button::default().with_label(&t!("save_apply"));
    style_primary_btn(&mut save_btn, pal);
    rules_bar.fixed(&save_btn, 96);
    rules_bar.end();
    rules_card.fixed(&rules_bar, 40);
    rules_card.end();

    // ---- 日志卡片 ----
    let mut log_card = Flex::default().column();
    style_card(&mut log_card, pal);
    log_card.set_pad(6);
    log_card.set_margin(12);
    let lcap = caption(&t!("log_title"), pal);
    log_card.fixed(&lcap, 26);
    let mut log_disp = TextDisplay::default();
    log_disp.set_frame(FrameType::RFlatBox);
    log_disp.set_color(pal.input_bg);
    log_disp.set_text_color(pal.text);
    log_disp.set_text_size(13);
    let log_buf = TextBuffer::default();
    log_disp.set_buffer(log_buf.clone());
    log_card.end();

    col.end();
    win.end();

    View {
        win,
        col,
        title_bar,
        icon,
        title,
        header,
        addr_label,
        listen_input,
        start_btn,
        status,
        auto_proxy,
        seg,
        theme_btns,
        lang_btn,
        stats_card,
        stats,
        rules_card,
        rule_cap: cap,
        col_header_frames,
        scroll,
        rows_pack,
        add_btn,
        save_btn,
        log_card,
        log_cap: lcap,
        log_disp,
        log_buf,
        close_btn,
        min_btn,
    }
}

/// 一条规则行的控件句柄
pub struct RowWidgets {
    pub row: Flex,
    pub m_addr: Input,
    pub m_prefix: Input,
    pub f_addr: Input,
    pub f_prefix: Input,
    pub arrow: Frame,
    pub del_btn: Button,
}

/// 在 pack 末尾构建一行规则的控件（回调由逻辑层接线）
pub fn add_rule_row_widgets(pack: &Pack, pal: Palette, idx: i32, rule: &Rule) -> RowWidgets {
    let pack = pack.clone();
    // FLTK 新建控件会自动挂到当前 group，必须先 begin 把 pack 设为当前
    pack.begin();
    let mut row = Flex::new(0, idx * (ROW_H + ROW_GAP), ROW_CONTENT_W, ROW_H, None).row();
    style_card(&mut row, pal);
    row.set_pad(10);
    row.set_margin(4);

    let mut m_addr = Input::default();
    style_input(&mut m_addr, pal);
    m_addr.set_value(&rule.matcher.addr);
    row.fixed(&m_addr, 190);

    let mut m_prefix = Input::default();
    style_input(&mut m_prefix, pal);
    m_prefix.set_value(&rule.matcher.path_prefix);
    row.fixed(&m_prefix, 110);

    let mut arrow = Frame::default().with_label("→");
    arrow.set_label_color(pal.subtext);
    row.fixed(&arrow, 24);

    let mut f_addr = Input::default();
    style_input(&mut f_addr, pal);
    f_addr.set_value(&rule.forward.addr);
    row.fixed(&f_addr, 190);

    let mut f_prefix = Input::default();
    style_input(&mut f_prefix, pal);
    f_prefix.set_value(&rule.forward.path_prefix);
    row.fixed(&f_prefix, 110);

    let mut del_btn = Button::default().with_label(&t!("rule_delete"));
    del_btn.set_frame(FrameType::RFlatBox);
    del_btn.set_down_frame(FrameType::RFlatBox);
    del_btn.set_color(pal.input_bg);
    del_btn.set_label_color(pal.danger);
    del_btn.visible_focus(false);
    row.fixed(&del_btn, 52);
    row.end();
    pack.end();

    RowWidgets {
        row,
        m_addr,
        m_prefix,
        f_addr,
        f_prefix,
        arrow,
        del_btn,
    }
}

/// Windows 平台窗口修补：Win11 圆角 + 无边框窗口补任务栏图标
#[cfg(target_os = "windows")]
pub fn platform_window_fixups(win: &Window) {
    #[link(name = "dwmapi")]
    unsafe extern "C" {
        fn DwmSetWindowAttribute(hwnd: isize, attr: u32, value: *const u32, size: u32) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "C" {
        fn GetWindowLongPtrW(hwnd: isize, idx: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: isize, idx: i32, val: isize) -> isize;
        fn ShowWindow(hwnd: isize, cmd: i32) -> i32;
    }
    const GWL_EXSTYLE: i32 = -20;
    const GWL_STYLE: i32 = -16;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const WS_EX_APPWINDOW: isize = 0x0004_0000;
    const WS_MINIMIZEBOX: isize = 0x0002_0000;
    unsafe {
        let hwnd = win.raw_handle() as isize;
        if hwnd == 0 {
            return;
        }
        // Win11 圆角，旧系统静默忽略
        DwmSetWindowAttribute(hwnd, 33, &2u32, 4);
        // 无边框窗口默认无任务栏按钮，补 WS_EX_APPWINDOW
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_TOOLWINDOW) | WS_EX_APPWINDOW);
        // 补 WS_MINIMIZEBOX，任务栏图标再次点击才能最小化
        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_MINIMIZEBOX);
        // 任务栏按钮在 show 时已创建，hide→show 强制按新样式重建
        ShowWindow(hwnd, 0); // SW_HIDE
        ShowWindow(hwnd, 5); // SW_SHOW
    }
}

#[cfg(not(target_os = "windows"))]
pub fn platform_window_fixups(_win: &Window) {}

/// 主题分段控件选中态（选中：色块；未选中：无框融入容器）
pub fn refresh_theme_btns(btns: &mut [Button], sel: usize, pal: Palette) {
    for (i, b) in btns.iter_mut().enumerate() {
        if i == sel {
            b.set_frame(FrameType::RFlatBox);
            b.set_color(pal.accent);
            b.set_label_color(Color::White);
        } else {
            b.set_frame(FrameType::NoBox);
            b.set_color(pal.input_bg);
            b.set_label_color(pal.subtext);
        }
        b.redraw(); // set_color 不带 damage，必须手动重绘
    }
}
