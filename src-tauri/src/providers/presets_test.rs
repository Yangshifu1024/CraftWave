//! 内置供应商预设清单测试。

use super::presets::{builtin_presets, find_preset, preset_to_provider};
use crate::providers::types::{ApiFormat, SCHEMA_VERSION};

#[test]
fn preset_slugs_unique_all_https() {
    let presets = builtin_presets();
    assert!(presets.len() >= 5, "M1 至少内置 5 家预设");
    let mut slugs: Vec<_> = presets.iter().map(|p| p.slug).collect();
    slugs.sort_unstable();
    let before = slugs.len();
    slugs.dedup();
    assert_eq!(before, slugs.len(), "slug 必须唯一");
    for preset in presets {
        assert!(
            preset.base_url.starts_with("https://"),
            "{} 的 baseUrl 必须 https",
            preset.slug
        );
        assert!(!preset.models.is_empty(), "{} 应带建议模型", preset.slug);
    }
}

#[test]
fn preset_instantiation_valid_provider_config() {
    let preset = find_preset("deepseek").expect("deepseek 预设存在");
    let provider = preset_to_provider(preset);
    assert_eq!(provider.name, "DeepSeek");
    assert_eq!(provider.api_format, ApiFormat::OpenaiChat);
    assert_eq!(provider.credential_count, 0, "凭据待 save_provider 提交");
    assert!(provider.credential_ref.is_none());
    assert!(uuid::Uuid::parse_str(&provider.id).is_ok(), "id 为 uuid");
    for model in &provider.models {
        assert!(uuid::Uuid::parse_str(&model.id).is_ok());
        assert!(model.max_tokens > 0 && model.context_window > 0);
    }
    // 实例化结果应能过 schema v2 归一化
    let mut config = crate::providers::types::AppConfig {
        schema_version: SCHEMA_VERSION,
        providers: vec![provider],
        ..crate::providers::types::AppConfig::default()
    };
    config.normalize().unwrap();
}

#[test]
fn unknown_slug_returns_empty() {
    assert!(find_preset("nope").is_none());
}
