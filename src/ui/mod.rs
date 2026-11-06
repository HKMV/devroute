//! UI 入口：组装 view + controller，跑事件循环。

pub mod controller;
pub mod view;

use crate::core::config::AppConfig;
use crate::core::stats::Stats;
use fltk::app;
use fltk::enums::FrameType;
use fltk::prelude::*;
use fltk_theme::{ThemeType, WidgetTheme};
use std::collections::VecDeque;

/// 铺全局配色与框型，返回对应调色板。
/// 启动时和运行中切换主题都要走这里 —— 少刷一项，切完主题那部分控件就还是旧色。
pub fn apply_global_style(dark: bool) -> view::Palette {
    // 基础主题修系统件（滚动条等），上层配色全部手动覆盖
    WidgetTheme::new(if dark { ThemeType::Dark } else { ThemeType::Greybird }).apply();
    // 上面这一步会重定义标准框型，自定义框型必须放在它之后覆盖
    // RFlatBox=6px 圆角纯填充；RoundUpBox=6px 圆角填充+1px 描边（输入框专用）；
    // DownBox=2px 微圆角填充+描边（复选框走这条路画 ✓）
    app::set_frame_type_cb(FrameType::RFlatBox, view::draw_flat_round, 0, 0, 0, 0);
    // 输入框框型给 2px 左右内缩：FLTK 的输入框文字就贴在 box 内沿，不内缩光标会压在描边上
    app::set_frame_type_cb(FrameType::RoundUpBox, view::draw_input_round, 2, 0, 4, 0);
    // BorderBox=行内小按钮的填充+描边（本项目没有别处用 FL_BORDER_BOX）
    app::set_frame_type_cb(FrameType::BorderBox, view::draw_btn_round, 0, 0, 0, 0);
    // RoundedBox=滚动条滑块（圆头细条；同一个框型也画滚动条两端的箭头按钮，那里按尺寸跳过）
    app::set_frame_type_cb(FrameType::RoundedBox, view::draw_scroll_thumb, 0, 0, 0, 0);
    // ShadowBox=弹窗蒙版（半透明铺满）；OvalBox=弹窗卡片（12px 圆角 + 描边）
    app::set_frame_type_cb(FrameType::ShadowBox, view::draw_mask, 0, 0, 0, 0);
    app::set_frame_type_cb(FrameType::OvalBox, view::draw_dialog_card, 0, 0, 0, 0);
    app::set_frame_type_cb(FrameType::DownBox, view::draw_flat_round_small, 1, 1, 2, 2);
    app::set_font_size(14);
    // 注意：不要全局 set_visible_focus(false)，否则输入框光标不闪；
    // 焦点框在各按钮上单独关闭

    let pal = view::palette(dark);
    // 复选框的框体和勾选色都由 FLTK 的全局色决定（Fl_Light_Button 里写死的）：
    // FL_BACKGROUND2_COLOR 既不随控件 color() 走，又会参与 fl_contrast(勾选色, 框体色) 的取舍
    // —— 框体不够暗，蓝勾会被换成白色。所以每次刷主题都把它按调色板重设一遍。
    let (r, g, b) = pal.check_bg.to_rgb();
    app::background2(r, g, b);
    let (r, g, b) = pal.accent.to_rgb();
    app::set_selection_color(r, g, b);
    pal
}

pub fn run() -> anyhow::Result<()> {
    let app = app::App::default();
    let (log_tx, log_rx) = app::channel::<String>();
    crate::libs::logs::init_default_with_ui(move |line| log_tx.send(line))?;

    // 主题：启动时应用
    let config = AppConfig::init().unwrap_or_default();
    let dark = match config.theme.as_str() {
        "dark" => true,
        "light" => false,
        _ => matches!(dark_light::detect(), Ok(dark_light::Mode::Dark)),
    };
    let pal = apply_global_style(dark);
    view::set_current_pal(pal);
    view::set_theme_sel(view::theme_index(&config));

    let stats = Stats::shared();
    let model = controller::new_model();
    let site_model = controller::new_site_model();
    let site_rule_model = controller::new_site_rule_model();
    let proxy_slot = controller::new_run_slot();
    let sites_slot = controller::new_run_slot();

    let mut v = view::build(&config, pal);

    // 初始代理规则行
    for rule in config.proxy_rules.clone() {
        controller::add_rule(&v.rows_pack, &model, rule);
    }
    controller::resize_pack(&v.rows_pack, &model);
    // 初始站点行
    for site in config.sites.clone() {
        controller::add_site(&v, &v.site_pack.clone(), &site_model, &site_rule_model, site);
    }
    controller::resize_site_pack(&v.site_pack, &site_model);
    view::refresh_theme_btns(&mut v.theme_btns, view::theme_index(&config), pal);

    // 默认选中第一个站点（规则区显示它自己的规则）
    let first_site = controller::site_id_at(&site_model, 0);
    controller::select_site(&v, &site_model, &site_rule_model, first_site);

    // 先让外层 Flex 分配一次尺寸，page_area 才有真实高度可供页签切换
    v.col.clone().layout();
    controller::show_tab(&v, 0); // 默认停在镜像站点页
    // 截图脚本用：DEVROUTE_UI_TAB=1 启动即停在代理模式页（普通运行不受影响）
    if let Some(idx) = std::env::var("DEVROUTE_UI_TAB")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
    {
        controller::show_tab(&v, idx.min(1));
    }

    controller::wire(
        &mut v,
        &model,
        &site_model,
        &site_rule_model,
        &proxy_slot,
        &sites_slot,
        &stats,
    );
    controller::start_stats_timer(stats.clone(), v.stats.clone());
    controller::start_cursor_blink(
        model.clone(),
        site_model.clone(),
        site_rule_model.clone(),
        v.listen_input.clone(),
    );
    // 点站点行即选中（输入框靠焦点轮询、按钮靠各自回调、空白区靠行回调）
    controller::start_site_row_watch(&v, &site_model, &site_rule_model);

    v.win.show();
    view::platform_window_fixups(&v.win);

    // 截图脚本用：DEVROUTE_UI_POPUP=<站点序号> 启动就展开该站点的规则弹窗（普通运行不受影响）
    if let Some(idx) = std::env::var("DEVROUTE_UI_POPUP")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
    {
        let v2 = v.clone_ref();
        let site_model2 = site_model.clone();
        let rule_model2 = site_rule_model.clone();
        // 等一帧：蒙版层要等窗口真正布局完才有尺寸
        app::add_timeout3(0.3, move |_| {
            if let Some(id) = controller::site_id_at(&site_model2, idx) {
                controller::open_site_rules_popup(&v2, &site_model2, &rule_model2, id);
            }
        });
    }

    // 隐藏自测：PROXY_FORWARD_UI_TEST=1 时模拟删除第一行 + 点主题按钮，验证后退出
    #[cfg(debug_assertions)]
    if std::env::var("PROXY_FORWARD_UI_TEST").is_ok() {
        let model = model.clone();
        let pack = v.rows_pack.clone();
        let mut btns = v.theme_btns.clone();
        app::add_timeout3(0.5, move |_| {
            tracing::info!(
                "UITEST: rows = {}, pack children = {}",
                model.borrow().len(),
                pack.children()
            );
            // 模拟删除回调的完整逻辑
            let entry = {
                let mut m = model.borrow_mut();
                if m.is_empty() { None } else { Some(m.remove(0)) }
            };
            if let Some(entry) = entry {
                app::delete_widget(entry.widget);
                {
                    let m = model.borrow();
                    for (idx, e) in m.iter().enumerate() {
                        let mut w = e.widget.clone();
                        w.set_pos(0, idx as i32 * (view::ROW_H + view::ROW_GAP));
                    }
                }
                controller::resize_pack(&pack, &model);
            }
            // 程序化点击主题按钮 1，打印颜色验证选中态
            btns[1].do_callback();
            let model2 = model.clone();
            let pack2 = pack.clone();
            let btns2 = btns.clone();
            app::add_timeout3(1.0, move |_| {
                tracing::info!(
                    "UITEST after delete: rows = {}, pack children = {}",
                    model2.borrow().len(),
                    pack2.children()
                );
                for (i, b) in btns2.iter().enumerate() {
                    tracing::info!("UITEST theme btn[{i}] color = {:?}", b.color());
                }
                app::add_timeout3(0.5, |_| app::quit());
            });
        });
    }

    // 主循环：驱动 UI + 收日志
    let mut log_lines: VecDeque<String> = VecDeque::new();
    while app.wait() {
        let mut dirty = false;
        while let Some(line) = log_rx.recv() {
            if log_lines.len() >= 1000 {
                log_lines.pop_front();
            }
            log_lines.push_back(line);
            dirty = true;
        }
        if dirty {
            // ponytail: 全量 join 重建文本，1000 行上限下开销可忽略
            let text = log_lines
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let mut buf = v.log_buf.clone();
            buf.set_text(&text);
            v.log_disp.scroll(buf.count_lines(0, buf.length()), 0);
        }
    }
    Ok(())
}
