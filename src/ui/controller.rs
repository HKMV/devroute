//! 业务逻辑：规则模型、代理/站点进程控制、配置持久化、定时刷新。

use crate::core::config::{AppConfig, Host, Rule, Site, SiteRule};
use crate::core::stats::Stats;
use crate::daemon::{self, Command};
use crate::ui::view::{self, View};
use rust_i18n::t;
use rust_i18n::set_locale;
use fltk::enums::CallbackTrigger;
use fltk::{app, frame::Frame, group::Pack, input::Input, prelude::*};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc::UnboundedSender;

thread_local! {
    /// 当前显示的页签：0=镜像站点（默认） 1=代理模式
    static CURRENT_TAB: Cell<usize> = Cell::new(0);
    /// 两种模式各自的运行状态：图标按「任一在跑」点亮
    static PROXY_RUNNING: Cell<bool> = Cell::new(false);
    static SITES_RUNNING: Cell<bool> = Cell::new(false);
    /// 站点列表里正在编辑规则的站点 id（0 = 没选中）
    static SELECTED_SITE: Cell<u64> = Cell::new(0);
}

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

/// 一行镜像站点：id 做稳定标识
pub struct SiteRow {
    pub id: u64,
    pub site: Site,
    pub widget: fltk::group::Flex,
    pub inputs: [Input; 3],
    pub enable: fltk::button::CheckButton,
    pub insecure: fltk::button::CheckButton,
    pub open_btn: fltk::button::Button,
    pub del_btn: fltk::button::Button,
}

pub type SiteModel = Rc<RefCell<Vec<SiteRow>>>;

/// 一行站点规则：只属于「当前选中站点」，编辑直接写回站点模型
pub struct SiteRuleRow {
    pub id: u64,
    pub widget: fltk::group::Flex,
    pub inputs: [Input; 3],
    pub arrow: Frame,
    pub del_btn: fltk::button::Button,
    /// 这条规则在所属站点 rules 里的下标（删掉前面的行后由剩余行各自更新）
    pub pos: Rc<Cell<usize>>,
}

pub type SiteRuleModel = Rc<RefCell<Vec<SiteRuleRow>>>;

/// 一个运行中的实例：(命令通道, 生效配置指纹)。
/// 代理和站点各持一个 —— 两种模式完全独立启停，互不影响
type RunSlot = Rc<RefCell<Option<(UnboundedSender<Command>, String)>>>;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn new_model() -> Model {
    Rc::new(RefCell::new(Vec::new()))
}

pub fn new_run_slot() -> RunSlot {
    Rc::new(RefCell::new(None))
}

pub fn new_site_model() -> SiteModel {
    Rc::new(RefCell::new(Vec::new()))
}

pub fn new_site_rule_model() -> SiteRuleModel {
    Rc::new(RefCell::new(Vec::new()))
}

pub fn sites_of(model: &SiteModel) -> Vec<Site> {
    model.borrow().iter().map(|e| e.site.clone()).collect()
}

/// 代理模式的重启指纹：只有监听地址变了才需要重启，规则改走热更新
pub fn proxy_signature(c: &AppConfig) -> String {
    c.proxy_listen.clone()
}

/// 镜像站点的重启指纹：站点结构（启用/名称/监听/上游/校验）变了要重绑端口；
/// 只改站点内嵌规则时走热更新，连接不会断
pub fn sites_signature(c: &AppConfig) -> String {
    let mut s = String::new();
    for site in c.sites.iter().filter(|s| s.enable) {
        s.push_str(&format!(
            "{}|{}|{}|{};",
            site.listen_addr, site.upstream, site.insecure, site.name
        ));
    }
    s
}

/// 用系统默认浏览器打开本地站点地址
pub fn open_url(url: &str) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
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

/// 追加一行站点（与规则行同样的增量策略）
pub fn add_site(
    v: &View,
    pack: &Pack,
    model: &SiteModel,
    site_rule_model: &SiteRuleModel,
    site: Site,
) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let idx = model.borrow().len() as i32;
    let w = view::add_site_row_widgets(pack, view::current_pal(), idx, &site);

    // field: 0=name 1=listen_addr 2=upstream
    for (inp, field) in [(&w.name, 0), (&w.listen, 1), (&w.upstream, 2)] {
        let m = model.clone();
        let v2 = v.clone_ref();
        let mut inp = inp.clone();
        inp.set_trigger(CallbackTrigger::Changed);
        inp.set_callback(move |i| {
            site_set(&m, id, field, &i.value());
            // 改了名字，规则区标题里的站点名要跟着变
            if selected_site() == id {
                refresh_site_selection(&v2, &m);
            }
        });
    }
    // 点行内任意位置都算选中这个站点：空白区走行的 callback，
    // 输入框靠焦点轮询（点进去就聚焦），复选框/按钮在这里顺手带走
    for (cb, insecure) in [(&w.enable, false), (&w.insecure, true)] {
        let m = model.clone();
        let v2 = v.clone_ref();
        let rm = site_rule_model.clone();
        let mut cb = cb.clone();
        cb.set_callback(move |c| {
            if let Some(e) = m.borrow_mut().iter_mut().find(|e| e.id == id) {
                if insecure {
                    e.site.insecure = c.is_checked();
                } else {
                    e.site.enable = c.is_checked();
                }
            }
            select_site(&v2, &m, &rm, Some(id));
            focus_site_input(&m, id);
        });
    }
    {
        // 打开时现读模型，改了地址不用重建行
        let m = model.clone();
        let v2 = v.clone_ref();
        let rm = site_rule_model.clone();
        let mut b = w.open_btn.clone();
        b.set_callback(move |_| {
            let addr = m
                .borrow()
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.site.listen_addr.clone());
            select_site(&v2, &m, &rm, Some(id));
            if let Some(addr) = addr {
                open_url(&format!("http://{addr}"));
            }
        });
    }
    {
        let m = model.clone();
        let pack = pack.clone();
        let v2 = v.clone_ref();
        let site_rule_model = site_rule_model.clone();
        let mut b = w.del_btn.clone();
        b.set_callback(move |_| {
            let (removed, pos) = {
                let mut m = m.borrow_mut();
                let Some(pos) = m.iter().position(|e| e.id == id) else {
                    return;
                };
                let entry = m.remove(pos);
                for (idx, e) in m.iter().enumerate() {
                    let mut w = e.widget.clone();
                    w.set_pos(0, idx as i32 * (view::SITE_ROW_H + view::SITE_ROW_GAP));
                }
                (entry.widget, pos)
            };
            app::delete_widget(removed);
            resize_site_pack(&pack, &m);
            // 删掉的正是正在编辑的站点时，顺位选同位置那条（没了就选末尾，全删光则清空规则区）
            let next = if selected_site() == id {
                let m2 = m.borrow();
                m2.get(pos.min(m2.len().saturating_sub(1))).map(|e| e.id)
            } else {
                Some(selected_site())
            };
            select_site(&v2, &m, &site_rule_model, next);
        });
    }

    // 列间的点击垫：点在行内空隙上也能选中 (FLTK 的 Group 不处理 FL_PUSH，行容器的 callback 收不到)
    for g in &w.gaps {
        let v2 = v.clone_ref();
        let m = model.clone();
        let rm = site_rule_model.clone();
        let mut g = g.clone();
        g.set_callback(move |_| {
            select_site(&v2, &m, &rm, Some(id));
            focus_site_input(&m, id);
        });
    }

    model.borrow_mut().push(SiteRow {
        id,
        site,
        widget: w.row,
        inputs: [w.name, w.listen, w.upstream],
        enable: w.enable,
        insecure: w.insecure,
        open_btn: w.open_btn,
        del_btn: w.del_btn,
    });
}

/// 点了行的空白处之后，把光标送进「离鼠标最近的那列输入框」，落地就能直接打字。
/// 点的是输入框本身时不用管 —— 焦点本来就跟着鼠标走了。
fn focus_site_input(site_model: &SiteModel, id: u64) {
    let px = app::event_x();
    let m = site_model.borrow();
    let Some(e) = m.iter().find(|e| e.id == id) else {
        return;
    };
    let mut pick = e.inputs[0].clone();
    let mut best = i32::MAX;
    for inp in e.inputs.iter() {
        let d = (px - (inp.x() + inp.w() / 2)).abs();
        if d < best {
            best = d;
            pick = inp.clone();
        }
    }
    pick.take_focus().ok();
}

fn site_set(model: &SiteModel, id: u64, field: usize, val: &str) {
    let mut m = model.borrow_mut();
    let Some(entry) = m.iter_mut().find(|e| e.id == id) else {
        return;
    };
    let site = &mut entry.site;
    match field {
        0 => site.name = val.to_string(),
        1 => site.listen_addr = val.to_string(),
        _ => site.upstream = val.to_string(),
    }
}

pub fn resize_site_pack(pack: &Pack, model: &SiteModel) {
    let mut pack = pack.clone();
    pack.set_size(
        view::SITE_ROW_W,
        model.borrow().len() as i32 * (view::SITE_ROW_H + view::SITE_ROW_GAP),
    );
    app::redraw();
}

// ---- 站点内嵌规则：只显示「当前选中站点」的规则，编辑直接写回站点模型 ----

/// 站点模型里第 idx 个站点的 id（下拉框选中项 → 站点）
pub fn site_id_at(model: &SiteModel, idx: usize) -> Option<u64> {
    model.borrow().get(idx).map(|e| e.id)
}

/// 重建站点规则区（切换站点、增删站点后调用）
pub fn load_site_rules(
    pack: &Pack,
    model: &SiteRuleModel,
    site_model: &SiteModel,
    site_id: Option<u64>,
) {
    clear_site_rule_rows(pack, model);
    let Some(site_id) = site_id else { return };
    let rules = site_model
        .borrow()
        .iter()
        .find(|e| e.id == site_id)
        .map(|e| e.site.rules.clone())
        .unwrap_or_default();
    for (i, rule) in rules.into_iter().enumerate() {
        push_site_rule_row(pack, model, site_model, site_id, i, rule);
    }
    resize_site_rule_pack(pack, model);
}

fn clear_site_rule_rows(pack: &Pack, model: &SiteRuleModel) {
    let rows: Vec<SiteRuleRow> = model.borrow_mut().drain(..).collect();
    for row in rows {
        let mut w = row.widget;
        w.hide(); // delete_widget 是延迟删除，先藏起来免得和新行重叠
        app::delete_widget(w);
    }
    resize_site_rule_pack(pack, model);
}

/// 追加一条站点规则（写进站点模型 + 生成一行控件）
pub fn add_site_rule(pack: &Pack, model: &SiteRuleModel, site_model: &SiteModel, site_id: u64) {
    let rule = SiteRule::default();
    let idx = {
        let mut sm = site_model.borrow_mut();
        let Some(entry) = sm.iter_mut().find(|e| e.id == site_id) else {
            return;
        };
        entry.site.rules.push(rule.clone());
        entry.site.rules.len() - 1
    };
    push_site_rule_row(pack, model, site_model, site_id, idx, rule);
    resize_site_rule_pack(pack, model);
}

fn push_site_rule_row(
    pack: &Pack,
    model: &SiteRuleModel,
    site_model: &SiteModel,
    site_id: u64,
    idx: usize,
    rule: SiteRule,
) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let idx_y = model.borrow().len() as i32;
    let w = view::add_site_rule_row_widgets(pack, view::current_pal(), idx_y, &rule);
    let pos = Rc::new(Cell::new(idx));

    // field: 0=path_prefix 1=forward_addr 2=forward_prefix
    for (inp, field) in [(&w.prefix, 0), (&w.addr, 1), (&w.fwd_prefix, 2)] {
        let sm = site_model.clone();
        let pos = pos.clone();
        let mut inp = inp.clone();
        inp.set_trigger(CallbackTrigger::Changed);
        inp.set_callback(move |i| {
            let val = i.value();
            let p = pos.get();
            if let Some(entry) = sm.borrow_mut().iter_mut().find(|e| e.id == site_id) {
                if let Some(r) = entry.site.rules.get_mut(p) {
                    match field {
                        0 => r.path_prefix = val,
                        1 => r.forward_addr = val,
                        _ => r.forward_prefix = val,
                    }
                }
            }
        });
    }

    {
        let sm = site_model.clone();
        let m = model.clone();
        let pack = pack.clone();
        let pos = pos.clone();
        let mut b = w.del_btn.clone();
        b.set_callback(move |_| {
            let p = pos.get();
            if let Some(entry) = sm.borrow_mut().iter_mut().find(|e| e.id == site_id) {
                if p < entry.site.rules.len() {
                    entry.site.rules.remove(p);
                }
            }
            let entry = {
                let mut m = m.borrow_mut();
                m.iter().position(|e| e.id == id).map(|i| m.remove(i))
            };
            let Some(entry) = entry else { return };
            app::delete_widget(entry.widget);
            // 剩余行上移，同时各自修正下标（被删那条之后的规则都前移一位）
            {
                let m = m.borrow();
                for (i, e) in m.iter().enumerate() {
                    let mut w = e.widget.clone();
                    w.set_pos(0, i as i32 * (view::SITE_RULE_ROW_H + view::SITE_RULE_ROW_GAP));
                    let cur = e.pos.get();
                    if cur > p {
                        e.pos.set(cur - 1);
                    }
                }
            }
            resize_site_rule_pack(&pack, &m);
        });
    }

    model.borrow_mut().push(SiteRuleRow {
        id,
        widget: w.row,
        inputs: [w.prefix, w.addr, w.fwd_prefix],
        arrow: w.arrow,
        del_btn: w.del_btn,
        pos,
    });
}

pub fn resize_site_rule_pack(pack: &Pack, model: &SiteRuleModel) {
    let mut pack = pack.clone();
    pack.set_size(
        view::SITE_RULE_ROW_W,
        model.borrow().len() as i32 * (view::SITE_RULE_ROW_H + view::SITE_RULE_ROW_GAP),
    );
    app::redraw();
}

/// 切换页签：两个页面叠放在 page_area 里，只显示当前页。
/// ponytail: 不能用 Flex 的 fixed=0 隐藏 —— 子控件会被压扁但仍然绘制在容器顶部
pub fn show_tab(v: &View, idx: usize) {
    let area = v.page_area.clone();
    // FLTK 的坐标是相对窗口的绝对值，页面要摆到 page_area 的位置上
    let (x, y, w, h) = (area.x(), area.y(), area.w(), area.h());
    let mut proxy = v.page_proxy.clone();
    let mut site = v.page_site.clone();
    proxy.resize(x, y, w, h);
    site.resize(x, y, w, h);
    if idx == 0 {
        proxy.hide();
        site.show();
    } else {
        site.hide();
        proxy.show();
    }
    proxy.layout();
    site.layout();
    CURRENT_TAB.with(|c| c.set(idx));
    view::refresh_tab_btns(&mut v.tab_btns.clone(), idx, view::current_pal());
    app::redraw();
}

pub fn current_tab() -> usize {
    CURRENT_TAB.with(|c| c.get())
}

/// 任务栏图标 + 界面左上角图标随状态切换：任一模式在跑 = 换色版
fn refresh_icon(win: &mut fltk::window::Window, frame: &mut Frame) {
    let running = PROXY_RUNNING.with(|c| c.get()) || SITES_RUNNING.with(|c| c.get());
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
}

/// 站点在标题里显示的名字：有名称用名称，没名称用序号 + 本地地址
fn site_label(idx: usize, site: &Site) -> String {
    if site.name.trim().is_empty() {
        format!("{}  {}", idx + 1, site.listen_addr)
    } else {
        site.name.clone()
    }
}

/// 站点列表里「正在编辑规则」的那个站点 id（0 = 没选中）
pub fn selected_site() -> u64 {
    SELECTED_SITE.with(|c| c.get())
}

/// 选中一个站点：高亮该行、标题带上站点名、规则区换成它自己的规则。
/// 规则本来每条站点各存一份，这里只是切换显示哪一份。
pub fn select_site(v: &View, site_model: &SiteModel, rule_model: &SiteRuleModel, id: Option<u64>) {
    // 传进来的 id 可能刚被删掉，回落到「没有选中」
    let id = id
        .filter(|id| site_model.borrow().iter().any(|e| e.id == *id))
        .unwrap_or(0);
    // 已经是选中的站点就什么都不做：点击会反复触发，别每次都重建规则行（会丢输入焦点）
    if id == selected_site() {
        return;
    }
    SELECTED_SITE.with(|c| c.set(id));
    refresh_site_selection(v, site_model);
    load_site_rules(
        &v.site_rule_pack,
        rule_model,
        site_model,
        (id != 0).then_some(id),
    );
}

/// 按当前选中态重刷站点行底色和规则区标题（切主题、切语言、增删站点后都要走这里）
pub fn refresh_site_selection(v: &View, site_model: &SiteModel) {
    let pal = view::current_pal();
    let sel = selected_site();
    let mut name = None;
    for (idx, e) in site_model.borrow().iter().enumerate() {
        let on = sel != 0 && e.id == sel;
        e.widget.clone().set_color(if on { pal.row_sel } else { pal.card });
        if on {
            name = Some(site_label(idx, &e.site));
        }
    }
    let mut cap = v.site_rule_cap.clone();
    cap.set_label(&match name {
        Some(n) => format!("{}  ·  {}", t!("site_rules_title"), n),
        None => t!("site_rules_title").to_string(),
    });
    cap.redraw();
    app::redraw();
}

/// 点站点行即切换「正在编辑的站点」。
/// ponytail: FLTK 没有事件冒泡，行里的 Input 会把 FL_PUSH 全吃掉，
/// 而 app::add_handler 只在「没人处理的事件」时才回调，靠不住。
/// 这里分三路收尾：行 Flex 自己的 callback（点空白）、各控件的现成回调（点复选框/按钮）、
/// 定时看焦点（点输入框，输入框一聚焦就说明用户要编这个站点）。
pub fn start_site_row_watch(v: &View, site_model: &SiteModel, site_rule_model: &SiteRuleModel) {
    let v = v.clone_ref();
    let sm = site_model.clone();
    let rm = site_rule_model.clone();
    app::add_timeout3(0.08, move |h| {
        if current_tab() == 0 && v.page_site.clone().visible() {
            let fp = app::focus().map(|w| w.as_widget_ptr() as usize).unwrap_or(0);
            if fp != 0 {
                let hit = sm.borrow().iter().find_map(|e| {
                    // 只认行里的输入框：复选框/按钮有各自的回调
                    e.inputs
                        .iter()
                        .any(|i| i.as_widget_ptr() as usize == fp)
                        .then_some(e.id)
                });
                if let Some(id) = hit {
                    if id != selected_site() {
                        select_site(&v, &sm, &rm, Some(id));
                    }
                }
            }
        }
        app::repeat_timeout3(0.08, h);
    });
}


/// 接线所有按钮回调
pub fn wire(
    v: &mut View,
    model: &Model,
    site_model: &SiteModel,
    site_rule_model: &SiteRuleModel,
    proxy_slot: &RunSlot,
    sites_slot: &RunSlot,
    stats: &Arc<Stats>,
) {
    // 页签切换
    for idx in 0..v.tab_btns.len() {
        let v2 = v.clone_ref();
        v.tab_btns[idx].set_callback(move |_| show_tab(&v2, idx));
    }

    // 代理模式启停：只动代理端口，镜像站点不受影响
    {
        let proxy_slot = proxy_slot.clone();
        let model = model.clone();
        let stats = stats.clone();
        let listen_input = v.listen_input.clone();
        let auto_proxy = v.auto_proxy.clone();
        let mut status = v.proxy_status.clone();
        let mut icon_win = v.win.clone();
        let mut titlebar_icon = v.icon.clone();
        v.proxy_start_btn.set_callback(move |b| {
            let mut slot = proxy_slot.borrow_mut();
            if let Some((tx, _)) = slot.take() {
                let _ = tx.send(Command::Shutdown);
                if auto_proxy.is_checked() {
                    match sysproxy::disable() {
                        Ok(()) => tracing::info!("{}", t!("log.sys_proxy_off")),
                        Err(e) => tracing::error!("{}", t!("log.sys_proxy_off_fail", e = e.to_string())),
                    }
                }
                PROXY_RUNNING.with(|c| c.set(false));
                status.set_label(&t!("status_stopped"));
                status.set_label_color(view::current_pal().subtext);
                b.set_label(&t!("start"));
            } else {
                let mut config = AppConfig::init().unwrap_or_default();
                config.proxy_listen = listen_input.value();
                config.proxy_rules = rules_of(&model);
                let addr = config.proxy_listen.clone();
                *slot = Some((
                    daemon::spawn_proxy(config.clone(), stats.clone()),
                    proxy_signature(&config),
                ));
                if auto_proxy.is_checked() {
                    match sysproxy::enable(&addr) {
                        Ok(()) => tracing::info!("{}", t!("log.sys_proxy_on", addr = addr)),
                        Err(e) => tracing::error!("{}", t!("log.sys_proxy_on_fail", e = e.to_string())),
                    }
                }
                PROXY_RUNNING.with(|c| c.set(true));
                status.set_label(&t!("status_running"));
                status.set_label_color(view::current_pal().ok);
                b.set_label(&t!("stop"));
            }
            refresh_icon(&mut icon_win, &mut titlebar_icon);
        });
    }

    // 镜像站点启停：只动站点端口，代理模式不受影响
    {
        let sites_slot = sites_slot.clone();
        let site_model = site_model.clone();
        let stats = stats.clone();
        let mut status = v.site_status.clone();
        let mut icon_win = v.win.clone();
        let mut titlebar_icon = v.icon.clone();
        v.site_start_btn.set_callback(move |b| {
            let mut slot = sites_slot.borrow_mut();
            if let Some((tx, _)) = slot.take() {
                let _ = tx.send(Command::Shutdown);
                SITES_RUNNING.with(|c| c.set(false));
                status.set_label(&t!("status_stopped"));
                status.set_label_color(view::current_pal().subtext);
                b.set_label(&t!("start"));
            } else {
                let enabled: Vec<Site> = site_model
                    .borrow()
                    .iter()
                    .filter(|e| e.site.enable)
                    .map(|e| e.site.clone())
                    .collect();
                if enabled.is_empty() {
                    tracing::warn!("{}", t!("log.no_site_enabled"));
                    return;
                }
                let mut config = AppConfig::init().unwrap_or_default();
                config.sites = sites_of(&site_model);
                *slot = Some((
                    daemon::spawn_sites(enabled, stats.clone()),
                    sites_signature(&config),
                ));
                SITES_RUNNING.with(|c| c.set(true));
                status.set_label(&t!("status_running"));
                status.set_label_color(view::current_pal().ok);
                b.set_label(&t!("stop"));
            }
            refresh_icon(&mut icon_win, &mut titlebar_icon);
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

    // 两页共用的「界面当前状态 → 配置」快照
    let collect_config: Rc<dyn Fn() -> AppConfig> = Rc::new({
        let model = model.clone();
        let site_model = site_model.clone();
        let listen_input = v.listen_input.clone();
        let auto_proxy = v.auto_proxy.clone();
        move || {
            let mut config = AppConfig::init().unwrap_or_default();
            config.proxy_listen = listen_input.value();
            config.proxy_rules = rules_of(&model);
            config.sites = sites_of(&site_model);
            config.auto_proxy = auto_proxy.is_checked();
            config
        }
    });

    // 添加代理规则
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

    // 添加站点
    {
        let v2 = v.clone_ref();
        let site_model = site_model.clone();
        let site_rule_model = site_rule_model.clone();
        let pack = v.site_pack.clone();
        v.add_site_btn.set_callback(move |_| {
            let mut site = Site::default();
            site.enable = true;
            // 端口按已有条目递增，省得手动改
            let n = site_model.borrow().len();
            site.listen_addr = format!("127.0.0.1:{}", 8088 + n);
            add_site(&v2, &pack, &site_model, &site_rule_model, site);
            resize_site_pack(&pack, &site_model);
            // 新加的站点直接选中，方便马上配它的规则
            let id = site_model.borrow().last().map(|e| e.id);
            select_site(&v2, &site_model, &site_rule_model, id);
        });
    }

    // 给当前选中的站点添加一条转发规则
    {
        let v2 = v.clone_ref();
        let site_model = site_model.clone();
        let site_rule_model = site_rule_model.clone();
        v.add_site_rule_btn.set_callback(move |_| {
            let sel = selected_site();
            if sel == 0 {
                tracing::warn!("{}", t!("log.no_site_selected"));
                return;
            }
            add_site_rule(&v2.site_rule_pack, &site_rule_model, &site_model, sel);
        });
    }

    // 代理页：保存并生效（只影响代理模式）
    {
        let collect_config = collect_config.clone();
        let proxy_slot = proxy_slot.clone();
        let stats = stats.clone();
        v.save_btn.set_callback(move |_| {
            let config = collect_config();
            if let Err(e) = config.save() {
                tracing::error!("{}", t!("log.save_fail", e = e.to_string()));
                return;
            }
            tracing::info!("{}", t!("log.saved"));
            let mut slot = proxy_slot.borrow_mut();
            let Some((tx, sig)) = slot.as_mut() else { return };
            if *sig == proxy_signature(&config) {
                let _ = tx.send(Command::SetProxyRules(config.proxy_rules.clone()));
            } else {
                tracing::info!("{}", t!("log.addr_changed_restart"));
                let _ = tx.send(Command::Shutdown);
                *slot = Some((
                    daemon::spawn_proxy(config.clone(), stats.clone()),
                    proxy_signature(&config),
                ));
            }
        });
    }

    // 站点页：保存并生效（只影响镜像站点）
    {
        let collect_config = collect_config.clone();
        let sites_slot = sites_slot.clone();
        let stats = stats.clone();
        v.site_save_btn.set_callback(move |_| {
            let config = collect_config();
            if let Err(e) = config.save() {
                tracing::error!("{}", t!("log.save_fail", e = e.to_string()));
                return;
            }
            tracing::info!("{}", t!("log.saved"));
            let enabled: Vec<Site> =
                config.sites.iter().filter(|s| s.enable).cloned().collect();
            let mut slot = sites_slot.borrow_mut();
            let Some((tx, sig)) = slot.as_mut() else { return };
            if *sig == sites_signature(&config) {
                let _ = tx.send(Command::SetSiteRules(enabled));
            } else {
                tracing::info!("{}", t!("log.addr_changed_restart"));
                let _ = tx.send(Command::Shutdown);
                *slot = Some((
                    daemon::spawn_sites(enabled, stats.clone()),
                    sites_signature(&config),
                ));
            }
        });
    }

    // 关窗（✕ 按钮 / 任务栏右键关闭）：恢复系统代理后退出
    {
        let cleanup: Rc<dyn Fn()> = Rc::new({
            let proxy_slot = proxy_slot.clone();
            let auto_proxy = v.auto_proxy.clone();
            move || {
                // 只有代理模式真的起来过，才可能被我们改过系统代理
                if proxy_slot.borrow().is_some() && auto_proxy.is_checked() {
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
        let site_model = site_model.clone();
        let site_rule_model = site_rule_model.clone();
        v.theme_btns[idx].set_callback(move |_| {
            let theme = ["system", "dark", "light"][idx];
            view::set_theme_sel(idx);
            let dark = match theme {
                "dark" => true,
                "light" => false,
                _ => matches!(dark_light::detect(), Ok(dark_light::Mode::Dark)),
            };
            apply_theme(&v2, &model, &site_model, &site_rule_model, dark);
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
        let site_model = site_model.clone();
        let site_rule_model = site_rule_model.clone();
        let proxy_slot = proxy_slot.clone();
        let sites_slot = sites_slot.clone();
        let stats = stats.clone();
        let v2 = v.clone_ref();
        v.lang_btn.set_callback(move |_| {
            let next = if &*rust_i18n::locale() == "zh" { "en" } else { "zh" };
            set_locale(next);
            let proxy_running = proxy_slot.borrow().is_some();
            let sites_running = sites_slot.borrow().is_some();
            refresh_i18n(
                &v2,
                &model,
                &site_model,
                &site_rule_model,
                proxy_running,
                sites_running,
                &stats.snapshot(),
            );
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
pub fn start_cursor_blink(
    model: Model,
    site_rule_model: SiteRuleModel,
    listen_input: Input,
) {
    let mut visible = true;
    app::add_timeout3(0.53, move |h| {
        let pal = view::current_pal();
        visible = !visible;
        let m = model.borrow();
        let sr = site_rule_model.borrow();
        let fp = app::focus().map(|w| w.as_widget_ptr() as usize).unwrap_or(0);
        // 只触摸当前仍在模型里的输入框，删除后的行天然跳过，无悬垂指针
        let inputs = m
            .iter()
            .flat_map(|e| e.inputs.iter().cloned())
            .chain(sr.iter().flat_map(|e| e.inputs.iter().cloned()))
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
pub fn apply_theme(
    v: &View,
    model: &Model,
    site_model: &SiteModel,
    site_rule_model: &SiteRuleModel,
    dark: bool,
) {
    // 先重刷全局色与框型（复选框框体/勾选、输入框等系统件都依赖它）
    let pal = crate::ui::apply_global_style(dark);
    view::set_current_pal(pal);
    let sel = view::current_theme_sel();

    v.win.clone().set_color(pal.border);
    v.col.clone().set_color(pal.bg);
    v.title_bar.clone().set_color(pal.bg);
    v.title.clone().set_label_color(pal.text);
    // 卡片
    for c in [
        &v.header,
        &v.stats_card,
        &v.log_card,
        &v.proxy_bar,
        &v.site_bar,
        &v.rules_card,
        &v.sites_card,
        &v.site_rules_card,
    ] {
        c.clone().set_color(pal.card);
    }
    // 页面容器本身是窗口底色
    v.page_area.clone().set_color(pal.bg);
    for c in [&v.page_proxy, &v.page_site] {
        c.clone().set_color(pal.bg);
    }
    // 状态灯颜色跟随各自的运行状态
    v.proxy_status.clone().set_label_color(if PROXY_RUNNING.with(|c| c.get()) {
        pal.ok
    } else {
        pal.subtext
    });
    v.site_status.clone().set_label_color(if SITES_RUNNING.with(|c| c.get()) {
        pal.ok
    } else {
        pal.subtext
    });
    // 标题栏按钮恢复常态色（hover 配色事件里现读 current_pal，不用管）
    for b in [&v.min_btn, &v.close_btn] {
        let mut b = b.clone();
        b.set_color(pal.bg);
        b.set_label_color(pal.subtext);
        b.redraw();
    }
    // 页签 + 主题分段控件
    view::refresh_tab_btns(&mut v.tab_btns.clone(), current_tab(), pal);
    v.tab_bar.clone().set_color(pal.seg_bg);
    v.seg.clone().set_color(pal.seg_bg);
    view::refresh_theme_btns(&mut v.theme_btns.clone(), sel, pal);
    view::style_ghost_btn(&mut v.lang_btn.clone(), pal);
    // 统计栏
    for f in &v.stats {
        f.clone().set_label_color(pal.subtext);
    }
    // 代理模式页
    v.addr_label.clone().set_label_color(pal.subtext);
    view::style_input(&mut v.listen_input.clone(), pal);
    view::style_primary_btn(&mut v.proxy_start_btn.clone(), pal);
    {
        let mut ap = v.auto_proxy.clone();
        ap.set_label_color(pal.text);
        ap.set_color(pal.input_bg);
        ap.set_selection_color(pal.accent);
    }
    v.rule_cap.clone().set_label_color(pal.text);
    for f in &v.col_header_frames {
        f.clone().set_label_color(pal.subtext);
    }
    view::style_ghost_btn(&mut v.add_btn.clone(), pal);
    view::style_primary_btn(&mut v.save_btn.clone(), pal);
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
    // 镜像站点页
    v.site_hint.clone().set_label_color(pal.subtext);
    view::style_primary_btn(&mut v.site_start_btn.clone(), pal);
    v.site_cap.clone().set_label_color(pal.text);
    for f in &v.site_col_header_frames {
        f.clone().set_label_color(pal.subtext);
    }
    view::style_ghost_btn(&mut v.add_site_btn.clone(), pal);
    for e in site_model.borrow().iter() {
        for inp in &e.inputs {
            view::style_input(&mut inp.clone(), pal);
        }
        for mut cb in [e.enable.clone(), e.insecure.clone()] {
            cb.set_label_color(pal.subtext);
            cb.set_color(pal.input_bg);
            cb.set_selection_color(pal.accent);
        }
        view::style_ghost_btn(&mut e.open_btn.clone(), pal);
        let mut d = e.del_btn.clone();
        d.set_color(pal.input_bg);
        d.set_label_color(pal.danger);
    }
    v.site_rule_cap.clone().set_label_color(pal.text);
    for f in &v.site_rule_header_frames {
        f.clone().set_label_color(pal.subtext);
    }
    view::style_ghost_btn(&mut v.add_site_rule_btn.clone(), pal);
    view::style_primary_btn(&mut v.site_save_btn.clone(), pal);
    for e in site_rule_model.borrow().iter() {
        e.widget.clone().set_color(pal.card);
        for inp in &e.inputs {
            view::style_input(&mut inp.clone(), pal);
        }
        e.arrow.clone().set_label_color(pal.subtext);
        let mut d = e.del_btn.clone();
        d.set_color(pal.input_bg);
        d.set_label_color(pal.danger);
    }
    // 滚动条
    for s in [&v.scroll, &v.site_scroll, &v.site_rule_scroll] {
        view::style_scroll(&mut s.clone(), pal);
    }
    // 站点行的选中底色 + 规则区标题（跟着调色板走，别漏）
    refresh_site_selection(v, site_model);
    // 日志区
    v.log_cap.clone().set_label_color(pal.text);
    let mut ld = v.log_disp.clone();
    ld.set_color(pal.input_bg);
    ld.set_text_color(pal.text);
    app::redraw();
}

/// 运行中切换语言：重刷所有静态文案 + 当前动态文案
fn refresh_i18n(
    v: &View,
    model: &Model,
    site_model: &SiteModel,
    site_rule_model: &SiteRuleModel,
    proxy_running: bool,
    sites_running: bool,
    s: &crate::core::stats::StatsSnapshot,
) {
    v.win.clone().set_label(&t!("app_title"));
    v.title.clone().set_label(&t!("app_title"));
    // 页签
    for (b, key) in v.tab_btns.iter().zip(["tab_sites", "tab_proxy"]) {
        b.clone().set_label(&t!(key));
    }
    view::refresh_tab_btns(&mut v.tab_btns.clone(), current_tab(), view::current_pal());
    // 代理模式页
    v.addr_label.clone().set_label(&t!("listen_addr"));
    v.proxy_start_btn
        .clone()
        .set_label(&t!(if proxy_running { "stop" } else { "start" }));
    v.proxy_status
        .clone()
        .set_label(&t!(if proxy_running { "status_running" } else { "status_stopped" }));
    v.auto_proxy.clone().set_label(&t!("sys_proxy"));
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
    for e in model.borrow().iter() {
        e.del_btn.clone().set_label(&t!("rule_delete"));
    }
    // 镜像站点页
    v.site_hint.clone().set_label(&t!("site_hint"));
    v.site_start_btn
        .clone()
        .set_label(&t!(if sites_running { "stop" } else { "start" }));
    v.site_status
        .clone()
        .set_label(&t!(if sites_running { "status_running" } else { "status_stopped" }));
    v.site_cap.clone().set_label(&t!("sites_title"));
    for (f, key) in v.site_col_header_frames.iter().zip([
        "col_enable",
        "col_site_name",
        "col_site_listen",
        "col_site_upstream",
        "col_site_insecure",
        "",
    ]) {
        if !key.is_empty() {
            f.clone().set_label(&t!(key));
        }
    }
    v.add_site_btn.clone().set_label(&t!("add_site"));
    for e in site_model.borrow().iter() {
        e.open_btn.clone().set_label(&t!("site_open"));
        e.del_btn.clone().set_label(&t!("rule_delete"));
        e.insecure.clone().set_label(&t!("site_insecure"));
    }
    // 站点规则区
    for (f, key) in v.site_rule_header_frames.iter().zip([
        "col_rule_prefix",
        "",
        "col_rule_forward",
        "col_rule_fwd_prefix",
        "",
    ]) {
        if !key.is_empty() {
            f.clone().set_label(&t!(key));
        }
    }
    v.add_site_rule_btn.clone().set_label(&t!("add_rule"));
    v.site_save_btn.clone().set_label(&t!("save_apply"));
    for e in site_rule_model.borrow().iter() {
        e.del_btn.clone().set_label(&t!("rule_delete"));
    }
    // 标题里带站点名，随语言重刷
    refresh_site_selection(v, site_model);
    // 主题 / 语言
    for (b, key) in v.theme_btns.iter().zip(["theme_system", "theme_dark", "theme_light"]) {
        b.clone().set_label(&t!(key));
    }
    view::refresh_theme_btns(&mut v.theme_btns.clone(), view::current_theme_sel(), view::current_pal());
    v.lang_btn.clone().set_label(&t!("lang_switch"));
    // 统计栏
    let stats = [(&v.stats[0], "stats_active", format!("{}", s.active)), (&v.stats[1], "stats_total", format!("{}", s.total))];
    for (f, key, val) in stats {
        f.clone().set_label(&format!("{}  {}", t!(key), val));
    }
    v.stats[2].clone().set_label(&format!("{}  {}", t!("stats_up"), fmt_bytes(s.bytes_up)));
    v.stats[3].clone().set_label(&format!("{}  {}", t!("stats_down"), fmt_bytes(s.bytes_down)));
    v.log_cap.clone().set_label(&t!("log_title"));
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
