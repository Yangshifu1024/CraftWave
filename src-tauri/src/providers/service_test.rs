//! provider/settings 服务编排测试（协调器组装层；keyring 已废除，
//! Key 明文随 config.json 落盘并收紧 0600）。

use super::service::{delete_provider, get_config, save_config, save_provider};
use crate::providers::types::{ApiFormat, AppConfig, ProviderConfig, ProviderModel};
use crate::storage::config_store::ConfigStore;
use crate::util::errors::E_NOT_FOUND;

/// 测试辅助：构造一个最小可用的供应商配置（单模型、无 Key）。
fn demo_provider(id: &str, model: &str) -> ProviderConfig {
    ProviderConfig {
        id: id.into(),
        name: format!("供应商-{id}"),
        api_format: ApiFormat::OpenaiChat,
        base_url: Some("https://api.example.com/v1".into()),
        api_key: None,
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

/// 测试辅助：在临时目录下建一个指向 config.json 的配置存储。
fn fixture(dir: &std::path::Path) -> ConfigStore<AppConfig> {
    ConfigStore::new(dir.join("config.json"))
}

/// 测试辅助（仅 unix）：取文件权限位（低 9 位，如 0o600）。
#[cfg(unix)]
fn mode_of(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// 断言：save_provider 带 newKey 时 Key 明文写入 config.json，且 unix 下
// 文件权限收紧为 0600（只有属主可读写）。
#[test]
fn save_provider_new_key_written_plaintext_with_tight_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let store = fixture(dir.path());

    let provider = demo_provider("p1", "test-model");
    let saved = save_provider(&store, provider, Some("sk-live-001".into())).unwrap();
    assert_eq!(saved.api_key.as_deref(), Some("sk-live-001"));

    // Key 明文随 config.json 落盘；文件权限收紧 0600（unix）
    let raw = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(raw.contains("sk-live-001"), "Key 明文在 config.json");
    #[cfg(unix)]
    assert_eq!(mode_of(&dir.path().join("config.json")), 0o600);
}

// 断言：不带 newKey 保存时，入参里的 apiKey 原样承载（整份回传口径）；
// 新供应商不带 Key 则保存后 apiKey 为空。
#[test]
fn save_provider_without_key_keeps_passthrough_api_key() {
    let dir = tempfile::tempdir().unwrap();
    let store = fixture(dir.path());

    // 先带 Key 存
    save_provider(&store, demo_provider("p1", "m"), Some("sk-a".into())).unwrap();
    // 再不带 newKey 保存元数据：入参 apiKey 原样承载（前端整份回传口径）
    let mut provider = demo_provider("p1", "m2");
    provider.api_key = Some("sk-a".into());
    let saved = save_provider(&store, provider, None).unwrap();
    assert_eq!(saved.api_key.as_deref(), Some("sk-a"));
    assert_eq!(saved.models[0].model, "m2", "元数据更新生效");

    // 入参不带 Key 也不回传（新供应商）：apiKey 为空
    let fresh = save_provider(&store, demo_provider("p2", "m"), None).unwrap();
    assert!(fresh.api_key.is_none());
}

// 断言：同 id 再保存是原位替换，不产生重复条目。
#[test]
fn save_provider_existing_id_replaced_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let store = fixture(dir.path());
    save_provider(&store, demo_provider("p1", "v1"), None).unwrap();
    let saved = save_provider(&store, demo_provider("p1", "v2"), None).unwrap();
    let config = store.load().unwrap();
    assert_eq!(config.providers.len(), 1, "同 id 原位替换不追加");
    assert_eq!(saved.models[0].model, "v2");
}

// 断言：空白 newKey 与空白 provider.id 都以 E_INVALID_ARG 拒绝（参数校验）。
#[test]
fn save_provider_rejects_blank_new_key_and_empty_id() {
    let dir = tempfile::tempdir().unwrap();
    let store = fixture(dir.path());
    let err = save_provider(&store, demo_provider("p1", "m"), Some("  ".into())).unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");

    let mut bad_id = demo_provider(" ", "m");
    bad_id.id = " ".into();
    let err = save_provider(&store, bad_id, None).unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
}

// 断言：删除供应商后配置中不再存在（Key 随之消失）；删除不存在的报 E_NOT_FOUND。
#[test]
fn delete_provider_removes_from_config() {
    let dir = tempfile::tempdir().unwrap();
    let store = fixture(dir.path());
    save_provider(&store, demo_provider("p1", "m"), Some("sk-x".into())).unwrap();
    delete_provider(&store, "p1").unwrap();
    let config = store.load().unwrap();
    assert!(config.providers.is_empty(), "Key 随配置一并消失");

    let err = delete_provider(&store, "p1").unwrap_err();
    assert_eq!(err.code, E_NOT_FOUND);
}

// 断言：save_config 返回归一化后的配置，且 unix 下落盘文件权限为 0600；
// get_config 能读回。
#[test]
fn save_config_returns_normalized_and_config_file_0600() {
    let dir = tempfile::tempdir().unwrap();
    let store = fixture(dir.path());
    let mut config = AppConfig::default();
    config.providers.push(demo_provider("p9", "m"));
    let saved = save_config(&store, config).unwrap();
    // demo_provider 的 maxTokens=8192 非零，归一化保留；0→默认由 types_test 覆盖
    assert_eq!(saved.providers[0].models[0].max_tokens, 8192);
    #[cfg(unix)]
    assert_eq!(mode_of(&dir.path().join("config.json")), 0o600);

    let loaded = get_config(&store).unwrap();
    assert_eq!(loaded.providers.len(), 1);
}

// 断言：8 线程并发 save_provider 全部落盘成功、无互相覆盖丢失
// （读-改-写在锁内串行）。
#[test]
fn concurrent_save_provider_all_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(fixture(dir.path()));
    let mut handles = Vec::new();
    for i in 0..8 {
        let store = store.clone();
        handles.push(std::thread::spawn(move || {
            save_provider(&store, demo_provider(&format!("p{i}"), "m"), None).unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    // 终态：全部提交落盘（并发读-改-写在锁内串行，无丢失）
    let config = store.load().unwrap();
    let mut ids: Vec<_> = config.providers.iter().map(|p| p.id.clone()).collect();
    ids.sort();
    let mut expect: Vec<_> = (0..8).map(|i| format!("p{i}")).collect();
    expect.sort();
    assert_eq!(ids, expect, "并发提交无丢失");
}
