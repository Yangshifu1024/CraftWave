//! 界面缩放（gpui-rewrite 阶段 2.3，spec 的 `ui_zoom`）。
//!
//! ## 模式（theme.md §2）
//!
//! gpui 没有浏览器那种整窗缩放，CraftWave 的缩放落在字号体系：
//! `Z(px) = round(14 × zoom)` 是全局字号基准，`Theme::font_size`（rem 的
//! 源头）取 Z，所有 UI 字号经 `font(offset) = round(Z + offset)` 取值——
//! 切档后全局字号重算、rem 随动、组件整体放大。
//!
//! ## 四档（spec 2.3「缩放四档」，端点落在旧栈滑杆 12–20px 范围内）
//!
//! | 档位 | zoom | Z |
//! |---|---|---|
//! | 小 | 0.85 | 12px |
//! | 标准（缺省） | 1.00 | 14px |
//! | 大 | 1.15 | 16px |
//! | 特大 | 1.30 | 18px |
//!
//! ⌘1/2/3/4 直选四档（循环式不如直选可预期，且菜单里四项带勾选语义）。
//!
//! ## 档位状态与持久化
//!
//! zoom 状态住 [`theme::CraftwaveTheme::zoom`]（theme.md §11.1「zoom: f32」），
//! 偏好落 `~/.craftwave/ui-prefs.json`（壳自管域，见 prefs.rs 模块文档——
//! 不走 engine 的 get/set_ui_state：那需要整套 AppState 装配，且旧栈里
//! ui-state 本就是前端自管域）。
//!
//! ## traffic light 与标题栏随动（spec 2.3 验收项）
//!
//! 灯本身是系统控件不随缩放变大，但界面整体放大后侧栏中轴/顶部净空按倍数
//! 挪位：开窗时按当前档计算 `traffic_light_position`，换档后经
//! [`move_traffic_lights`] 挪到新位置（macOS 专属 API）。标题栏（TitleBar
//! 组件）高度是组件常量不随动，栏内文案字号走字阶 → 视觉随缩放变大。

use gpui_kit::{App, KeyBinding, Pixels, Point, Window, actions, point, px};

use crate::prefs;
use crate::theme::{self, ActiveCraftwave as _, CraftwaveTheme};

actions!(
    craftwave,
    [ZoomSmall, ZoomStandard, ZoomLarge, ZoomXLarge]
);

/// 四档缩放倍率（下标即 ⌘1–⌘4 的档序）。
pub const LEVELS: [f32; 4] = [0.85, 1.00, 1.15, 1.30];
/// 缺省档：标准（1.00）。
pub const DEFAULT_IX: usize = 1;
/// 偏好存档文件名。
const PREF_FILE: &str = "ui-prefs.json";
/// 100% 档的 traffic light 位置（gpui-component TitleBar 默认值，见其
/// `title_bar_options()`）；其他档按缩放倍率挪位。
const LIGHTS_AT_100: f32 = 9.0;

/// ui-prefs.json 的落盘形状（字段缺省可读——手改偏好文件删字段不算损坏）。
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct UiPrefs {
    zoom: Option<f32>,
}

/// 当前缩放倍率。
pub fn current(cx: &App) -> f32 {
    cx.craftwave().zoom
}

/// 当前档序（菜单勾选等需要档序的场合）。
#[allow(dead_code)]
pub fn current_ix(cx: &App) -> usize {
    let zoom = current(cx);
    LEVELS
        .iter()
        .position(|&level| level == zoom)
        .unwrap_or(DEFAULT_IX)
}

/// 启动读档：必须在首次 [`theme::apply`] 与开任何窗之前（字号与 traffic
/// light 位置都从 zoom 算出）。读不到/值不在档位表 → 回标准档。
pub fn init(cx: &mut App) {
    let saved = prefs::read_json::<UiPrefs>(&prefs::file(PREF_FILE))
        .and_then(|p| p.zoom)
        .filter(|z| LEVELS.contains(z));
    let zoom = saved.unwrap_or(LEVELS[DEFAULT_IX]);
    cx.global_mut::<CraftwaveTheme>().zoom = zoom;
}

/// 只认档位表里的值：手改的偏好文件与去掉了的档位都退回默认。
fn persist(zoom: f32) {
    prefs::write_json(
        &prefs::file(PREF_FILE),
        &UiPrefs {
            zoom: Some(zoom),
        },
    );
}

/// 换档内核：更新 zoom 状态 → 重算全局字号（rem 失效）→ 刷新全部窗口。
/// 不落盘（测试装置走这条；生产入口 [`set_level`] 负责持久化）。
///
/// 变高列表 remeasure 钩子（spec 2.3）：阶段 2.4 消息流接入后，消息行的
/// 测高缓存按 zoom 代次失效——做法是列表侧记 `seen_zoom`，render 每帧
/// `current(cx) != seen` 时重测（Wake `ui_zoom::changed` 同款）。
/// 本阶段无列表，留此接口注释即可。
pub fn apply_zoom(zoom: f32, cx: &mut App) {
    if zoom == current(cx) {
        return;
    }
    theme::set_zoom(zoom, cx);
}

/// 生产换档入口：落盘 → 换档。落盘失败不阻断换档（存档可丢，
/// 下次启动回标准档而已）。
pub fn set_level(ix: usize, cx: &mut App) {
    let Some(zoom) = LEVELS.get(ix).copied() else {
        return;
    };
    persist(zoom);
    apply_zoom(zoom, cx);
}

/// 四个缩放 action 挂**全局**监听器：不管焦点在哪都得生效（Wake 同款
/// 结论——挂在窗口根上，焦点一旦不在根的子树里快捷键就失灵，而缩放恰恰
/// 是版式乱掉时用户最需要的恢复手段）。代价是菜单项到头不置灰，到头时
/// 什么也不做（同档早退）。
pub fn register(cx: &mut App) {
    cx.on_action(move |_: &ZoomSmall, cx| set_level(0, cx));
    cx.on_action(move |_: &ZoomStandard, cx| set_level(1, cx));
    cx.on_action(move |_: &ZoomLarge, cx| set_level(2, cx));
    cx.on_action(move |_: &ZoomXLarge, cx| set_level(3, cx));
}

/// ⌘1–⌘4 键位（无 key context：全局生效，与 register 的全局监听器配套）。
/// 前缀用 `cmd-` 的原因见 main.rs `key_bindings` 的注释（`secondary-` 在
/// gpui-pre 0.3.10 的 KeyBinding::new 路径下构造即 panic）。
pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("cmd-1", ZoomSmall, None),
        KeyBinding::new("cmd-2", ZoomStandard, None),
        KeyBinding::new("cmd-3", ZoomLarge, None),
        KeyBinding::new("cmd-4", ZoomXLarge, None),
    ]
}

/// 缩放菜单四项的显示形制（"85%" 等，menu 标签 + 勾选态由当前档决定，
/// 展开 in main.rs 的 View 菜单）。
pub fn level_label(ix: usize) -> String {
    match LEVELS.get(ix) {
        Some(&zoom) => format!("{}%", (zoom * 100.0).round() as i32),
        None => String::new(),
    }
}

/// macOS traffic light 位置：100% 档 = gpui-component TitleBar 默认 (9,9)，
/// 其他档按缩放倍率挪位（顶部净空与侧栏中轴同比例放大，灯压回中轴）。
pub fn traffic_light_position(zoom: f32) -> Point<Pixels> {
    point(px(LIGHTS_AT_100 * zoom), px(LIGHTS_AT_100 * zoom))
}

/// 开窗时按当前档给 TitlebarOptions 用的位置（与
/// `TitleBar::title_bar_options()` 同源，仅位置随档）。
pub fn traffic_lights_now(cx: &App) -> Point<Pixels> {
    traffic_light_position(current(cx))
}

/// 换档后把已开窗口的 traffic light 挪到新位置（字号/圆角由
/// theme::set_zoom 内的窗口刷新负责，灯是唯一要单独跟动的）。只有 macOS
/// 有这个 API（gpui 在别的平台不编译它，`cfg!` 表达式也会被类型检查，
/// 所以必须是属性）。
pub fn move_traffic_lights(window: &mut Window, cx: &App) {
    let position = traffic_lights_now(cx);
    #[cfg(target_os = "macos")]
    window.set_traffic_light_position(position);
    #[cfg(not(target_os = "macos"))]
    let _ = (window, position);
}

/// 换过缩放档没有：`seen` 是视图上次处理时的档位，变了记下新档并返回 true。
/// 缓存了按旧倍数量出的尺寸的地方（变高列表行高、traffic light 坐标）在
/// render 里每帧问一次（整数比较，未换档零开销）。
pub fn changed(seen: &mut f32, cx: &App) -> bool {
    let now = current(cx);
    if *seen == now {
        return false;
    }
    *seen = now;
    true
}

/// 视图 render 每帧调的换档随动口（Wake `sync_zoom` 同款位置）：traffic
/// light 挪到新档的位置；变高列表 remeasure 钩子（spec 2.3）在阶段 2.4
/// 消息流接入后挂这里——ListState 行高缓存按 zoom 代次重测。
pub fn sync_zoom(seen: &mut f32, window: &mut Window, cx: &App) {
    if changed(seen, cx) {
        move_traffic_lights(window, cx);
    }
}
