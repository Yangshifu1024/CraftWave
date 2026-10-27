//! provider 域命令（§3.3 #8/#9）：save_provider / delete_provider。
//!
//! 薄壳：转调 providers::service（协调器 + §9.4 keyring 协议在服务层，可单测）。

use tauri::State;

use crate::host::AppState;
use crate::providers::service;
use crate::providers::types::ProviderConfig;
use crate::util::error::IpcError;

/// §3.3 #8：保存供应商；newKey 有值时走 keyring 提交协议（§9.4），
/// 成功后仅落 credentialRef/credentialCount。返回归一化后的供应商。
#[tauri::command]
pub async fn save_provider(
    state: State<'_, AppState>,
    provider: ProviderConfig,
    new_key: Option<String>,
) -> Result<ProviderConfig, IpcError> {
    // opId：每命令一个新 uuid（§9.4 意向幂等去重键）
    let op_id = crate::util::ids::new_uuid();
    service::save_provider(
        &state.config,
        state.keys.as_ref(),
        provider,
        new_key,
        &op_id,
    )
}

/// §3.3 #9：删除供应商并同步删除 keyring 账户。
#[tauri::command]
pub async fn delete_provider(
    state: State<'_, AppState>,
    provider_id: String,
) -> Result<(), IpcError> {
    service::delete_provider(&state.config, state.keys.as_ref(), &provider_id)
}
