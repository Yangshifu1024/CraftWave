//! Provider/settings 领域服务编排：协调器（§7.6）+ keyring 协议（§9.4）的组装层。
//!
//! host 命令层是本层的薄壳；单测直接打本层（不依赖 Tauri 运行时）。

use crate::providers::keyring::{self, KeyringStore};
use crate::providers::types::{AppConfig, ProviderConfig};
use crate::storage::config_store::ConfigStore;
use crate::util::errors::IpcError;

/// 读取当前配置（缓存优先）。附带意向残留内容于日志层面提示——
/// UI 提示接线属后续票（`ConfigStore::pending_intent` 可查，不自动重放）。
pub fn get_config(store: &ConfigStore<AppConfig>) -> Result<AppConfig, IpcError> {
    if store.pending_intent()?.is_some() {
        // §7.6：崩溃后重启发现意向日志则提示用户重试（不自动重放写操作）。
        // M1：残留不影响读取；重试保存（相同 provider 再存一次 Key）即清。
        eprintln!("[craftwave] 检测到未完成的凭据提交意向（intent.json），请重新保存该供应商 Key");
    }
    Ok(store.load()?.as_ref().clone())
}

/// 保存整份配置（§3.3 #7）：经协调器归一化原子落盘，返回归一化结果。
/// 不涉 keyring（newKey 只经 save_provider 提交）。
pub fn save_config(
    store: &ConfigStore<AppConfig>,
    config: AppConfig,
) -> Result<AppConfig, IpcError> {
    Ok(store.save(config)?.as_ref().clone())
}

/// 保存供应商（§3.3 #8）：`{ provider, newKey? } → ProviderConfig`。
///
/// - newKey 有值：走 §9.4 协议——intent 落盘 → keyring 写（generation 递增，
///   数组整体覆写，M1 单 Key）→ 读回校验 → 仅落 credentialRef/credentialCount
///   → config 原子替换 → 删意向；
/// - newKey 无值：仅更新元数据（保留既有凭据引用）。
///
/// 返回归一化后的该供应商（含凭据元数据）。
pub fn save_provider(
    store: &ConfigStore<AppConfig>,
    keys: &dyn KeyringStore,
    provider: ProviderConfig,
    new_key: Option<String>,
    op_id: &str,
) -> Result<ProviderConfig, IpcError> {
    if provider.id.trim().is_empty() {
        return Err(IpcError::invalid_arg("provider.id 不能为空"));
    }
    if let Some(key) = &new_key
        && key.trim().is_empty()
    {
        return Err(IpcError::invalid_arg("newKey 不能为空"));
    }
    // §9.4：意向在外部落子前落盘；指纹对"本次将写入的明文"计算。
    let intent = new_key
        .as_deref()
        .map(|key| keyring::intent_record(op_id, &provider.id, &keyring::fingerprint(key)));

    let provider_id = provider.id.clone();
    let mut prepared = provider;
    let keys_for_commit = new_key;
    let arc = store.commit_from_current(intent, |cfg| {
        if let Some(key) = &keys_for_commit {
            let meta = keyring::commit_keys(keys, &provider_id, std::slice::from_ref(key))?;
            prepared.credential_ref = Some(meta.credential_ref);
            prepared.credential_count = meta.credential_count;
        }
        match cfg.providers.iter().position(|p| p.id == provider_id) {
            Some(i) => cfg.providers[i] = prepared.clone(),
            None => cfg.providers.push(prepared.clone()),
        }
        Ok(())
    })?;

    arc.providers
        .iter()
        .find(|p| p.id == provider_id)
        .cloned()
        .ok_or_else(|| IpcError::internal("保存成功但回读不到该供应商"))
}

/// 删除供应商（§3.3 #9）：config 原子移除后同步删除 keyring 账户。
///
/// 顺序取舍：先 config 后 keyring——若反过来（先删 keyring）config 写失败
/// 会留下悬空凭据引用（读 Key 必失败）；先 config 失败则整体失败无副作用；
/// config 成功后 keyring 删除失败只产生无引用的孤儿账户（无害，M1 不回滚）。
pub fn delete_provider(
    store: &ConfigStore<AppConfig>,
    keys: &dyn KeyringStore,
    provider_id: &str,
) -> Result<(), IpcError> {
    // 锁内读-改-写：并发 save/delete 不丢更新
    store.update(|next| {
        let before = next.providers.len();
        next.providers.retain(|p| p.id != provider_id);
        if next.providers.len() == before {
            return Err(IpcError::not_found(format!("供应商不存在：{provider_id}")));
        }
        // activeModelId 悬空由归一化自愈
        Ok(())
    })?;
    // 孤儿账户无害：失败不回滚、不阻塞（M1 简化，见函数头注释）
    let _ = keyring::delete_credentials(keys, provider_id);
    Ok(())
}
