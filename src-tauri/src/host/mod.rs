//! host 模块（§2.2）：IPC 层——命令实现（校验/转调）、事件汇、通知、托盘、菜单、退出拦截。
//!
//! 已落地：会话树两命令（10 号票，commands/session_tree.rs）、provider/config
//! 域 P0 命令（§3.3 #6–9）、start_chat/cancel_run（§3.3 #18/#19，13 号票，
//! commands/chat.rs）、resolve_ask/approve_plan（§3.3 #22/#26c，14 号票）、
//! AppState（数据目录 §7.1 + 协调器 + RunManager + 事件汇 + 会话用量累计）。
//! 2026-10-28：keyring 废除（§9.4 改写），API Key 明文存 config.json（0600）。
//! `IpcError` 本体在 `util::errors`（providers 亦依赖，util 为最底层）；
//! error.rs 仅存 `From<StorageError>` 映射。IPC manifest 单一事实源在
//! crate 根 lib.rs（`ipc_commands!` 宏，§12.3/NFR-12 防漂移）。

pub mod commands;
mod error;
pub mod followup;
pub mod notify;
pub mod run_persistence;
pub mod shell;

#[cfg(test)]
#[path = "run_persistence_test.rs"]
mod run_persistence_test;

pub use crate::util::errors::IpcError;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::agent::RunManager;
use crate::backend::frames::UsageTotal;
use crate::host::followup::FollowUpManager;
use crate::providers::types::AppConfig;
use crate::storage::SessionStore;
use crate::storage::config_store::ConfigStore;
use crate::storage::project_store::ProjectStore;
use crate::util::events::{EventSink, keys};

/// 应用级共享状态（tauri manage）。全部字段都是共享句柄（Arc 或内含 Arc），
/// 因此 [`AppState::share_handles`] 能以零拷贝代价产出一份轻量副本供异步
/// 任务持有（06 号票：run 收尾的 followUp 自动投递要在此后调用
/// start_chat_core——命令层之外的第二个入口，此前各任务只捕获细粒度克隆件）。
pub struct AppState {
    /// 配置协调器（§7.6）：config.json 唯一写入口（API Key 明文承载，0600）。
    /// Arc 共享：17 号票起通知门（notify.rs）读同一份缓存，避免双实例缓存漂移。
    pub config: Arc<ConfigStore<AppConfig>>,
    /// 活跃 run 注册表（§5.1；M1 单会话核心链路，13 号票）。内部 Arc<Mutex>，
    /// Clone 即同源句柄。
    pub runs: RunManager,
    /// 事件汇（§3.5）：生产 tauri emit；测试 RecordingEventSink（14 号票）。
    pub events: Arc<dyn EventSink>,
    /// 会话累计用量（tokens:update 的全量口径；run 帧增量折算 + suggest 后台小调用）。
    pub session_usage: Arc<Mutex<HashMap<String, UsageTotal>>>,
    /// 会话存储器（15 号票：run 落盘器与命令层共享同一锁表/索引）。
    pub store: Arc<SessionStore>,
    /// 项目注册表（FR-10-01，16 号票）。Arc 共享：run 任务内解析会话 roots 用。
    pub projects: Arc<ProjectStore>,
    /// UI 状态（§3.3 #30/31，16 号票：Tab 布局等前端自管 Json）。
    pub ui_state: Arc<ConfigStore<Value>>,
    /// 活跃会话标识（前端 set_ui_state 顺带写入；通知防打扰精判共享，
    /// 17 号票）。headless 测试下无写入方，保持 None。
    pub active_session: Arc<Mutex<Option<String>>>,
    /// 退出确认状态机（§9.6 退出拦截 + 19 号票超时兜底：request 置位并
    /// 起算，resolve 回执解除；超时未回执由 request_exit 排定的定时任务
    /// 判定后放行）。
    pub exit_confirm: Arc<Mutex<shell::ExitConfirm>>,
    /// 会话级 followUp 队列（§5.5，06 号票：运行中排队续发 + 失败暂停 +
    /// 取消返还，`sessions/<sid>.queue.json` 持久化）。
    pub followups: FollowUpManager,
}

impl AppState {
    /// `~/.craftwave` 数据目录（§7.1）下的 config.json。
    pub fn new_in(data_dir: PathBuf, events: Arc<dyn EventSink>) -> Self {
        let store = Arc::new(SessionStore::with_root(data_dir.clone()));
        Self {
            config: Arc::new(ConfigStore::new(data_dir.join("config.json"))),
            runs: RunManager::new(),
            events,
            session_usage: Arc::new(Mutex::new(HashMap::new())),
            store,
            projects: Arc::new(ProjectStore::new(&data_dir)),
            ui_state: Arc::new(ConfigStore::new(data_dir.join("ui-state.json"))),
            active_session: Arc::new(Mutex::new(None)),
            exit_confirm: Arc::new(Mutex::new(shell::ExitConfirm::new(
                shell::EXIT_CONFIRM_TIMEOUT,
            ))),
            followups: FollowUpManager::new(data_dir.clone()),
        }
    }

    /// 产出一份全部字段同源的轻量副本（Arc/内部 Arc 克隆，零深拷贝）——
    /// 异步任务（run 收尾的 followUp 自动投递，06 号票）经它持有完整状态
    /// 面，可直接调用 start_chat_core 开新 run。
    pub fn share_handles(&self) -> AppState {
        Self {
            config: self.config.clone(),
            runs: self.runs.clone(),
            events: self.events.clone(),
            session_usage: self.session_usage.clone(),
            store: self.store.clone(),
            projects: self.projects.clone(),
            ui_state: self.ui_state.clone(),
            active_session: self.active_session.clone(),
            exit_confirm: self.exit_confirm.clone(),
            followups: self.followups.clone(),
        }
    }

    /// 累计会话用量并返回累计后全量（tokens:update 载荷，FR-01-13 会话口径）。
    pub fn accumulate_usage(&self, session_id: &str, delta: &UsageTotal) -> UsageTotal {
        let mut map = self
            .session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let total = map.entry(session_id.to_owned()).or_default();
        total.accumulate(delta);
        *total
    }
}

/// 生产事件汇：tauri `Emitter::emit`（尽力而为，失败不影响 run 主链路）；
/// 对通知触发键（run 终态三键 + ask:opened）旁路系统通知门（§9.6，17 号票）。
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
