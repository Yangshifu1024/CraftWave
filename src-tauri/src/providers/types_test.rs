//! CW schema v2 类型与归一化测试（独立测试文件，源码零测试）。

use serde_json::json;

use super::types::*;
use crate::util::error::{E_INVALID_ARG, IpcError};

fn sample_provider() -> ProviderConfig {
    ProviderConfig {
        id: "11111111-1111-1111-1111-111111111111".into(),
        name: "DeepSeek".into(),
        api_format: ApiFormat::OpenaiChat,
        base_url: Some("https://api.deepseek.com".into()),
        credential_ref: Some("craftwave.provider.1111".into()),
        credential_count: 1,
        models: vec![ProviderModel {
            id: "22222222-2222-2222-2222-222222222222".into(),
            model: "deepseek-chat".into(),
            max_tokens: 8192,
            context_window: 128_000,
            reasoning_effort: None,
            vision: Some(false),
        }],
        headers: None,
    }
}

#[test]
fn schema_roundtrip_camel_case字段无null满字段() {
    let mut config = AppConfig {
        providers: vec![sample_provider()],
        active_model_id: Some("22222222-2222-2222-2222-222222222222".into()),
        ..AppConfig::default()
    };
    config.normalize().unwrap();

    let serialized = serde_json::to_value(&config).unwrap();
    assert_eq!(serialized["schemaVersion"], 2);
    assert_eq!(serialized["providers"][0]["apiFormat"], "openai_chat");
    assert_eq!(
        serialized["providers"][0]["baseUrl"],
        "https://api.deepseek.com"
    );
    assert_eq!(
        serialized["providers"][0]["credentialRef"],
        "craftwave.provider.1111"
    );
    assert_eq!(serialized["providers"][0]["models"][0]["maxTokens"], 8192);
    // 可选字段缺省时不出现在 JSON（"无 null 满字段"）
    let minimal: AppConfig = AppConfig::default();
    let minimal_json = serde_json::to_value(&minimal).unwrap();
    assert!(minimal_json.get("activeModelId").is_none());
    assert!(minimal_json.get("proxy").is_none());

    // 反序列化还原
    let deserialized: AppConfig = serde_json::from_value(serialized).unwrap();
    assert_eq!(deserialized, config);
}

#[test]
fn 反序列化容忍缺省节并归一化补默认值() {
    let value = json!({
        "schemaVersion": 2,
        "providers": [{
            "id": "p1",
            "name": "  自定义端点  ",
            "apiFormat": "openai_chat",
            "baseUrl": "https://llm.example.com/v1",
            "credentialCount": 0,
            "models": [{ "id": "m1", "model": "qwen3", "maxTokens": 0, "contextWindow": 0 }]
        }]
    });
    let mut config: AppConfig = serde_json::from_value(value).unwrap();
    assert_eq!(config.ui.language, Language::ZhCn, "缺省 ui 节 → 默认值");
    assert!(config.approval.enabled, "缺省 approval 节 → 审批默认开");
    config.normalize().unwrap();
    let p = &config.providers[0];
    assert_eq!(p.name, "自定义端点", "name 去首尾空白");
    assert_eq!(p.models[0].max_tokens, DEFAULT_MAX_TOKENS, "0 → 默认 32768");
    assert_eq!(
        p.models[0].context_window, DEFAULT_CONTEXT_WINDOW,
        "0 → 默认 128000"
    );
}

#[test]
fn schema版本非2拒绝() {
    let mut config = AppConfig {
        schema_version: 1,
        ..AppConfig::default()
    };
    let err = config.normalize().unwrap_err();
    assert_eq!(err.code, E_INVALID_ARG);
}

#[test]
fn api_format三方言枚举全兼容() {
    for (raw, expected) in [
        ("openai_chat", ApiFormat::OpenaiChat),
        ("anthropic_messages", ApiFormat::AnthropicMessages),
        ("openai_responses", ApiFormat::OpenaiResponses),
    ] {
        let p: ProviderConfig = serde_json::from_str(&format!(
            r#"{{ "id": "p", "name": "n", "apiFormat": "{raw}", "credentialCount": 0, "models": [{{"id":"m","model":"x"}}] }}"#
        ))
        .unwrap();
        assert_eq!(p.api_format, expected);
    }
    let bad = serde_json::from_str::<ProviderConfig>(
        r#"{ "id": "p", "name": "n", "apiFormat": "gemini", "credentialCount": 0, "models": [] }"#,
    );
    assert!(bad.is_err(), "未知 apiFormat 必须拒绝");
}

#[test]
fn base_url仅接受http_s() {
    let mut config = AppConfig {
        providers: vec![ProviderConfig {
            base_url: Some("ftp://evil".into()),
            ..sample_provider()
        }],
        ..AppConfig::default()
    };
    let err = config.normalize().unwrap_err();
    assert_eq!(err.code, E_INVALID_ARG);
}

#[test]
fn active_model_id悬空自愈为空() {
    let mut config = AppConfig {
        providers: vec![sample_provider()],
        active_model_id: Some("ghost-model".into()),
        ..AppConfig::default()
    };
    config.normalize().unwrap();
    assert_eq!(config.active_model_id, None);
}

#[test]
fn 凭据数量超上限拒绝() {
    let mut config = AppConfig {
        providers: vec![ProviderConfig {
            credential_count: (MAX_KEYS_PER_PROVIDER + 1) as u32,
            ..sample_provider()
        }],
        ..AppConfig::default()
    };
    assert!(matches!(config.normalize(), Err(IpcError { .. })));
}

#[test]
fn 官方缺省地址按方言给出() {
    assert_eq!(
        ApiFormat::OpenaiChat.official_base_url(),
        "https://api.openai.com/v1"
    );
    assert_eq!(
        ApiFormat::AnthropicMessages.official_base_url(),
        "https://api.anthropic.com"
    );
    assert_eq!(
        ApiFormat::OpenaiResponses.official_base_url(),
        "https://api.openai.com/v1"
    );
}
