//! 纯 UI：配色、样式、控件构建。不含任何业务逻辑。

use crate::core::config::{AppConfig, Rule, Site, SiteRule};
use rust_i18n::t;
use fltk::enums::{Align, Color, Event, FrameType};
use fltk::app;
use fltk::group::{Flex, Group, Pack, PackType, Scroll};
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

/// 站点行与规则行同高：两页控件尺寸一致，视觉上才是一套东西
pub const SITE_ROW_H: i32 = ROW_H;
pub const SITE_ROW_GAP: i32 = ROW_GAP;
/// 站点行内容宽：边距4*2 + 启用26 + 名称120 + 本地120 + 上游190 + 跳过校验90 + 按钮114 + 点击垫10*5
pub const SITE_ROW_CONTENT_W: i32 =
    4 + 26 + 10 + 120 + 10 + 120 + 10 + 190 + 10 + 90 + 10 + 114 + 4;
pub const SITE_ROW_W: i32 = SITE_ROW_CONTENT_W + 10;

/// 站点内嵌规则行：前缀 → 转发地址 + 转发前缀
pub const SITE_RULE_ROW_H: i32 = ROW_H;
pub const SITE_RULE_ROW_GAP: i32 = ROW_GAP;
/// 边距4*2 + 前缀110 + 箭头24 + 转发190 + 转发前缀110 + 删除52 + 间距10*4
pub const SITE_RULE_ROW_CONTENT_W: i32 =
    4 + 110 + 10 + 24 + 10 + 190 + 10 + 110 + 10 + 52 + 4;
pub const SITE_RULE_ROW_W: i32 = SITE_RULE_ROW_CONTENT_W + 10;

/// 扁平化配色
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color,       // 窗口底色
    pub card: Color,     // 卡片
    pub text: Color,     // 主文本
    pub subtext: Color,  // 次要文本
    pub input_bg: Color, // 输入框
    pub seg_bg: Color,   // 分段控件（页签/主题）的槽底色
    pub row_sel: Color,  // 选中的站点行底色（规则区正在编辑谁）
    pub check_bg: Color, // 复选框框体（FLTK 用全局 FL_BACKGROUND2_COLOR 画，也影响勾的取色）
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
            seg_bg: Color::from_hex(0x111827),
            row_sel: Color::from_hex(0x1e3a5f),
            // 勾选色要过 fl_contrast(accent, check_bg) 这一关：对比不足会被换成白色，
            // 所以框体取足够深的色，蓝勾才留得住（阈值 99，accent 亮度约 121）
            check_bg: Color::from_hex(0x0a1018),
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
            seg_bg: Color::from_hex(0xf3f4f6),
            row_sel: Color::from_hex(0xeff6ff),
            check_bg: Color::from_hex(0xf3f4f6),
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

/// 复选框统一字号：FLTK 的框大小直接取 labelsize，调大才会跟着变大
pub const CHECK_LABEL_SIZE: i32 = 15;

/// 复选框：框体走 DownBox（已被全局重写成微圆角平底），勾选色用主色
pub(crate) fn style_check(b: &mut CheckButton, pal: Palette, label_size: i32) {
    b.set_down_frame(FrameType::DownBox);
    b.set_color(pal.input_bg);
    b.set_selection_color(pal.accent);
    b.set_label_color(pal.text);
    b.set_label_size(label_size);
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
    /// 顶栏：页签 + 主题 + 语言（两种模式共用）
    pub header: Flex,
    pub tab_bar: Flex,
    pub tab_btns: Vec<Button>,
    pub seg: Flex,
    pub theme_btns: Vec<Button>,
    pub lang_btn: Button,
    /// 页面容器：代理页和站点页绝对定位叠放，靠 hide/show 切换
    pub page_area: Group,
    pub page_proxy: Flex,
    pub page_site: Flex,
    // ---- 代理模式页 ----
    pub proxy_bar: Flex,
    pub addr_label: Frame,
    pub listen_input: Input,
    pub proxy_start_btn: Button,
    pub proxy_status: Frame,
    pub auto_proxy: CheckButton,
    pub rules_card: Flex,
    pub rule_cap: Frame,
    pub col_header_frames: Vec<Frame>,
    pub scroll: Scroll,
    pub rows_pack: Pack,
    pub add_btn: Button,
    pub save_btn: Button,
    // ---- 镜像站点页 ----
    pub site_bar: Flex,
    pub site_hint: Frame,
    pub site_start_btn: Button,
    pub site_status: Frame,
    pub sites_card: Flex,
    pub site_cap: Frame,
    pub site_col_header_frames: Vec<Frame>,
    pub site_scroll: Scroll,
    pub site_pack: Pack,
    pub add_site_btn: Button,
    pub site_rules_card: Flex,
    pub site_rule_cap: Frame,
    pub site_rule_header_frames: Vec<Frame>,
    pub site_rule_scroll: Scroll,
    pub site_rule_pack: Pack,
    pub add_site_rule_btn: Button,
    pub site_save_btn: Button,
    // ---- 两页共用 ----
    pub stats_card: Flex,
    pub stats: [Frame; 4],
    pub log_card: Flex,
    pub log_cap: Frame,
    pub log_disp: TextDisplay,
    pub log_buf: TextBuffer,
    pub close_btn: Button,
    pub min_btn: Button,
    pub gh_btn: Button,
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
            tab_bar: self.tab_bar.clone(),
            tab_btns: self.tab_btns.clone(),
            seg: self.seg.clone(),
            theme_btns: self.theme_btns.clone(),
            lang_btn: self.lang_btn.clone(),
            page_area: self.page_area.clone(),
            page_proxy: self.page_proxy.clone(),
            page_site: self.page_site.clone(),
            proxy_bar: self.proxy_bar.clone(),
            addr_label: self.addr_label.clone(),
            listen_input: self.listen_input.clone(),
            proxy_start_btn: self.proxy_start_btn.clone(),
            proxy_status: self.proxy_status.clone(),
            auto_proxy: self.auto_proxy.clone(),
            rules_card: self.rules_card.clone(),
            rule_cap: self.rule_cap.clone(),
            col_header_frames: self.col_header_frames.clone(),
            scroll: self.scroll.clone(),
            rows_pack: self.rows_pack.clone(),
            add_btn: self.add_btn.clone(),
            save_btn: self.save_btn.clone(),
            site_bar: self.site_bar.clone(),
            site_hint: self.site_hint.clone(),
            site_start_btn: self.site_start_btn.clone(),
            site_status: self.site_status.clone(),
            sites_card: self.sites_card.clone(),
            site_cap: self.site_cap.clone(),
            site_col_header_frames: self.site_col_header_frames.clone(),
            site_scroll: self.site_scroll.clone(),
            site_pack: self.site_pack.clone(),
            add_site_btn: self.add_site_btn.clone(),
            site_rules_card: self.site_rules_card.clone(),
            site_rule_cap: self.site_rule_cap.clone(),
            site_rule_header_frames: self.site_rule_header_frames.clone(),
            site_rule_scroll: self.site_rule_scroll.clone(),
            site_rule_pack: self.site_rule_pack.clone(),
            add_site_rule_btn: self.add_site_rule_btn.clone(),
            site_save_btn: self.site_save_btn.clone(),
            stats_card: self.stats_card.clone(),
            stats: self.stats.clone(),
            log_card: self.log_card.clone(),
            log_cap: self.log_cap.clone(),
            log_disp: self.log_disp.clone(),
            log_buf: self.log_buf.clone(),
            close_btn: self.close_btn.clone(),
            min_btn: self.min_btn.clone(),
            gh_btn: self.gh_btn.clone(),
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

/// GitHub 标记（simple-icons 的 24×24 路径），随主题配色重新填充
const GITHUB_MARK: &str = "M12 .297c-6.63 0-12 5.373-12 12 0 5.303 3.438 9.8 8.205 11.385.6.113.82-.258.82-.577 0-.285-.01-1.04-.015-2.04-3.338.724-4.042-1.61-4.042-1.61C4.422 18.07 3.633 17.7 3.633 17.7c-1.087-.744.084-.729.084-.729 1.205.084 1.838 1.236 1.838 1.236 1.07 1.835 2.809 1.305 3.495.998.108-.776.417-1.305.76-1.605-2.665-.3-5.466-1.332-5.466-5.93 0-1.31.465-2.38 1.235-3.22-.135-.303-.54-1.523.105-3.176 0 0 1.005-.322 3.3 1.23.96-.267 1.98-.399 3-.405 1.02.006 2.04.138 3 .405 2.28-1.552 3.285-1.23 3.285-1.23.645 1.653.24 2.873.12 3.176.765.84 1.23 1.91 1.23 3.22 0 4.61-2.805 5.625-5.475 5.92.42.36.81 1.096.81 2.22 0 1.606-.015 2.896-.015 3.286 0 .315.21.69.825.57C20.565 22.092 24 17.592 24 12.297c0-6.627-5.373-12-12-12";

/// 图标在按钮里的边长（逻辑像素）：和 `—`/`✕` 的视觉高度对齐
const GH_ICON: i32 = 15;

/// 生成指定颜色的 GitHub 图标（fltk 1.5 没有 svg feature，但 SvgImage 本身可用）
/// 必须 scale 到目标尺寸：不带 svg feature 时 `draw(x, y, w, h)` 不会缩放，会按原始 24 单位画并裁掉
fn github_icon(color: Color) -> Option<fltk::image::SvgImage> {
    let svg = format!(
        r#"<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg"><path fill="{}" d="{GITHUB_MARK}"/></svg>"#,
        color.to_hex_str()
    );
    let mut img = fltk::image::SvgImage::from_data(&svg).ok()?;
    img.scale(GH_ICON, GH_ICON, false, true);
    Some(img)
}

/// GitHub 图标缓存：自绘时按当前配色取用，换主题后自动重新生成。
/// `(生成图标用的颜色, 图标)` —— 颜色不同说明主题变了，得重画一份新的
type IconSlot = std::cell::RefCell<(Color, Option<fltk::image::SvgImage>)>;
type IconCache = std::rc::Rc<IconSlot>;

/// 图标按钮自己画：底板 + 正中的图标。
/// 不用 widget 自带的 image：FLTK 把「图 + 空标签」当成图上文下整体居中，图标会被顶到
/// 上半格；而且这条绘制路径还会在按钮上方留下残影。
fn draw_icon_btn(b: &mut Button, cache: &IconSlot) {
    let (x, y, w, h) = (b.x(), b.y(), b.w(), b.h());
    fltk::draw::draw_box(b.frame(), x, y, w, h, b.color());
    let mut slot = cache.borrow_mut();
    let fg = current_pal().subtext;
    if slot.0 != fg || slot.1.is_none() {
        slot.0 = fg;
        slot.1 = github_icon(fg);
    }
    if let Some(img) = slot.1.as_mut() {
        // +1 再除：向上取整半像素，和右边 `—`/`✕` 的视觉中线对齐
        let (ix, iy) = (x + (w - GH_ICON) / 2, y + (h - GH_ICON + 1) / 2);
        img.draw(ix, iy, GH_ICON, GH_ICON);
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

/// 扁平滚动条：细条 + 平槽隐形 + 圆角滑块
pub(crate) fn style_scroll(scroll: &mut Scroll, pal: Palette) {
    scroll.set_frame(FrameType::NoBox);
    scroll.set_color(pal.card);
    //（FLTK 首尾箭头按钮和滑块共用配色，8px 下只剩两个不可见的微点）
    scroll.set_scrollbar_size(8);
    for mut sb in [scroll.scrollbar(), scroll.hscrollbar()] {
        sb.set_frame(FrameType::FlatBox);
        sb.set_color(pal.card);
        sb.set_selection_color(pal.border);
        sb.set_slider_frame(FrameType::RFlatBox);
        sb.set_label_color(pal.border);
    }
}

/// 列宽定义：(locale key, 宽度)
const RULE_COLS: [(&str, i32); 5] = [
    ("col_match_addr", 190),
    ("col_match_prefix", 110),
    ("", 24),
    ("col_fwd_addr", 190),
    ("col_fwd_prefix", 110),
];
const SITE_COLS: [(&str, i32); 6] = [
    ("col_enable", 26),
    ("col_site_name", 120),
    ("col_site_listen", 120),
    ("col_site_upstream", 190),
    ("col_site_insecure", 90),
    ("", 114),
];
const SITE_RULE_COLS: [(&str, i32); 5] = [
    ("col_rule_prefix", 110),
    ("", 24),
    ("col_rule_forward", 190),
    ("col_rule_fwd_prefix", 110),
    ("", 52),
];

/// 表头行：与下方输入框逐列对齐（同几何参数：margin 4 / pad 10 / 固定列宽）
fn header_row(cols: &[(&str, i32)], pal: Palette) -> (Flex, Vec<Frame>) {
    let mut row = Flex::default().row();
    row.set_margin(4);
    row.set_pad(10);
    let mut frames = Vec::new();
    for &(key, w) in cols {
        let mut f = Frame::default();
        if !key.is_empty() {
            f.set_label(&t!(key));
        }
        f.set_label_color(pal.subtext);
        f.set_label_size(12);
        f.set_align(Align::Left | Align::Inside);
        row.fixed(&f, w);
        frames.push(f);
    }
    row.end();
    (row, frames)
}

/// 构建整个窗口（不 show，由调用方决定时机）
pub fn build(config: &AppConfig, pal: Palette) -> View {
    let mut win = Window::default().with_size(800, 820).center_screen();
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
    // GitHub 仓库：图标按钮（地址取自 Cargo.toml 的 repository），点击用系统浏览器打开
    let mut gh_btn = chrome_btn("", false);
    gh_btn.set_tooltip(env!("CARGO_PKG_REPOSITORY"));
    let cache: IconCache = std::rc::Rc::new(std::cell::RefCell::new((Color::Black, None)));
    gh_btn.draw(move |b| draw_icon_btn(b, &cache));
    title_bar.fixed(&gh_btn, 32);
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
    let mut col = Flex::new(1, 33, 798, 786, None).column();
    col.set_frame(FrameType::FlatBox);
    col.set_color(pal.bg);
    col.set_margin(12);
    col.set_pad(10);

    // ---- 顶栏：模式页签 + 主题 + 语言（两种模式共用） ----
    let mut header = Flex::default().row();
    style_card(&mut header, pal);
    header.set_pad(9);
    header.set_margin(6);

    // 页签：灰底圆角容器 + 选中项高亮块（与主题分段控件同款）
    let mut tabs = Flex::default().row();
    tabs.set_frame(FrameType::RFlatBox);
    tabs.set_color(pal.seg_bg);
    tabs.set_pad(0);
    tabs.set_margin(3);
    let mut tab_btns: Vec<Button> = Vec::new();
    // 镜像站点在前：它是主推用法，默认也停在这一页
    for label in [t!("tab_sites"), t!("tab_proxy")] {
        let mut b = Button::default().with_label(&label);
        b.visible_focus(false);
        tab_btns.push(b);
    }
    tabs.end();
    header.fixed(&tabs, 180);

    Frame::default(); // 弹性填充

    // 主题分段控件组：灰底圆角容器 + 选中项高亮块
    let mut seg = Flex::default().row();
    seg.set_frame(FrameType::RFlatBox);
    seg.set_color(pal.seg_bg);
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

    // ---- 页面容器：两个页面绝对定位叠放，靠 hide/show 切换（Flex 的 fixed=0 不会隐藏子控件） ----
    let mut page_area = Group::default();
    page_area.set_frame(FrameType::NoBox);
    page_area.set_color(pal.bg);

    // ======== 代理模式页 ========
    let mut page_proxy = Flex::new(0, 0, 0, 0, None).column();
    page_proxy.set_frame(FrameType::FlatBox);
    page_proxy.set_color(pal.bg);
    page_proxy.set_pad(10);
    page_proxy.set_margin(0);

    let mut proxy_bar = Flex::default().row();
    style_card(&mut proxy_bar, pal);
    proxy_bar.set_pad(9);
    proxy_bar.set_margin(6);
    let mut addr_label = Frame::default().with_label(&t!("listen_addr"));
    addr_label.set_label_color(pal.subtext);
    addr_label.set_align(Align::Right | Align::Inside);
    proxy_bar.fixed(&addr_label, 62);
    let mut listen_input = Input::default();
    style_input(&mut listen_input, pal);
    listen_input.set_value(&config.proxy_listen);
    proxy_bar.fixed(&listen_input, 180);
    let mut auto_proxy = CheckButton::default().with_label(&t!("sys_proxy"));
    style_check(&mut auto_proxy, pal, CHECK_LABEL_SIZE);
    auto_proxy.set_checked(config.auto_proxy);
    proxy_bar.fixed(&auto_proxy, 110);
    Frame::default(); // 弹性填充：把启动按钮和状态推到右侧，与镜像站点页对齐
    let mut proxy_start_btn = Button::default();
    style_primary_btn(&mut proxy_start_btn, pal);
    proxy_start_btn.set_label(&t!("start"));
    proxy_bar.fixed(&proxy_start_btn, 76);
    let mut proxy_status = Frame::default().with_label(&t!("status_stopped"));
    proxy_status.set_label_color(pal.subtext);
    proxy_bar.fixed(&proxy_status, 70);
    proxy_bar.end();
    page_proxy.fixed(&proxy_bar, 52);

    // 代理规则卡片
    let mut rules_card = Flex::default().column();
    style_card(&mut rules_card, pal);
    rules_card.set_pad(6);
    rules_card.set_margin(12);
    rules_card.begin();
    let cap = caption(&t!("rules_title"), pal);
    rules_card.fixed(&cap, 26);
    let (col_header, col_header_frames) = header_row(&RULE_COLS, pal);
    rules_card.fixed(&col_header, 20);
    let mut scroll = Scroll::default();
    style_scroll(&mut scroll, pal);
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
    page_proxy.end();

    // ======== 镜像站点页 ========
    page_area.begin();
    let mut page_site = Flex::new(0, 0, 0, 0, None).column();
    page_site.set_frame(FrameType::FlatBox);
    page_site.set_color(pal.bg);
    page_site.set_pad(10);
    page_site.set_margin(0);

    let mut site_bar = Flex::default().row();
    style_card(&mut site_bar, pal);
    site_bar.set_pad(9);
    site_bar.set_margin(6);
    let mut site_hint = Frame::default().with_label(&t!("site_hint"));
    site_hint.set_label_color(pal.subtext);
    site_hint.set_label_size(13);
    site_hint.set_align(Align::Left | Align::Inside);
    let mut site_start_btn = Button::default();
    style_primary_btn(&mut site_start_btn, pal);
    site_start_btn.set_label(&t!("start"));
    site_bar.fixed(&site_start_btn, 76);
    let mut site_status = Frame::default().with_label(&t!("status_stopped"));
    site_status.set_label_color(pal.subtext);
    site_bar.fixed(&site_status, 70);
    site_bar.end();
    page_site.fixed(&site_bar, 52);

    // 站点列表卡片
    let mut sites_card = Flex::default().column();
    style_card(&mut sites_card, pal);
    sites_card.set_pad(6);
    sites_card.set_margin(12);
    sites_card.begin();
    let site_cap = caption(&t!("sites_title"), pal);
    sites_card.fixed(&site_cap, 26);
    let (site_header, site_col_header_frames) = header_row(&SITE_COLS, pal);
    sites_card.fixed(&site_header, 20);
    let mut site_scroll = Scroll::default();
    style_scroll(&mut site_scroll, pal);
    let mut site_pack = Pack::new(0, 0, SITE_ROW_W, 0, None).with_type(PackType::Vertical);
    site_pack.set_spacing(SITE_ROW_GAP);
    site_pack.end();
    site_scroll.end();
    let mut sites_bar = Flex::default().row();
    sites_bar.set_pad(6);
    Frame::default(); // 弹性填充
    let mut add_site_btn = Button::default().with_label(&t!("add_site"));
    style_ghost_btn(&mut add_site_btn, pal);
    sites_bar.fixed(&add_site_btn, 96);
    sites_bar.end();
    sites_card.fixed(&sites_bar, 40);
    sites_card.end();
    // 卡片高度 = 边距24 + 标题26 + 表头20 + 两行(40*2+8) + 按钮栏40 + pad12 + 余量4（不留余量会顶出滚动条）
    page_site.fixed(&sites_card, 214);

    // 选中站点的转发规则卡片
    let mut site_rules_card = Flex::default().column();
    style_card(&mut site_rules_card, pal);
    site_rules_card.set_pad(6);
    site_rules_card.set_margin(12);
    // 标题行：文案随选中站点变化（「本站转发规则 · 站点名」），由逻辑层刷新
    site_rules_card.begin();
    let site_rule_cap = caption(&t!("site_rules_title"), pal);
    site_rules_card.fixed(&site_rule_cap, 26);
    site_rules_card.begin();
    let (site_rule_header, site_rule_header_frames) = header_row(&SITE_RULE_COLS, pal);
    site_rules_card.fixed(&site_rule_header, 20);
    site_rules_card.begin();
    let mut site_rule_scroll = Scroll::default();
    style_scroll(&mut site_rule_scroll, pal);
    let mut site_rule_pack =
        Pack::new(0, 0, SITE_RULE_ROW_W, 0, None).with_type(PackType::Vertical);
    site_rule_pack.set_spacing(SITE_RULE_ROW_GAP);
    site_rule_pack.end();
    site_rule_scroll.end();
    site_rules_card.begin();
    let mut site_rules_bar = Flex::default().row();
    site_rules_bar.set_pad(6);
    Frame::default(); // 弹性填充
    let mut add_site_rule_btn = Button::default().with_label(&t!("add_rule"));
    style_ghost_btn(&mut add_site_rule_btn, pal);
    site_rules_bar.fixed(&add_site_rule_btn, 96);
    let mut site_save_btn = Button::default().with_label(&t!("save_apply"));
    style_primary_btn(&mut site_save_btn, pal);
    site_rules_bar.fixed(&site_save_btn, 96);
    site_rules_bar.end();
    site_rules_card.fixed(&site_rules_bar, 40);
    site_rules_card.end();
    page_site.end();

    page_area.end();
    // page_area 不 fixed → 吃掉剩余高度；两个页面各自的 fixed 由 controller 切换

    // ---- 统计卡片（两页共用） ----
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

    // ---- 日志卡片（两页共用） ----
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
    col.fixed(&log_card, 130);

    col.end();
    win.end();

    View {
        win,
        col,
        title_bar,
        icon,
        title,
        header,
        tab_bar: tabs,
        tab_btns,
        seg,
        theme_btns,
        lang_btn,
        page_area,
        page_proxy,
        page_site,
        proxy_bar,
        addr_label,
        listen_input,
        proxy_start_btn,
        proxy_status,
        auto_proxy,
        rules_card,
        rule_cap: cap,
        col_header_frames,
        scroll,
        rows_pack,
        add_btn,
        save_btn,
        site_bar,
        site_hint,
        site_start_btn,
        site_status,
        sites_card,
        site_cap,
        site_col_header_frames,
        site_scroll,
        site_pack,
        add_site_btn,
        site_rules_card,
        site_rule_cap,
        site_rule_header_frames,
        site_rule_scroll,
        site_rule_pack,
        add_site_rule_btn,
        site_save_btn,
        stats_card,
        stats,
        log_card,
        log_cap: lcap,
        log_disp,
        log_buf,
        close_btn,
        min_btn,
        gh_btn,
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

/// 一个站点行的控件句柄
pub struct SiteWidgets {
    pub row: Flex,
    pub enable: CheckButton,
    pub name: Input,
    pub listen: Input,
    pub upstream: Input,
    pub insecure: CheckButton,
    pub open_btn: Button,
    pub del_btn: Button,
    /// 列之间那些「不可见的点击垫」：FLTK 的 Group 不处理 FL_PUSH，
    /// 光靠行容器的 callback 收不到空白处点击，垫子用无框 Button 顶上
    pub gaps: Vec<Button>,
}

/// 在 pack 末尾构建一行站点配置（回调由逻辑层接线）
pub fn add_site_row_widgets(pack: &Pack, pal: Palette, idx: i32, site: &Site) -> SiteWidgets {
    let pack = pack.clone();
    pack.begin();
    let mut row = Flex::new(
        0,
        idx * (SITE_ROW_H + SITE_ROW_GAP),
        SITE_ROW_CONTENT_W,
        SITE_ROW_H,
        None,
    )
    .row();
    style_card(&mut row, pal);
    // 列间距不再交给 pad，而是插入可点中的垫子（宽度还是 10，行宽不变）
    row.set_pad(0);
    row.set_margin(4);

    let mut gaps = Vec::new();
    let add_gap = |row: &mut Flex, gaps: &mut Vec<Button>| {
        let mut b = Button::default();
        b.set_frame(FrameType::NoBox);
        b.set_down_frame(FrameType::NoBox);
        b.set_color(pal.card);
        b.set_label("");
        b.visible_focus(false);
        row.fixed(&b, 10);
        gaps.push(b);
    };

    let mut enable = CheckButton::default();
    style_check(&mut enable, pal, CHECK_LABEL_SIZE);
    enable.set_checked(site.enable);
    row.fixed(&enable, 26);
    add_gap(&mut row, &mut gaps);

    let mut name = Input::default();
    style_input(&mut name, pal);
    name.set_value(&site.name);
    row.fixed(&name, 120);
    add_gap(&mut row, &mut gaps);

    let mut listen = Input::default();
    style_input(&mut listen, pal);
    listen.set_value(&site.listen_addr);
    row.fixed(&listen, 120);
    add_gap(&mut row, &mut gaps);

    let mut upstream = Input::default();
    style_input(&mut upstream, pal);
    upstream.set_value(&site.upstream);
    row.fixed(&upstream, 190);
    add_gap(&mut row, &mut gaps);

    let mut insecure = CheckButton::default().with_label(&t!("site_insecure"));
    style_check(&mut insecure, pal, CHECK_LABEL_SIZE);
    insecure.set_label_color(pal.subtext);
    insecure.set_checked(site.insecure);
    row.fixed(&insecure, 90);
    add_gap(&mut row, &mut gaps);

    let mut open_btn = Button::default().with_label(&t!("site_open"));
    style_ghost_btn(&mut open_btn, pal);
    row.fixed(&open_btn, 52);

    let mut del_btn = Button::default().with_label(&t!("rule_delete"));
    del_btn.set_frame(FrameType::RFlatBox);
    del_btn.set_down_frame(FrameType::RFlatBox);
    del_btn.set_color(pal.input_bg);
    del_btn.set_label_color(pal.danger);
    del_btn.visible_focus(false);
    row.fixed(&del_btn, 52);

    row.end();
    pack.end();

    SiteWidgets {
        row,
        enable,
        name,
        listen,
        upstream,
        insecure,
        open_btn,
        del_btn,
        gaps,
    }
}

/// 一条站点内嵌规则行的控件句柄
pub struct SiteRuleWidgets {
    pub row: Flex,
    pub prefix: Input,
    pub arrow: Frame,
    pub addr: Input,
    pub fwd_prefix: Input,
    pub del_btn: Button,
}

/// 在 pack 末尾构建一行站点规则（回调由逻辑层接线）
pub fn add_site_rule_row_widgets(
    pack: &Pack,
    pal: Palette,
    idx: i32,
    rule: &SiteRule,
) -> SiteRuleWidgets {
    let pack = pack.clone();
    pack.begin();
    let mut row = Flex::new(
        0,
        idx * (SITE_RULE_ROW_H + SITE_RULE_ROW_GAP),
        SITE_RULE_ROW_CONTENT_W,
        SITE_RULE_ROW_H,
        None,
    )
    .row();
    style_card(&mut row, pal);
    row.set_pad(10);
    row.set_margin(4);

    let mut prefix = Input::default();
    style_input(&mut prefix, pal);
    prefix.set_value(&rule.path_prefix);
    row.fixed(&prefix, 110);

    let mut arrow = Frame::default().with_label("→");
    arrow.set_label_color(pal.subtext);
    row.fixed(&arrow, 24);

    let mut addr = Input::default();
    style_input(&mut addr, pal);
    addr.set_value(&rule.forward_addr);
    row.fixed(&addr, 190);

    let mut fwd_prefix = Input::default();
    style_input(&mut fwd_prefix, pal);
    fwd_prefix.set_value(&rule.forward_prefix);
    row.fixed(&fwd_prefix, 110);

    let mut del_btn = Button::default().with_label(&t!("rule_delete"));
    del_btn.set_frame(FrameType::RFlatBox);
    del_btn.set_down_frame(FrameType::RFlatBox);
    del_btn.set_color(pal.input_bg);
    del_btn.set_label_color(pal.danger);
    del_btn.visible_focus(false);
    row.fixed(&del_btn, 52);

    row.end();
    pack.end();

    SiteRuleWidgets {
        row,
        prefix,
        arrow,
        addr,
        fwd_prefix,
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

/// 非 Windows 平台没有 dwmapi/user32，留空实现让调用点不分平台
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
            b.set_color(pal.seg_bg); // 必须和分段容器同色，否则未选中项会亮出一块
            b.set_label_color(pal.subtext);
        }
        b.redraw(); // set_color 不带 damage，必须手动重绘
    }
}

/// 页签选中态：与主题分段控件同一套画法
pub fn refresh_tab_btns(btns: &mut [Button], sel: usize, pal: Palette) {
    refresh_theme_btns(btns, sel, pal);
}
