//! 系统通知门（§9.6，FR-13-06/07，17 号票）。
//!
//! 职责：把「值得用户回来看一眼」的事件转成系统通知——run 终态（窗口
//! 失焦/隐藏时）与人工介入等待（ask 询问/审批）。触发点不在本模块：
//! [`crate::host::TauriEventSink::emit`] 对 `run:done` / `run:error` /
//! `run:cancelled` / `ask:opened` 四个事件键旁路调用本门（单一旁路点，
//! 测试用的 [`crate::util::events::RecordingEventSink`] 天然无通知副作用）。
//!
//! 防打扰（FR-13-06）：通知总开关关闭时不发；「仅人工介入」开启时 run
//! 终态不发；窗口聚焦且正停留在触发会话时不发（隐藏窗口视为失焦）。
//!
//! 点击回跳：通知携带 `anchor`（sessionId/runId/anchor）载荷，点击后由
//! [`install_click_routing`] 注册的回调唤起主窗并转发 `notify:activate`
//! 事件，前端据此路由到对应会话。

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tauri::Manager;
use tauri_plugin_notification::{ActionPerformed, NotificationExt};

use crate::host::shell::MAIN_WINDOW;
use crate::providers::types::AppConfig;
use crate::storage::config_store::ConfigStore;
use crate::util::events::{EventSink, keys};

/// 通知门：窗口焦点判定 + 配置开关判定 + 发送，均为尽力而为（失败只
/// 影响通知本身，不影响 run 主链路）。
pub struct NotifyGate {
    app: tauri::AppHandle,
    /// 活跃会话标识（前端 `set_ui_state` 顺带写入；与本门共享同一把锁）。
    active_session: Arc<Mutex<Option<String>>>,
    /// 应用配置（读通知偏好 `notifications`，FR-13-06 设置项）。
    config: Arc<ConfigStore<AppConfig>>,
}

impl NotifyGate {
    /// 构造通知门：三个入参与 AppState 共享同一实例（Arc 克隆），
    /// 保证焦点判定与配置开关读到的都是最新状态。
    pub fn new(
        app: tauri::AppHandle,
        active_session: Arc<Mutex<Option<String>>>,
        config: Arc<ConfigStore<AppConfig>>,
    ) -> Self {
        Self {
            app,
            active_session,
            config,
        }
    }

    /// run 终态通知（§9.6 触发点①）。`reason` 为 done / error / cancelled。
    pub fn run_ended(&self, session_id: &str, run_id: &str, reason: &str, duration_ms: u64) {
        let verb = match reason {
            "done" => "运行完成",
            "error" => "运行出错",
            _ => "运行已取消",
        };
        let body = format!("{verb}（耗时 {:.1} 秒）", duration_ms as f64 / 1000.0);
        self.send(
            session_id,
            &body,
            json!({
                "sessionId": session_id,
                "runId": run_id,
                "anchor": { "kind": "runEnd", "ref": run_id },
            }),
        );
    }

    /// 人工介入通知（§9.6 触发点②：ask 询问/审批等待用户回应）。
    pub fn ask_opened(&self, session_id: &str, ask_id: &str) {
        self.send(
            session_id,
            "等待你的回应（询问/审批）",
            json!({
                "sessionId": session_id,
                "anchor": { "kind": "ask", "ref": ask_id },
            }),
        );
    }

    /// 统一出口：防打扰判定通过才发送。
    fn send(&self, session_id: &str, body: &str, anchor: Value) {
        if self.suppressed(session_id) {
            return;
        }
        let _ = self
            .app
            .notification()
            .builder()
            .title("CraftWave")
            .body(body)
            // anchor 载荷随通知存储；点击时经 on_action 回读路由
            .extra("anchor", anchor)
            .show();
    }

    /// 防打扰判定（FR-13-06）：总开关关 / 前台正看该会话 → 抑制。
    fn suppressed(&self, session_id: &str) -> bool {
        let prefs = self.config.load().map(|c| c.notifications.clone()).ok();
        let Some(prefs) = prefs else {
            return true; // 配置读不出来时不打扰用户
        };
        if !prefs.enabled {
            return true;
        }
        // 窗口聚焦且正停留在触发会话：用户已经看着它，无需通知
        let focused = self
            .app
            .get_webview_window(MAIN_WINDOW)
            .is_some_and(|w| w.is_focused().unwrap_or(false));
        if focused {
            let active = self
                .active_session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if active.as_deref() == Some(session_id) {
                return true;
            }
        }
        false
    }
}

/// 注册通知点击回调（装配期调用一次）：唤起主窗 + 转发 `notify:activate`。
/// 注意插件语义：第一个回调注册之前发出的通知点击不上报。
pub fn install_click_routing(gate: &NotifyGate) {
    let app = gate.app.clone();
    let sink_app = app.clone();
    let _ = app
        .notification()
        .on_action(move |action: &ActionPerformed| {
            let Some(notification) = action.notification() else {
                return;
            };
            let Some(anchor) = notification.extra().get("anchor") else {
                return;
            };
            crate::host::shell::activate_main_window(&sink_app);
            // 与事件汇同语义：尽力而为，失败不影响唤起
            TauriEventEmit(&sink_app).emit(keys::NOTIFY_ACTIVATE, anchor.clone());
        });
}

/// emit 的最小适配（避免在回调里直接依赖 TauriEventSink 完整结构）。
struct TauriEventEmit<'a>(&'a tauri::AppHandle);

impl EventSink for TauriEventEmit<'_> {
    fn emit(&self, key: &str, payload: Value) {
        use tauri::Emitter;
        let _ = self.0.emit(key, payload);
    }
}
