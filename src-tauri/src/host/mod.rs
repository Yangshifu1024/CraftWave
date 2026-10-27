//! host 模块（§2.2）：IPC 层——命令实现（校验/转调）、事件汇、通知、托盘、菜单、退出拦截。
//!
//! 已落地：会话树两命令（10 号票，commands/session_tree.rs）、provider/config
//! 域 P0 命令（§3.3 #6–9）、start_chat/cancel_run（§3.3 #18/#19，13 号票，
//! commands/chat.rs）、resolve_ask/approve_plan（§3.3 #22/#26c，14 号票）、
//! AppState（数据目录 §7.1 + 协调器 + keyring + RunManager + 事件汇 +
//! 会话用量累计）。`IpcError` 本体在 `util::errors`（providers 亦依赖，
//! util 为最底层）；error.rs 仅存 `From<StorageError>` 映射。IPC manifest
//! 单一事实源在 crate 根 lib.rs（`ipc_commands!` 宏，§12.3/NFR-12 防漂移）。

pub mod commands;
mod error;

pub use crate::util::errors::IpcError;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::agent::RunManager;
use crate::backend::frames::UsageTotal;
use crate::providers::keyring::KeyringStore;
use crate::providers::types::AppConfig;
use crate::storage::config_store::ConfigStore;
use crate::util::events::EventSink;

/// 应用级共享状态（tauri manage）。
pub struct AppState {
    /// 配置协调器（§7.6）：config.json 唯一写入口。
    pub config: ConfigStore<AppConfig>,
    /// 钥匙串抽象：生产为系统钥匙串；测试可注入内存 mock。
    pub keys: Arc<dyn KeyringStore>,
    /// 活跃 run 注册表（§5.1；M1 单会话核心链路，13 号票）。
    pub runs: RunManager,
    /// 事件汇（§3.5）：生产 tauri emit；测试 RecordingEventSink（14 号票）。
    pub events: Arc<dyn EventSink>,
    /// 会话累计用量（tokens:update 的全量口径；run 帧增量折算 + suggest 后台小调用）。
    pub session_usage: Mutex<HashMap<String, UsageTotal>>,
}

impl AppState {
    /// `~/.craftwave` 数据目录（§7.1）下的 config.json + intent.json。
    pub fn new_in(
        data_dir: PathBuf,
        keys: Arc<dyn KeyringStore>,
        events: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            config: ConfigStore::new(data_dir.join("config.json"), data_dir.join("intent.json")),
            keys,
            runs: RunManager::new(),
            events,
            session_usage: Mutex::new(HashMap::new()),
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

/// 生产事件汇：tauri `Emitter::emit`（尽力而为，失败不影响 run 主链路）。
pub struct TauriEventSink(pub tauri::AppHandle);

impl EventSink for TauriEventSink {
    fn emit(&self, key: &str, payload: serde_json::Value) {
        use tauri::Emitter;
        let _ = self.0.emit(key, payload);
    }
}
