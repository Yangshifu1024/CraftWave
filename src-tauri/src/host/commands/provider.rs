//! provider 域命令（§3.3 #8/#9）：save_provider / delete_provider。
//!
//! 薄壳：转调 providers::service（协调器 + §9.4 keyring 协议在服务层，可单测）。

use serde::Deserialize;
use tauri::State;

use crate::host::AppState;
use crate::providers::service;
use crate::providers::types::ProviderConfig;
use crate::util::errors::IpcError;

/// `save_provider` 参数契约（§3.3 #8；文档命名锚点，命令形参按字段平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProviderArgs {
    pub provider: ProviderConfig,
    pub new_key: Option<String>,
}

/// `delete_provider` 参数契约（§3.3 #9；文档命名锚点，命令形参按字段平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteProviderArgs {
    pub provider_id: String,
}

/// §3.3 #8：保存供应商；newKey 有值时走 keyring 提交协议（§9.4），
/// 成功后仅落 credentialRef/credentialCount。返回归一化后的供应商。
#[tauri::command]
pub async fn save_provider(
    state: State<'_, AppState>,
    provider: ProviderConfig,
    new_key: Option<String>,
) -> Result<ProviderConfig, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = SaveProviderArgs { provider, new_key };
    // opId：每命令一个新 uuid（§9.4 意向幂等去重键）
    let op_id = crate::util::ids::new_uuid();
    service::save_provider(
        &state.config,
        state.keys.as_ref(),
        args.provider,
        args.new_key,
        &op_id,
    )
}

/// §3.3 #9：删除供应商并同步删除 keyring 账户。
#[tauri::command]
pub async fn delete_provider(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<(), IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = DeleteProviderArgs { provider_id };
    service::delete_provider(&state.config, state.keys.as_ref(), &args.provider_id)
}
