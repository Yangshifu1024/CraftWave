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

#[cfg(test)]
#[path = "notify_test.rs"]
mod notify_test;

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
    /// 正文语言随 `config.ui.language`（31 号票——每次触发读最新配置缓存，
    /// 语言切换后下一条通知即新语言）。
    pub fn run_ended(&self, session_id: &str, run_id: &str, reason: &str, duration_ms: u64) {
        let lang = self.ui_language();
        let body = crate::host::strings::run_ended_body(reason, duration_ms as f64 / 1000.0, lang);
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
            crate::host::strings::t(
                crate::host::strings::Str::NotifyAskOpened,
                self.ui_language(),
            ),
            json!({
                "sessionId": session_id,
                "anchor": { "kind": "ask", "ref": ask_id },
            }),
        );
    }

    /// 统一出口：防打扰判定通过才发送。
    ///
    /// 授权未授予时的口径（19 号票）：`show()` 失败（macOS 拒发）一律吞掉——
    /// 通知是尽力而为的旁路，触发侧静默降级不报错、不影响 run 主链路；
    /// 授权引导在设置页（`notification_permission_status` 命令）承担。
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

    /// 界面语言（31 号票）：读最新配置缓存；读不出（配置损坏等）按缺省
    /// 中文——宁可退回基准语言也不阻塞通知。
    fn ui_language(&self) -> crate::providers::types::Language {
        self.config
            .load()
            .map(|c| c.ui.language)
            .unwrap_or_default()
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

/// 授权态前端口径（字符串化，跨 IPC 返回）：
/// `granted` 可送达 / `denied` 被拒 / `prompt` 未决定 / `unknown` 查不出。
#[cfg(target_os = "macos")]
pub const AUTH_GRANTED: &str = "granted";
#[cfg(target_os = "macos")]
pub const AUTH_DENIED: &str = "denied";
#[cfg(target_os = "macos")]
pub const AUTH_PROMPT: &str = "prompt";
#[cfg(target_os = "macos")]
pub const AUTH_UNKNOWN: &str = "unknown";

/// UNAuthorizationStatus 原始值 → 前端口径。Provisional（静音送达）与
/// Ephemeral 都能送达，归 granted；纯映射无副作用，单测见 notify_test.rs。
#[cfg(target_os = "macos")]
fn authorization_word(status: objc2_user_notifications::UNAuthorizationStatus) -> &'static str {
    use objc2_user_notifications::UNAuthorizationStatus;
    if status == UNAuthorizationStatus::NotDetermined {
        AUTH_PROMPT
    } else if status == UNAuthorizationStatus::Denied {
        AUTH_DENIED
    } else {
        AUTH_GRANTED
    }
}

/// macOS 通知授权态查询（19 号票，设置页引导的数据面）。
///
/// 必须在主线程调用（UNUserNotificationCenter 的线程约束）：发起异步设置
/// 查询，结果经 `tx` 回传——completion 由系统队列回调，调用方在别的线程
/// `recv_timeout` 等待（毫秒级；等不到按 `unknown` 口径，宁可少打扰）。
///
/// 为什么不走插件的 `permission_state`：桌面端实现恒返回 Granted
/// （tauri-plugin-notification 2.7/2.8 源码），拿不到 macOS 真实授权态。
/// dev 裸进程（`cargo tauri dev`，无 Info.plist）没有 bundle id，直接查会
/// 抛 ObjC 异常——按 `unknown` 早退，生产（bundle .app）才有真实口径。
#[cfg(target_os = "macos")]
pub fn query_macos_authorization(tx: std::sync::mpsc::Sender<&'static str>) {
    use block2::RcBlock;
    use objc2_foundation::NSBundle;
    use objc2_user_notifications::{UNNotificationSettings, UNUserNotificationCenter};
    use std::ptr::NonNull;

    // dev 裸进程没有 bundle id：UNUserNotificationCenter 会抛 ObjC 异常，早退
    // （objc2 把这两个方法标为安全绑定——异常是 ObjC 层的，Rust 面拦不住）
    let bundled = NSBundle::mainBundle().bundleIdentifier().is_some();
    if !bundled {
        let _ = tx.send(AUTH_UNKNOWN);
        return;
    }
    // 线程约束：本函数已由调用方调度在主线程；completion 块由系统队列回调，
    // 块内只做 channel send（不触碰 UI、不 panic——send 断连也只返回 Err）。
    let center = UNUserNotificationCenter::currentNotificationCenter();
    let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
        // NonNull::as_ref 是 unsafe（对象生命周期由系统保证存活到回调结束）
        let status = unsafe { settings.as_ref().authorizationStatus() };
        let _ = tx.send(authorization_word(status));
    });
    center.getNotificationSettingsWithCompletionHandler(&block);
}
