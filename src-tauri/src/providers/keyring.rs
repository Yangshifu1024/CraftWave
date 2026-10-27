//! keyring 提交协议（§9.4，FR-05-09）与钥匙串抽象。
//!
//! - 账户命名：`craftwave.provider.<providerId>`；config 只存账户引用，永不存明文。
//! - 载荷 envelope：`{"generation": n, "keys": [...]}`（§11.2 M1 单 Key 直用，数组就位）。
//! - 提交协议：写账户（generation 递增）→ 读回校验（fingerprint 比对）；
//!   intent.json 的生命周期由配置协调器（§7.6）编排。
//! - 日志永不打印 Key 明文与指纹全文（指纹本身即 sha256 前 16 位截断）。

use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::util::errors::IpcError;

/// 钥匙串 service 名（三平台共用）。
pub const KEYRING_SERVICE: &str = "craftwave";

/// 某供应商的 keyring 账户名（§9.4：craftwave.provider.<id>）。
pub fn credential_account(provider_id: &str) -> String {
    format!("craftwave.provider.{provider_id}")
}

/// 存入钥匙串的载荷：generation 随每次提交递增；keys 为多 Key 数组
/// （§11.2：M1 单 Key 直用，数组结构就位）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialEnvelope {
    pub generation: u32,
    pub keys: Vec<String>,
}

/// 提交成功后回填进 ProviderConfig 的凭据元数据。
#[derive(Debug, Clone, PartialEq)]
pub struct CredentialMeta {
    /// keyring 账户引用（ProviderConfig.credentialRef）。
    pub credential_ref: String,
    /// 展示用 Key 数量（ProviderConfig.credentialCount）。
    pub credential_count: u32,
}

/// 钥匙串交互抽象：测试用内存 mock（`MemoryKeyring`），生产用系统钥匙串（`OsKeyring`）。
pub trait KeyringStore: Send + Sync {
    /// 写入（覆盖语义）。
    fn set_secret(&self, account: &str, secret: &str) -> Result<(), IpcError>;
    /// 读取；不存在返回 None。
    fn get_secret(&self, account: &str) -> Result<Option<String>, IpcError>;
    /// 删除；账户不存在视为成功（幂等，屏蔽各平台 NoEntry 差异）。
    fn delete_secret(&self, account: &str) -> Result<(), IpcError>;
}

/// 测试用内存实现（亦作 trait 语义的参考实现）。
#[derive(Default)]
pub struct MemoryKeyring {
    entries: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl MemoryKeyring {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeyringStore for MemoryKeyring {
    fn set_secret(&self, account: &str, secret: &str) -> Result<(), IpcError> {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(account.to_string(), secret.to_string());
        Ok(())
    }

    fn get_secret(&self, account: &str) -> Result<Option<String>, IpcError> {
        Ok(self
            .entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(account)
            .cloned())
    }

    fn delete_secret(&self, account: &str) -> Result<(), IpcError> {
        self.entries
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(account);
        Ok(())
    }
}

/// 生产实现：系统钥匙串（keyring 4.2，默认 v1 feature =
/// apple keychain / windows native / zbus-secret-service）。
pub struct OsKeyring;

impl KeyringStore for OsKeyring {
    fn set_secret(&self, account: &str, secret: &str) -> Result<(), IpcError> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, account)
            .map_err(|e| IpcError::keyring(format!("钥匙串条目创建失败：{e}")))?;
        entry
            .set_password(secret)
            .map_err(|e| IpcError::keyring(format!("钥匙串写入失败：{e}")))
    }

    fn get_secret(&self, account: &str) -> Result<Option<String>, IpcError> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, account)
            .map_err(|e| IpcError::keyring(format!("钥匙串条目创建失败：{e}")))?;
        match entry.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(IpcError::keyring(format!("钥匙串读取失败：{e}"))),
        }
    }

    fn delete_secret(&self, account: &str) -> Result<(), IpcError> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, account)
            .map_err(|e| IpcError::keyring(format!("钥匙串条目创建失败：{e}")))?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(IpcError::keyring(format!("钥匙串删除失败：{e}"))),
        }
    }
}

/// 指纹：sha256 前 16 位 hex（截断即"非全文"，§9.4 日志纪律）。
pub fn fingerprint(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..16].to_string()
}

/// §9.4 意向记录：`{opId, providerId, keyFingerprint}`（intent.json 内容）。
pub fn intent_record(op_id: &str, provider_id: &str, key_fingerprint: &str) -> serde_json::Value {
    json!({
        "opId": op_id,
        "providerId": provider_id,
        "keyFingerprint": key_fingerprint,
    })
}

/// 读取账户里的 envelope（不存在或非 envelope JSON 返回 None——旧版裸文本 Key 不被继承）。
pub fn read_envelope(
    store: &dyn KeyringStore,
    provider_id: &str,
) -> Result<Option<CredentialEnvelope>, IpcError> {
    let account = credential_account(provider_id);
    let Some(raw) = store.get_secret(&account)? else {
        return Ok(None);
    };
    Ok(serde_json::from_str(&raw).ok())
}

/// §9.4 提交协议核心：写账户（generation 递增，数组整体覆写）→ 读回校验指纹。
/// 成功返回凭据元数据；任一步失败 config 不变更（由协调器保证）。
pub fn commit_keys(
    store: &dyn KeyringStore,
    provider_id: &str,
    keys: &[String],
) -> Result<CredentialMeta, IpcError> {
    if keys.is_empty() {
        return Err(IpcError::invalid_arg("Key 列表不能为空"));
    }
    if keys.iter().any(|k| k.trim().is_empty()) {
        return Err(IpcError::invalid_arg("Key 不能为空"));
    }
    let account = credential_account(provider_id);
    let generation = read_envelope(store, provider_id)?
        .map(|e| e.generation.wrapping_add(1))
        .unwrap_or(1);
    let envelope = CredentialEnvelope {
        generation,
        keys: keys.to_vec(),
    };
    let secret = serde_json::to_string(&envelope)
        .map_err(|e| IpcError::internal(format!("envelope 序列化失败：{e}")))?;
    let expected = fingerprint(&secret);

    // 1) 写账户
    store.set_secret(&account, &secret)?;
    // 2) 读回校验（fingerprint 比对）
    let readback = store
        .get_secret(&account)?
        .ok_or_else(|| IpcError::keyring("钥匙串读回为空（写入疑似未生效）"))?;
    if fingerprint(&readback) != expected {
        return Err(IpcError::keyring("钥匙串读回校验失败（指纹不一致）"));
    }
    Ok(CredentialMeta {
        credential_ref: account,
        credential_count: keys.len() as u32,
    })
}

/// 删除某供应商的 keyring 账户（幂等）。
pub fn delete_credentials(store: &dyn KeyringStore, provider_id: &str) -> Result<(), IpcError> {
    store.delete_secret(&credential_account(provider_id))
}
