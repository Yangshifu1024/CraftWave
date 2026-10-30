//! 内置供应商预设清单测试。

use super::presets::{builtin_presets, find_preset, preset_to_provider};
use crate::providers::types::{ApiFormat, SCHEMA_VERSION};

// 断言：内置预设 slug 全局唯一、远端预设 baseUrl 一律 https、本地预设走
// http 回环、每家至少带一个建议模型。
#[test]
fn preset_slugs_unique_and_urls_well_formed() {
    let presets = builtin_presets();
    assert!(
        presets.len() >= 9,
        "M2 至少内置 9 家预设（6 远端 + 3 本地，19 号票）"
    );
    let mut slugs: Vec<_> = presets.iter().map(|p| p.slug).collect();
    slugs.sort_unstable();
    let before = slugs.len();
    slugs.dedup();
    assert_eq!(before, slugs.len(), "slug 必须唯一");
    for preset in presets {
        assert!(!preset.models.is_empty(), "{} 应带建议模型", preset.slug);
        if preset.local {
            // 本地预设（FR-02-04）：http 回环 + 免 Key 标记
            assert!(
                preset.base_url.starts_with("http://127.0.0.1:"),
                "{} 的 baseUrl 必须是本机回环地址",
                preset.slug
            );
        } else {
            assert!(
                preset.base_url.starts_with("https://"),
                "{} 的 baseUrl 必须 https",
                preset.slug
            );
        }
    }
}

// 断言：本地模型三家（FR-02-04，19 号票）——slug 齐全、local=true、
// openai_chat 兼容方言、默认端口对齐各家官方缺省值。
#[test]
fn local_presets_three_vendors() {
    let cases = [
        ("ollama", "http://127.0.0.1:11434/v1"),
        ("llamacpp", "http://127.0.0.1:8080/v1"),
        ("lmstudio", "http://127.0.0.1:1234/v1"),
    ];
    for (slug, expected_base) in cases {
        let preset = find_preset(slug).unwrap_or_else(|| panic!("{slug} 预设存在"));
        assert!(preset.local, "{slug} 应标记 local");
        assert_eq!(preset.base_url, expected_base);
        assert!(
            matches!(preset.api_format, ApiFormat::OpenaiChat),
            "{slug} 走 openai_chat 兼容方言"
        );
        assert!(!preset.name.is_empty());
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
    assert!(!provider.local, "远端预设 local=false");
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

// 断言：本地预设免 Key 实例化（FR-02-04）——api_key 为 None + local=true
// 的组合能过归一化（19 号票免 Key 校验的预设侧数据源）。
#[test]
fn local_preset_instantiates_keyless() {
    let preset = find_preset("ollama").expect("ollama 预设存在");
    let provider = preset_to_provider(preset);
    assert!(provider.local, "local 随预设带出");
    assert!(provider.api_key.is_none(), "本地预设无 Key");
    let mut config = crate::providers::types::AppConfig {
        schema_version: SCHEMA_VERSION,
        providers: vec![provider],
        ..crate::providers::types::AppConfig::default()
    };
    config.normalize().expect("免 Key 本地供应商应通过归一化");
}

// 断言：按不存在的 slug 查预设返回 None（不报错）。
#[test]
fn unknown_slug_returns_empty() {
    assert!(find_preset("nope").is_none());
}
