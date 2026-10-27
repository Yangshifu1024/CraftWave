//! host 模块（§2.2）：IPC 层——命令实现（校验/转调）、事件汇、通知、托盘、菜单、退出拦截。
//!
//! 已落地：会话树两命令（10 号票，commands/session_tree.rs）、provider/config
//! 域 P0 命令（§3.3 #6–9）、AppState（数据目录 §7.1 + 协调器 + keyring）。
//! `IpcError` 本体在 `util::errors`（providers 亦依赖，util 为最底层）；
//! error.rs 仅存 `From<StorageError>` 映射。IPC manifest 单一事实源在
//! crate 根 lib.rs（`ipc_commands!` 宏，§12.3/NFR-12 防漂移）。

pub mod commands;
mod error;

pub use crate::util::errors::IpcError;

use std::path::PathBuf;
use std::sync::Arc;

use crate::providers::keyring::KeyringStore;
use crate::providers::types::AppConfig;
use crate::storage::config_store::ConfigStore;

/// 应用级共享状态（tauri manage）。
pub struct AppState {
    /// 配置协调器（§7.6）：config.json 唯一写入口。
    pub config: ConfigStore<AppConfig>,
    /// 钥匙串抽象：生产为系统钥匙串；测试可注入内存 mock。
    pub keys: Arc<dyn KeyringStore>,
}

impl AppState {
    /// `~/.craftwave` 数据目录（§7.1）下的 config.json + intent.json。
    pub fn new_in(data_dir: PathBuf, keys: Arc<dyn KeyringStore>) -> Self {
        Self {
            config: ConfigStore::new(data_dir.join("config.json"), data_dir.join("intent.json")),
            keys,
        }
    }
}
