//! provider/settings 服务编排测试（协调器组装层；keyring 已废除，
//! Key 明文随 config.json 落盘并收紧 0600）。全链路合并为单个 #[test]，
//! 断言消息带阶段标识。

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
        local: false,
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

// 断言：供应商设置服务全链路——save_provider 带 newKey 时 Key 明文入
// config.json 且 unix 下文件 0600；不带 newKey 时入参 apiKey 原样承载
//（整份回传口径）、新供应商不带 Key 则为空；同 id 原位替换不追加；空白
// newKey 与空白 id 均 E_INVALID_ARG；delete_provider 移除配置条目、删不存在
// 报 E_NOT_FOUND；save_config 返回归一化配置、get_config 可读回；8 线程并发
// save_provider 全部落盘无丢失（读-改-写锁内串行）。
#[test]
fn provider_settings_service_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let store = fixture(dir.path());

    // 带 newKey：明文落盘 + unix 0600
    let saved = save_provider(
        &store,
        demo_provider("p1", "test-model"),
        Some("sk-live-001".into()),
    )
    .unwrap();
    assert_eq!(saved.api_key.as_deref(), Some("sk-live-001"));
    let raw = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    assert!(raw.contains("sk-live-001"), "Key 明文在 config.json");
    #[cfg(unix)]
    assert_eq!(mode_of(&dir.path().join("config.json")), 0o600);

    // 不带 newKey：入参 apiKey 原样承载 + 元数据更新生效
    let mut provider = demo_provider("p1", "m2");
    provider.api_key = Some("sk-a".into());
    let saved = save_provider(&store, provider, None).unwrap();
    assert_eq!(saved.api_key.as_deref(), Some("sk-a"), "整份回传口径");
    assert_eq!(saved.models[0].model, "m2");

    // 同 id 原位替换不追加
    let saved = save_provider(&store, demo_provider("p1", "v2"), None).unwrap();
    let config = store.load().unwrap();
    assert_eq!(config.providers.len(), 1, "同 id 原位替换");
    assert_eq!(saved.models[0].model, "v2");

    // 参数校验：空白 newKey / 空白 id 均 E_INVALID_ARG
    let err = save_provider(&store, demo_provider("p2", "m"), Some("  ".into())).unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
    let mut bad_id = demo_provider("p3", "m");
    bad_id.id = " ".into();
    let err = save_provider(&store, bad_id, None).unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");

    // 新供应商不带 Key：apiKey 为空
    let fresh = save_provider(&store, demo_provider("p4", "m"), None).unwrap();
    assert!(fresh.api_key.is_none());

    // 删除：条目与 Key 随配置消失；删不存在报 E_NOT_FOUND
    save_provider(&store, demo_provider("p5", "m"), Some("sk-x".into())).unwrap();
    delete_provider(&store, "p5").unwrap();
    let config = store.load().unwrap();
    assert!(
        config.providers.iter().all(|p| p.id != "p5"),
        "删除后条目消失"
    );
    let err = delete_provider(&store, "p5").unwrap_err();
    assert_eq!(err.code, E_NOT_FOUND);

    // save_config / get_config：归一化返回 + 读回
    let mut config = AppConfig::default();
    config.providers.push(demo_provider("p9", "m"));
    config.ui.font_size = Some(18);
    let saved = save_config(&store, config).unwrap();
    assert_eq!(saved.providers[0].models[0].max_tokens, 8192);
    #[cfg(unix)]
    assert_eq!(mode_of(&dir.path().join("config.json")), 0o600);
    let loaded = get_config(&store).unwrap();
    assert_eq!(loaded.providers.len(), 1);
    assert_eq!(
        loaded.ui.font_size,
        Some(18),
        "字号经真实配置文件保存后可读回"
    );

    // 并发 save_provider：全部落盘无丢失（独立目录，避免与前面段落共享 config.json）
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
    let config = store.load().unwrap();
    let mut ids: Vec<_> = config.providers.iter().map(|p| p.id.clone()).collect();
    ids.sort();
    let mut expect: Vec<_> = (0..8).map(|i| format!("p{i}")).collect();
    expect.sort();
    assert_eq!(ids, expect, "并发提交无丢失");
}
