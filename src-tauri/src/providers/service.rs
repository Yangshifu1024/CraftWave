//! Provider/settings 领域服务编排：协调器（§7.6）的组装层。
//!
//! host 命令层是本层的薄壳；单测直接打本层（不依赖 Tauri 运行时）。
//!
//! 2026-10-28 用户决策废除 keyring（§9.4 改写）：API Key 明文存
//! config.json（文件权限 0600，由 ConfigStore 落盘时收紧），
//! 不再有 intent 意向日志与两阶段提交。

use crate::providers::types::{AppConfig, ProviderConfig};
use crate::storage::config_store::ConfigStore;
use crate::util::errors::IpcError;

/// 读取当前配置（缓存优先）。
pub fn get_config(store: &ConfigStore<AppConfig>) -> Result<AppConfig, IpcError> {
    Ok(store.load()?.as_ref().clone())
}

/// 保存整份配置（§3.3 #7）：经协调器归一化原子落盘，返回归一化结果。
pub fn save_config(
    store: &ConfigStore<AppConfig>,
    config: AppConfig,
) -> Result<AppConfig, IpcError> {
    Ok(store.save(config)?.as_ref().clone())
}

/// 保存供应商（§3.3 #8）：`{ provider, newKey? } → ProviderConfig`。
///
/// - newKey 有值：写入 provider.apiKey（明文，随 config.json 0600 落盘）；
/// - newKey 无值：按入参原样保存（前端整份回传，apiKey 字段随回传值）。
///
/// 返回归一化后的该供应商。
pub fn save_provider(
    store: &ConfigStore<AppConfig>,
    mut provider: ProviderConfig,
    new_key: Option<String>,
) -> Result<ProviderConfig, IpcError> {
    if provider.id.trim().is_empty() {
        return Err(IpcError::invalid_arg("provider.id 不能为空"));
    }
    if let Some(key) = &new_key
        && key.trim().is_empty()
    {
        return Err(IpcError::invalid_arg("newKey 不能为空"));
    }
    if let Some(key) = new_key {
        provider.api_key = Some(key);
    }
    let provider_id = provider.id.clone();
    let arc = store.update(|cfg| {
        match cfg.providers.iter().position(|p| p.id == provider_id) {
            Some(i) => cfg.providers[i] = provider.clone(),
            None => cfg.providers.push(provider.clone()),
        }
        Ok(())
    })?;

    arc.providers
        .iter()
        .find(|p| p.id == provider_id)
        .cloned()
        .ok_or_else(|| IpcError::internal("保存成功但回读不到该供应商"))
}

/// 删除供应商（§3.3 #9）：config 原子移除（Key 随配置一并消失）。
pub fn delete_provider(store: &ConfigStore<AppConfig>, provider_id: &str) -> Result<(), IpcError> {
    store.update(|next| {
        let before = next.providers.len();
        next.providers.retain(|p| p.id != provider_id);
        if next.providers.len() == before {
            return Err(IpcError::not_found(format!("供应商不存在：{provider_id}")));
        }
        // activeModelId 悬空由归一化自愈
        Ok(())
    })?;
    Ok(())
}
