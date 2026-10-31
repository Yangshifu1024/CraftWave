//! 窗口几何记忆（29 号票，§10.7 回写条目）：关掉再开回到原位原尺寸，
//! 拔显示器也不丢窗。
//!
//! 数据面：`~/.craftwave/window-state.json`，`{ version, x, y, width, height,
//! maximized }`（逻辑像素）。**不进 ui-state.json**——那是前端自管域，且
//! host 必须在亮窗前读取几何；文件损坏/缺字段/版本不符 → 整体丢弃回默认，
//! 不报错（几何是可丢数据，任何异常都不值得阻塞启动）。
//!
//! 保存时机（FR 语义：退出不丢最后一帧几何）：
//! - 主窗 `Moved`/`Resized` 事件：更新缓存 + 防抖 500ms 落盘（拖动窗口时
//!   事件高频触发，逐事件写盘没有意义，只保留最后一次）；
//! - `CloseRequested`：同步强写（防抖窗口内的最后几何此时必须落盘）；
//! - `Destroyed`：窗口方法已不可用，直接把缓存冲盘（终值语义）。
//!
//! maximized 单独记布尔：最大化期间不更新几何字段（保留还原前几何），重启
//! 后恢复还原前几何再 maximize，还原本来的窗口而不是铺满整屏的几何。
//!
//! 恢复时机：tauri setup 阶段——窗口以 `visible:false` 启动尚未亮窗，恢复
//! 先于前端激活协议（§2.1）的 show，天然满足"恢复先于亮窗"。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{LogicalPosition, LogicalSize, Manager, WebviewWindow};

use crate::util::atomic;

/// 落盘格式版本（结构变化时递增；不匹配按损坏处理整体丢弃）。
pub const STATE_VERSION: u32 = 1;
/// 越界校验的最小可见交集宽（逻辑像素，§10.7 回写口径）。
pub const MIN_VISIBLE_WIDTH: f64 = 120.0;
/// 越界校验的最小可见交集高（逻辑像素，§10.7 回写口径）。
pub const MIN_VISIBLE_HEIGHT: f64 = 80.0;
/// Moved/Resized 事件的落盘防抖窗口。
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

/// window-state.json 的落盘形状（逻辑像素；字段全必填——缺任何一个都视为
/// 损坏文件整体丢弃，不做半份恢复）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub version: u32,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
}

impl WindowState {
    /// 逻辑坐标下的窗口矩形（越界判定的入参形态）。
    pub fn rect(&self) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
        }
    }
}

/// 逻辑坐标下的矩形（窗口或显示器 bounds；纯几何域，便于表驱动单测）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// 越界校验（§10.7 回写口径的纯几何函数）：窗口与任一显示器的可见交集
/// 宽高都达到 [`MIN_VISIBLE_WIDTH`]×[`MIN_VISIBLE_HEIGHT`] 即视为可恢复。
/// "可见交集"按矩形重叠取正部计算；完全不重叠时交集为零自然不达标。
pub fn is_recoverable(rect: &Rect, monitors: &[Rect]) -> bool {
    monitors.iter().any(|m| {
        let left = rect.x.max(m.x);
        let top = rect.y.max(m.y);
        let right = (rect.x + rect.width).min(m.x + m.width);
        let bottom = (rect.y + rect.height).min(m.y + m.height);
        right - left >= MIN_VISIBLE_WIDTH && bottom - top >= MIN_VISIBLE_HEIGHT
    })
}

/// 读取几何文件：不存在/损坏（非法 JSON、缺字段、version 不符、宽高非正）
/// 一律 `None`（整体丢弃回默认），不区分错误种类（调用方语义完全一致）。
pub fn load_state(path: &Path) -> Option<WindowState> {
    let bytes = std::fs::read(path).ok()?;
    let state: WindowState = serde_json::from_slice(&bytes).ok()?;
    if state.version != STATE_VERSION {
        return None;
    }
    // 宽高必须为正：零/负尺寸的"恢复"等于把窗口做没，宁可回默认
    if state.width <= 0.0 || state.height <= 0.0 {
        return None;
    }
    Some(state)
}

/// 抓取窗口当前几何快照（逻辑像素）。任一属性读不到（窗口已销毁等）返回
/// `None`，调用方保留缓存原值。
fn snapshot(window: &WebviewWindow) -> Option<WindowState> {
    let scale = window.scale_factor().ok()?;
    let position = window.outer_position().ok()?.to_logical::<f64>(scale);
    let size = window.inner_size().ok()?.to_logical::<f64>(scale);
    Some(WindowState {
        version: STATE_VERSION,
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
        maximized: window.is_maximized().unwrap_or(false),
    })
}

/// 几何记忆的进程内载体：落盘路径 + 最新状态缓存 + 防抖序号。
///
/// 缓存承载"最后一次已知几何"——`Destroyed` 时窗口方法不可用，靠它冲盘；
/// 最大化期间只置位 maximized、几何字段保留还原前值（合并逻辑在
/// [`Self::observe`]）。
pub struct WindowStateStore {
    path: PathBuf,
    cache: Mutex<Option<WindowState>>,
    debounce_seq: AtomicU64,
}

impl WindowStateStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            cache: Mutex::new(None),
            debounce_seq: AtomicU64::new(0),
        }
    }

    /// 从窗口抓取当前几何并入缓存。最大化期间几何字段保留缓存里的还原前
    /// 值、只置位 maximized（§10.7：重启后还原"原来的窗口"再最大化）。
    pub fn observe(&self, window: &WebviewWindow) {
        if window.is_maximized().unwrap_or(false) {
            let mut cache = self
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(state) = cache.as_mut() {
                state.maximized = true;
            }
            return;
        }
        if let Some(state) = snapshot(window) {
            *self
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(state);
        }
    }

    /// 把缓存冲盘（原子写；失败只记警告——几何可丢，不值得惊动用户）。
    pub fn flush(&self) {
        let cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = cache.as_ref() else {
            return;
        };
        let value = serde_json::to_value(state).unwrap_or(serde_json::Value::Null);
        if let Err(err) = atomic::write_json(&self.path, &value) {
            eprintln!("[window-state] 几何落盘失败（忽略）：{err}");
        }
    }

    /// 抓取当前几何并同步强写（CloseRequested 关闭面：防抖窗口里的最后
    /// 几何此时必须落盘，退出不丢最后一帧）。
    pub fn save_now(&self, window: &WebviewWindow) {
        self.observe(window);
        self.flush();
    }

    /// Moved/Resized 事件入口：更新缓存 + 防抖 500ms 落盘。防抖用序号法
    /// （每次事件递增、任务醒来比对——期间又有新事件则新任务接管，本任务
    /// 放弃），比"清定时器"式防抖少一把锁。
    pub fn on_geometry_event(&self, window: &WebviewWindow) {
        self.observe(window);
        let seq = self.debounce_seq.fetch_add(1, Ordering::Relaxed) + 1;
        let app = window.app_handle().clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(SAVE_DEBOUNCE).await;
            let Some(state) = app.try_state::<crate::host::AppState>() else {
                return;
            };
            let store = &state.window_geometry;
            if store.debounce_seq.load(Ordering::Relaxed) != seq {
                return; // 期间又有新事件：新任务会落盘，本任务放弃
            }
            store.flush();
        });
    }

    /// setup 阶段恢复（窗口仍隐藏未亮）：读文件 → 越界校验 → 应用几何 →
    /// 按需最大化。校验不过（含显示器清单读不到的异常场景）整体丢弃，
    /// 保留 tauri.conf.json 默认 1280×800 居中。
    pub fn restore(&self, window: &WebviewWindow) {
        let Some(state) = load_state(&self.path) else {
            return;
        };
        // 显示器 bounds 统一转逻辑坐标（各自按自身缩放比换算，与窗口几何
        // 的逻辑坐标同域）；读不到清单属异常环境，保守丢弃恢复
        let monitors: Vec<Rect> = window
            .available_monitors()
            .unwrap_or_default()
            .iter()
            .map(|m| {
                let position = m.position().to_logical::<f64>(m.scale_factor());
                let size = m.size().to_logical::<f64>(m.scale_factor());
                Rect {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                }
            })
            .collect();
        if monitors.is_empty() || !is_recoverable(&state.rect(), &monitors) {
            eprintln!("[window-state] 记录几何越界或显示器清单不可用，回默认位置");
            return;
        }
        let _ = window.set_position(LogicalPosition::new(state.x, state.y));
        let _ = window.set_size(LogicalSize::new(state.width, state.height));
        if state.maximized {
            let _ = window.maximize();
        }
        // 恢复值即初始缓存：后续事件在它之上合并（防 destroy 时无从冲盘）
        *self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(state);
    }
}

#[cfg(test)]
#[path = "window_state_test.rs"]
mod window_state_test;
