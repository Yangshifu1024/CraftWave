//! IPC 层（§2.2 host）：命令实现（校验/转调）、事件汇、通知、托盘、菜单、退出拦截。
//!
//! 本切片交付：provider/config 域 P0 命令（§3.3 #6–9）与 IPC manifest
//! 契约测试基座（§12.3）；事件汇/通知/托盘属后续票。

pub mod commands;

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

#[cfg(test)]
#[path = "manifest_test.rs"]
mod manifest_test;
