//! host 模块：IPC 层——命令实现（校验/转调）、事件汇、通知、托盘、菜单、退出拦截。
//!
//! gpui-rewrite 阶段 1（引擎搬移）后本模块只剩壳职责：
//! - 命令壳（`#[tauri::command]`，校验/转调引擎 `*_core`，命令面清单在
//!   crate 根 lib.rs 的 `ipc_commands!` 宏，防漂移）；
//! - 生产事件汇 TauriEventSink（tauri emit + 系统通知门旁路路由）；
//! - 通知门（notify）、托盘与退出拦截（shell）、窗口几何（window_state）、
//!   壳侧文案（strings）。
//!
//! 应用级共享状态 AppState 已随引擎走（craftwave_engine::app_state）；
//! `IpcError` 本体在引擎 `util::errors`（providers 亦依赖，util 为最底层）。

pub mod commands;
pub mod notify;
// Rust 侧用户可见文案表（托盘菜单 + 系统通知正文）
pub mod strings;
// 宿主外壳：托盘、关闭到托盘、退出拦截、单实例唤起
pub mod shell;
// 窗口几何记忆
pub mod window_state;

pub use craftwave_engine::util::errors::IpcError;

use std::sync::Arc;

use craftwave_engine::util::events::{EventSink, keys};

/// 生产事件汇：tauri `Emitter::emit`（尽力而为，失败不影响 run 主链路）；
/// 对通知触发键（run 终态三键 + ask:opened）旁路系统通知门。
pub struct TauriEventSink {
    pub app: tauri::AppHandle,
    pub notify: Arc<notify::NotifyGate>,
}

impl EventSink for TauriEventSink {
    /// 双写出口：先照常 emit 给前端，再按事件键旁路系统通知门
    /// （通知触发判定收在通知门内部，这里只做路由分发）。
    fn emit(&self, key: &str, payload: serde_json::Value) {
        use tauri::Emitter;
        let _ = self.app.emit(key, payload.clone());
        // 通知旁路：只认四个触发键，payload 字段缺失时静默跳过
        let session_id = payload.get("sessionId").and_then(|v| v.as_str());
        let run_id = payload.get("runId").and_then(|v| v.as_str());
        match key {
            keys::ASK_OPENED => {
                if let (Some(session_id), Some(ask_id)) =
                    (session_id, payload.get("askId").and_then(|v| v.as_str()))
                {
                    self.notify.ask_opened(session_id, ask_id);
                }
            }
            keys::RUN_DONE | keys::RUN_ERROR | keys::RUN_CANCELLED => {
                if let (Some(session_id), Some(run_id)) = (session_id, run_id) {
                    let reason = match key {
                        keys::RUN_DONE => "done",
                        keys::RUN_ERROR => "error",
                        _ => "cancelled",
                    };
                    let duration_ms = payload
                        .get("durationMs")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    self.notify
                        .run_ended(session_id, run_id, reason, duration_ms);
                }
            }
            _ => {}
        }
    }
}
