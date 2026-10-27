//! provider/settings 服务编排测试（协调器 + keyring mock 组装层）。

use std::sync::Arc;

use super::service::{delete_provider, get_config, save_config, save_provider};
use crate::providers::keyring::{MemoryKeyring, read_envelope};
use crate::providers::types::{ApiFormat, AppConfig, ProviderConfig, ProviderModel};
use crate::storage::config_store::ConfigStore;
use crate::util::errors::E_NOT_FOUND;

fn demo_provider(id: &str, model: &str) -> ProviderConfig {
    ProviderConfig {
        id: id.into(),
        name: format!("供应商-{id}"),
        api_format: ApiFormat::OpenaiChat,
        base_url: Some("https://api.example.com/v1".into()),
        credential_ref: None,
        credential_count: 0,
        models: vec![ProviderModel {
            id: format!("m-{id}"),
            model: model.into(),
            max_tokens: 8192,
            context_window: 128_000,
            reasoning_effort: None,
            vision: None,
        }],
        headers: None,
    }
}

fn fixture(dir: &std::path::Path) -> (ConfigStore<AppConfig>, MemoryKeyring) {
    (
        ConfigStore::new(dir.join("config.json"), dir.join("intent.json")),
        MemoryKeyring::new(),
    )
}

#[test]
fn save_provider_new_key_protocol_ref_not_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let (store, keys) = fixture(dir.path());

    let provider = demo_provider("p1", "test-model");
    let saved = save_provider(&store, &keys, provider, Some("sk-live-001".into()), "op-1").unwrap();
    assert_eq!(
        saved.credential_ref.as_deref(),
        Some("craftwave.provider.p1")
    );
    assert_eq!(saved.credential_count, 1);

    // config 永不存明文 Key
    let raw = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(
        !raw.contains("sk-live-001"),
        "config.json 不得出现 Key 明文"
    );
    // keyring 里是 envelope
    let envelope = read_envelope(&keys, "p1").unwrap().unwrap();
    assert_eq!(envelope.keys, vec!["sk-live-001".to_string()]);
    // 意向已清理
    assert!(store.pending_intent().unwrap().is_none());
}

#[test]
fn save_provider_existing_id_replaced_generation_up() {
    let dir = tempfile::tempdir().unwrap();
    let (store, keys) = fixture(dir.path());
    let p = demo_provider("p1", "m1");
    save_provider(&store, &keys, p.clone(), Some("sk-1".into()), "op-1").unwrap();
    let mut renamed = demo_provider("p1", "m1");
    renamed.name = "改名".into();
    let saved = save_provider(&store, &keys, renamed, Some("sk-2".into()), "op-2").unwrap();
    assert_eq!(saved.name, "改名");
    let envelope = read_envelope(&keys, "p1").unwrap().unwrap();
    assert_eq!(envelope.generation, 2, "数组整体覆写 generation+1（§11.2）");

    let config = get_config(&store).unwrap();
    assert_eq!(config.providers.len(), 1, "同 id 不重复插入");
}

#[test]
fn save_provider_no_key_keeps_credential_ref() {
    let dir = tempfile::tempdir().unwrap();
    let (store, keys) = fixture(dir.path());
    save_provider(
        &store,
        &keys,
        demo_provider("p1", "m1"),
        Some("sk-1".into()),
        "op-1",
    )
    .unwrap();

    let mut meta_only = demo_provider("p1", "m1");
    meta_only.credential_ref = Some("craftwave.provider.p1".into());
    meta_only.credential_count = 1;
    meta_only.name = "仅改名".into();
    let saved = save_provider(&store, &keys, meta_only, None, "op-2").unwrap();
    assert_eq!(saved.name, "仅改名");
    assert_eq!(
        saved.credential_ref.as_deref(),
        Some("craftwave.provider.p1")
    );
    let envelope = read_envelope(&keys, "p1").unwrap().unwrap();
    assert_eq!(envelope.generation, 1, "无 newKey 不触 keyring");
}

#[test]
fn save_provider_keyring_failure_keeps_intent() {
    struct FailingStore;
    impl crate::providers::keyring::KeyringStore for FailingStore {
        fn set_secret(&self, _a: &str, _s: &str) -> Result<(), crate::util::errors::IpcError> {
            Err(crate::util::errors::IpcError::keyring("系统钥匙串锁定"))
        }
        fn get_secret(&self, _a: &str) -> Result<Option<String>, crate::util::errors::IpcError> {
            Ok(None)
        }
        fn delete_secret(&self, _a: &str) -> Result<(), crate::util::errors::IpcError> {
            Ok(())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(
        dir.path().join("config.json"),
        dir.path().join("intent.json"),
    );

    let err = save_provider(
        &store,
        &FailingStore,
        demo_provider("p1", "m1"),
        Some("sk-x".into()),
        "op-1",
    )
    .unwrap_err();
    assert_eq!(err.code, "E_KEYRING");
    assert!(
        get_config(&store).unwrap().providers.is_empty(),
        "config 不变更"
    );
    let pending = store.pending_intent().unwrap().expect("意向保留待重试");
    assert_eq!(pending["providerId"], "p1");
}

#[test]
fn delete_provider_removes_keyring_and_config() {
    let dir = tempfile::tempdir().unwrap();
    let (store, keys) = fixture(dir.path());
    save_provider(
        &store,
        &keys,
        demo_provider("p1", "m1"),
        Some("sk-1".into()),
        "op-1",
    )
    .unwrap();

    delete_provider(&store, &keys, "p1").unwrap();
    assert!(get_config(&store).unwrap().providers.is_empty());
    assert_eq!(read_envelope(&keys, "p1").unwrap(), None, "同步删除账户");
}

#[test]
fn delete_provider_missing_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let (store, keys) = fixture(dir.path());
    let err = delete_provider(&store, &keys, "ghost").unwrap_err();
    assert_eq!(err.code, E_NOT_FOUND);
}

#[test]
fn save_config_returns_normalized() {
    let dir = tempfile::tempdir().unwrap();
    let (store, _keys) = fixture(dir.path());
    let mut config = AppConfig::default();
    config.providers.push(demo_provider("p1", "  m1  "));
    let saved = save_config(&store, config).unwrap();
    assert_eq!(saved.providers[0].models[0].model, "m1");
}

#[test]
fn concurrent_save_provider_all_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(ConfigStore::new(
        dir.path().join("config.json"),
        dir.path().join("intent.json"),
    ));
    let keys = Arc::new(MemoryKeyring::new());
    let mut handles = Vec::new();
    for i in 0..6 {
        let store = store.clone();
        let keys = keys.clone();
        handles.push(std::thread::spawn(move || {
            save_provider(
                &store,
                keys.as_ref(),
                demo_provider(&format!("p{i}"), "m"),
                Some(format!("sk-{i}")),
                &format!("op-{i}"),
            )
        }));
    }
    for h in handles {
        h.join().unwrap().unwrap();
    }
    let config = get_config(&store).unwrap();
    assert_eq!(config.providers.len(), 6, "并发提交全部落盘");
    for i in 0..6 {
        let envelope = read_envelope(keys.as_ref(), &format!("p{i}"))
            .unwrap()
            .unwrap();
        assert_eq!(envelope.generation, 1);
    }
}
