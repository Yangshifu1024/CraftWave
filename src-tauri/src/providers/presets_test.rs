//! 内置供应商预设清单测试。

use super::presets::{builtin_presets, find_preset, preset_to_provider};
use crate::providers::types::{ApiFormat, SCHEMA_VERSION};

// 断言：内置预设 slug 全局唯一、baseUrl 一律 https、每家至少带一个建议模型。
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

// 断言：从预设实例化出的 ProviderConfig 字段齐全合法（Key 留空、id 为 uuid、
// 窗口参数为正数），且能通过 schema v2 归一化。
#[test]
fn preset_instantiation_valid_provider_config() {
    let preset = find_preset("deepseek").expect("deepseek 预设存在");
    let provider = preset_to_provider(preset);
    assert_eq!(provider.name, "DeepSeek");
    assert_eq!(provider.api_format, ApiFormat::OpenaiChat);
    assert!(provider.api_key.is_none(), "Key 留空待设置页填写");
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

// 断言：按不存在的 slug 查预设返回 None（不报错）。
#[test]
fn unknown_slug_returns_empty() {
    assert!(find_preset("nope").is_none());
}
