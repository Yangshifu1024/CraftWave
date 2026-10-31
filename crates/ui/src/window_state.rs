//! 主窗几何记忆（gpui-rewrite 阶段 2.1）：关掉再开回到原位原尺寸。
//!
//! 字段语义与保存/恢复时机照旧壳 `src-tauri/src/host/window_state.rs`
//! （`{ version, x, y, width, height, maximized }`，逻辑像素；损坏/缺字段/
//! 版本不符 → 整体丢弃回默认；越界校验：与任一显示器的可见交集 ≥
//! 120×80 才恢复），但实现自包含在本 crate——不依赖 tauri 壳，几何观察、
//! 防抖落盘、恢复校验全部走 gpui API。
//!
//! 与旧壳的两处平台性差异（照 Wake `main_window.rs` 的结论）：
//!
//! - gpui 的 `window.bounds()`/开窗 `window_bounds` 在 macOS 上是**所在
//!   屏幕内的相对坐标**（Windows/X11 是虚拟桌面全局坐标）。本阶段骨架
//!   只做单文件简化：落盘直接存 bounds 原值，恢复时与各显示器的可见区
//!   求交校验后**夹进命中的屏幕**——多屏坐标精确还原（屏内相对坐标 +
//!   display uuid）留到阶段 4 桌面集成（三平台打包时一并做），届时按
//!   Wake 的 (display uuid, 屏内相对 bounds) 方案升级。
//! - 关窗即退出（spec 阶段 4.6 之前的默认）：退出前 [`flush`] 强写，
//!   不存在「关窗驻留、进程存活」的中间态。
//!
//! 保存时机（FR 语义：退出不丢最后一帧几何）：主窗 `observe_window_bounds`
//! 更新缓存 + 防抖 500ms 落盘（拖动窗口事件高频触发，只保留最后一次）；
//! `on_app_quit` / 关窗回调里同步强写。

use std::time::Duration;

use gpui_kit::{App, Context, Global, Pixels, Task, Window, size, px};
use serde::{Deserialize, Serialize};

use crate::prefs;
use crate::MainWindowView;

/// 落盘格式版本（结构变化时递增；不匹配按损坏处理整体丢弃）。
pub const STATE_VERSION: u32 = 1;
/// 越界校验的最小可见交集宽（逻辑像素，回写口径；旧壳同值）。
pub const MIN_VISIBLE_WIDTH: f32 = 120.0;
/// 越界校验的最小可见交集高（逻辑像素，回写口径；旧壳同值）。
pub const MIN_VISIBLE_HEIGHT: f32 = 80.0;
/// Moved/Resized 事件的落盘防抖窗口（旧壳同值）。
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);
/// 存档文件名（壳侧自管，见 prefs.rs）。
const STATE_FILE: &str = "window-state.json";
/// 首次启动的主窗默认尺寸。
pub const MAIN_SIZE: (f32, f32) = (1200.0, 800.0);
/// 主窗最小尺寸（开窗那一刻生效）。
pub const MAIN_MIN_SIZE: (f32, f32) = (960.0, 620.0);

/// window-state.json 的落盘形状（逻辑像素；字段全必填——缺任何一个都视为
/// 损坏文件整体丢弃，不做半份恢复；语义对齐旧壳 `WindowState`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub version: u32,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            x: 0.0,
            y: 0.0,
            width: MAIN_SIZE.0,
            height: MAIN_SIZE.1,
            maximized: false,
        }
    }
}

/// 读取几何文件：不存在/损坏（非法 JSON、缺字段、version 不符、宽高非正）
/// 一律 `None`（整体丢弃回默认），不区分错误种类（调用方语义完全一致）。
/// 纯函数，测试直接覆盖。
pub fn load_state(bytes: &[u8]) -> Option<WindowState> {
    let state: WindowState = serde_json::from_slice(bytes).ok()?;
    if state.version != STATE_VERSION {
        return None;
    }
    // 宽高必须为正：零/负尺寸的"恢复"等于把窗口做没，宁可回默认
    if state.width <= 0.0 || state.height <= 0.0 {
        return None;
    }
    Some(state)
}

/// 越界校验（回写口径的纯几何函数，照旧壳 `is_recoverable`）：窗口矩形与
/// 任一显示器的可见交集宽高都达到 [`MIN_VISIBLE_WIDTH`]×[`MIN_VISIBLE_HEIGHT`]
/// 即视为可恢复。"可见交集"按矩形重叠取正部计算；完全不重叠时交集为零
/// 自然不达标。单测表驱动覆盖。
pub fn is_recoverable(rect: (f32, f32, f32, f32), monitors: &[(f32, f32, f32, f32)]) -> bool {
    let (x, y, width, height) = rect;
    monitors.iter().any(|&(mx, my, mw, mh)| {
        let left = x.max(mx);
        let top = y.max(my);
        let right = (x + width).min(mx + mw);
        let bottom = (y + height).min(my + mh);
        right - left >= MIN_VISIBLE_WIDTH && bottom - top >= MIN_VISIBLE_HEIGHT
    })
}

/// 把窗口矩形夹进屏幕范围：尺寸先按屏幕封顶，再把原点限制在屏内
/// （恢复路径的兜底：换过分辨率也不会开到屏外）。纯函数。
pub fn fit_in(rect: (f32, f32, f32, f32), screen: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    let (rx, ry, rw, rh) = rect;
    let (sx, sy, sw, sh) = screen;
    let width = rw.min(sw);
    let height = rh.min(sh);
    let x = rx.clamp(sx, sx + sw - width);
    let y = ry.clamp(sy, sy + sh - height);
    (x, y, width, height)
}

/// 从显示器清单提取 (x, y, w, h) 逻辑矩形（gpui `Bounds<Pixels>` 三元组
/// 展平，方便与纯函数对接；Pixels 的标量经 From 转换——字段私有不能 `.0`）。
fn display_rect(bounds: gpui_kit::Bounds<Pixels>) -> (f32, f32, f32, f32) {
    (
        f32::from(bounds.origin.x),
        f32::from(bounds.origin.y),
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
    )
}

/// 几何记忆的进程内载体：最新状态缓存 + 防抖任务句柄。
///
/// 缓存承载"最后一次已知几何"——退出/关窗时窗口方法可能已不可用，靠它冲盘；
/// 最大化期间只置位 maximized、几何字段保留还原前值（重启后还原"原来的
/// 窗口"再最大化，旧壳同语义）。
#[derive(Default)]
struct GeometryMemory {
    cache: Option<WindowState>,
    /// 节流落盘任务：在队 ⇔ 快照有未落盘的变化；在队就不再起新的，
    /// 触发时读最新缓存（Wake `MainGeometry` 同款「一次移动只排一个任务」）。
    save_task: Option<Task<()>>,
}

impl Global for GeometryMemory {}

/// 抓取窗口当前几何快照（bounds 的可读性即快照的可读性）。
fn snapshot(window: &Window) -> Option<WindowState> {
    let bounds = window.bounds();
    Some(WindowState {
        version: STATE_VERSION,
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
        maximized: window.is_maximized(),
    })
}

/// 挂到主窗上（开窗构造闭包里调用一次）：立即记一次 + 观察 bounds 变化
/// 防抖落盘。开窗即记：启动后不挪窗直接退出，也有初值可冲盘。
pub fn track(window: &mut Window, cx: &mut Context<MainWindowView>) {
    if !cx.has_global::<GeometryMemory>() {
        cx.set_global(GeometryMemory::default());
    }
    cx.observe_window_bounds(window, |_, window, cx| observe(window, cx))
        .detach();
    observe(window, cx);
}

/// bounds 变化入口：更新缓存 + 排防抖落盘。最大化期间几何字段保留缓存
/// 原值、只置位 maximized（还原语义，旧壳同款）。
fn observe(window: &Window, cx: &mut App) {
    let Some(next) = snapshot(window) else {
        return;
    };
    if !cx.has_global::<GeometryMemory>() {
        cx.set_global(GeometryMemory::default());
    }
    let memory = cx.global_mut::<GeometryMemory>();
    let changed = match memory.cache.as_mut() {
        // 最大化：只置位布尔，几何保留还原前值
        Some(saved) if next.maximized && !saved.maximized => {
            saved.maximized = true;
            true
        }
        Some(saved) if *saved == next => false,
        // 还原出最大化：几何字段照录（此时 bounds 即还原后的 windowed 几何）
        Some(saved) => {
            saved.maximized = next.maximized;
            saved.x = next.x;
            saved.y = next.y;
            saved.width = next.width;
            saved.height = next.height;
            true
        }
        None => {
            memory.cache = Some(next);
            true
        }
    };
    if changed {
        schedule_save(cx);
    }
}

/// 一次移动只排一个任务：500ms 后写当时最新的缓存，期间的事件只改缓存。
fn schedule_save(cx: &mut App) {
    if cx.global::<GeometryMemory>().save_task.is_some() {
        return;
    }
    let task = cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(SAVE_DEBOUNCE)
            .await;
        cx.update(flush);
    });
    cx.global_mut::<GeometryMemory>().save_task = Some(task);
}

/// 有排队中的落盘任务才写（排队 ⇔ 缓存有未落盘的变化），写完即出队。
/// 防抖任务与退出前（on_app_quit / 关窗回调）都走这里。
pub fn flush(cx: &mut App) {
    if !cx.has_global::<GeometryMemory>() {
        return;
    }
    let memory = cx.global_mut::<GeometryMemory>();
    if memory.save_task.take().is_none() {
        return;
    }
    if let Some(state) = memory.cache.clone() {
        prefs::write_json(&prefs::file(STATE_FILE), &state);
    }
}

/// 开主窗的参数：上次落盘 > 主屏居中。读文件 → 越界校验（显示器清单与
/// 可见区交集）→ 夹进屏内；校验不过整体丢弃回默认（居中 1200×800）。
pub fn restore(cx: &App) -> gpui_kit::WindowBounds {
    let default = || {
        gpui_kit::WindowBounds::centered(
            size(px(MAIN_SIZE.0), px(MAIN_SIZE.1)),
            cx,
        )
    };
    let Some(bytes) = std::fs::read(prefs::file(STATE_FILE)).ok() else {
        return default();
    };
    let Some(state) = load_state(&bytes) else {
        eprintln!("[window-state] 几何文件损坏或版本不符，回默认位置");
        return default();
    };
    // 显示器可见区清单读不到属异常环境，保守丢弃恢复
    let monitors: Vec<(f32, f32, f32, f32)> = cx
        .displays()
        .iter()
        .map(|d| display_rect(d.visible_bounds()))
        .collect();
    if monitors.is_empty() {
        eprintln!("[window-state] 显示器清单不可用，回默认位置");
        return default();
    }
    let rect = (state.x, state.y, state.width, state.height);
    if !is_recoverable(rect, &monitors) {
        eprintln!("[window-state] 记录几何越界，回默认位置");
        return default();
    }
    // 夹进第一个命中的屏幕（多屏精确还原见模块头，阶段 4 升级）
    let fitted = monitors
        .iter()
        .find(|m| is_recoverable(rect, std::slice::from_ref(m)))
        .map(|&m| fit_in(rect, m))
        .unwrap_or(rect);
    gpui_kit::WindowBounds::Windowed(gpui_kit::Bounds {
        origin: gpui_kit::point(px(fitted.0), px(fitted.1)),
        size: gpui_kit::size(px(fitted.2), px(fitted.3)),
    })
}
