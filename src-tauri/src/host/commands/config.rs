//! config 域命令（§3.3 #6/#7）：get_config / save_config。
//!
//! 薄壳：参数即类型校验（serde 反序列化）+ 转调 providers::service。

use tauri::State;

use crate::host::AppState;
use crate::providers::service;
use crate::providers::types::AppConfig;
use crate::util::error::IpcError;

/// §3.3 #6：读取当前配置（协调器缓存优先）。
#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<AppConfig, IpcError> {
    service::get_config(&state.config)
}

/// §3.3 #7：整份配置经协调器归一化原子落盘，返回归一化结果。
#[tauri::command]
pub async fn save_config(
    state: State<'_, AppState>,
    config: AppConfig,
) -> Result<AppConfig, IpcError> {
    service::save_config(&state.config, config)
}
