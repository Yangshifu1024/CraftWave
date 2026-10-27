//! keyring 提交协议（§9.4）测试：内存 mock 驱动；真实钥匙串冒烟标 `#[ignore]`。

use super::keyring::{
    CredentialEnvelope, KeyringStore, MemoryKeyring, commit_keys, credential_account,
    delete_credentials, fingerprint, intent_record, read_envelope,
};
use crate::util::errors::{E_INVALID_ARG, IpcError};

#[test]
fn 账户命名按协议() {
    assert_eq!(credential_account("abc-123"), "craftwave.provider.abc-123");
}

#[test]
fn 首次提交generation为1且envelope可读回() {
    let store = MemoryKeyring::new();
    let meta = commit_keys(&store, "p1", &["sk-test-001".into()]).unwrap();
    assert_eq!(meta.credential_ref, "craftwave.provider.p1");
    assert_eq!(meta.credential_count, 1);

    let envelope = read_envelope(&store, "p1").unwrap().unwrap();
    assert_eq!(envelope.generation, 1);
    assert_eq!(envelope.keys, vec!["sk-test-001".to_string()]);
}

#[test]
fn 重复提交generation递增且数组整体覆写() {
    let store = MemoryKeyring::new();
    commit_keys(&store, "p1", &["sk-old".into()]).unwrap();
    commit_keys(&store, "p1", &["sk-new-1".into(), "sk-new-2".into()]).unwrap();
    let envelope = read_envelope(&store, "p1").unwrap().unwrap();
    assert_eq!(envelope.generation, 2, "generation 随提交递增");
    assert_eq!(
        envelope.keys,
        vec!["sk-new-1".to_string(), "sk-new-2".to_string()],
        "整体覆写（§11.2）"
    );
}

#[test]
fn 读回校验失败返回keyring错误码() {
    // 注入式 mock：set 之后 get 返回被篡改内容，模拟读回不一致
    struct TamperingStore {
        inner: MemoryKeyring,
    }
    impl KeyringStore for TamperingStore {
        fn set_secret(&self, account: &str, secret: &str) -> Result<(), IpcError> {
            self.inner.set_secret(account, secret)
        }
        fn get_secret(&self, account: &str) -> Result<Option<String>, IpcError> {
            Ok(self
                .inner
                .get_secret(account)?
                .map(|s| s.replace("sk-real", "sk-flip")))
        }
        fn delete_secret(&self, account: &str) -> Result<(), IpcError> {
            self.inner.delete_secret(account)
        }
    }
    let store = TamperingStore {
        inner: MemoryKeyring::new(),
    };
    let err = commit_keys(&store, "p1", &["sk-real-key".into()]).unwrap_err();
    assert_eq!(err.code, "E_KEYRING");
}

#[test]
fn 空key列表与空白key拒绝() {
    let store = MemoryKeyring::new();
    let err = commit_keys(&store, "p1", &[]).unwrap_err();
    assert_eq!(err.code, E_INVALID_ARG);
    let err = commit_keys(&store, "p1", &["   ".into()]).unwrap_err();
    assert_eq!(err.code, E_INVALID_ARG);
    // 失败后不落任何东西
    assert_eq!(read_envelope(&store, "p1").unwrap(), None);
}

#[test]
fn 删除幂等且删除后读不到() {
    let store = MemoryKeyring::new();
    commit_keys(&store, "p1", &["sk-1".into()]).unwrap();
    delete_credentials(&store, "p1").unwrap();
    assert_eq!(read_envelope(&store, "p1").unwrap(), None);
    // 幂等：再删一次也成功
    delete_credentials(&store, "p1").unwrap();
}

#[test]
fn envelope明文永不等于secret原文_指纹截断() {
    let envelope = CredentialEnvelope {
        generation: 1,
        keys: vec!["sk-visible".into()],
    };
    let secret = serde_json::to_string(&envelope).unwrap();
    assert!(
        secret.contains("sk-visible"),
        "钥匙串内存的是 envelope JSON（钥匙串侧本身受系统保护）"
    );
    let fp = fingerprint(&secret);
    assert_eq!(fp.len(), 16, "指纹为 sha256 前 16 位截断（非全文）");
    assert_ne!(fingerprint("other"), fp);
}

#[test]
fn 意向记录含三字段且不含key明文() {
    let record = intent_record("op-1", "p1", &fingerprint("sk-x"));
    assert_eq!(record["opId"], "op-1");
    assert_eq!(record["providerId"], "p1");
    assert!(record["keyFingerprint"].is_string());
    assert!(
        record.to_string().find("sk-x").is_none(),
        "意向记录不含 Key 明文"
    );
}

/// 真实钥匙串冒烟（CI 无钥匙串服务，跳过）：`cargo test -- --ignored`
#[test]
#[ignore = "真实钥匙串冒烟：CI 无钥匙串服务，本地手动执行"]
fn os_keyring_真实读写删冒烟() {
    use super::keyring::OsKeyring;
    let store = OsKeyring;
    let provider_id = format!("smoke-{}", uuid::Uuid::new_v4());
    let meta = commit_keys(&store, &provider_id, &["sk-smoke-值".into()]).expect("写入系统钥匙串");
    assert_eq!(meta.credential_count, 1);
    let envelope = read_envelope(&store, &provider_id)
        .expect("读回")
        .expect("条目应存在");
    assert_eq!(envelope.keys, vec!["sk-smoke-值".to_string()]);
    delete_credentials(&store, &provider_id).expect("清理");
    assert_eq!(read_envelope(&store, &provider_id).unwrap(), None);
}
