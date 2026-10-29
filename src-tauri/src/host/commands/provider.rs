//! provider 域命令（§3.3 #8/#9）：save_provider / delete_provider。
//!
//! 薄壳：转调 providers::service（协调器在服务层，可单测）。

use serde::Deserialize;
use tauri::State;

use crate::host::AppState;
use crate::providers::service;
use crate::providers::types::ProviderConfig;
use crate::util::errors::IpcError;

/// `save_provider` 参数契约（§3.3 #8；文档命名锚点，命令形参按字段平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SaveProviderArgs {
    pub provider: ProviderConfig,
    #[cfg_attr(test, ts(optional))]
    pub new_key: Option<String>,
}

/// `delete_provider` 参数契约（§3.3 #9；文档命名锚点，命令形参按字段平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DeleteProviderArgs {
    pub provider_id: String,
}

/// §3.3 #8：保存供应商；newKey 有值时写入 apiKey（明文随 config.json 0600
/// 落盘，§9.4 改写）。返回归一化后的供应商。
#[tauri::command]
pub async fn save_provider(
    state: State<'_, AppState>,
    provider: ProviderConfig,
    new_key: Option<String>,
) -> Result<ProviderConfig, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = SaveProviderArgs { provider, new_key };
    service::save_provider(&state.config, args.provider, args.new_key)
}

/// §3.3 #9：删除供应商（Key 随配置一并消失）。
#[tauri::command]
pub async fn delete_provider(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<(), IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = DeleteProviderArgs { provider_id };
    service::delete_provider(&state.config, &args.provider_id)
}
