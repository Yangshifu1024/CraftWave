//! 内置供应商预设清单测试。全链路合并为单个 #[test]，断言消息带阶段标识。

use super::presets::{builtin_presets, find_preset, preset_to_provider};
use crate::providers::types::{ApiFormat, SCHEMA_VERSION};

// 断言：预设清单与实例化全语义——slug 全局唯一、远端预设 baseUrl 一律 https、
// 本地预设走 http 回环、每家至少带一个建议模型、清单至少 9 家（6 远端 + 3 本地）；
// 本地模型三家（FR-02-04）slug/端口/方言对齐官方缺省；deepseek 实例化出字段
// 齐全合法的 ProviderConfig（Key 留空、id 为 uuid、窗口参数为正数）且能过
// schema v2 归一化；ollama 免 Key 实例化（api_key=None + local=true 组合可过
// 归一化）；按不存在的 slug 查询返回 None 不报错。
#[test]
fn presets_inventory_and_instantiation() {
    // 清单唯一性与 URL 形态
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

    // 本地三家：slug、local、openai_chat 方言、默认端口对齐官方缺省
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

    // 远端实例化：字段齐全合法且能过 schema v2 归一化
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
    let mut config = crate::providers::types::AppConfig {
        schema_version: SCHEMA_VERSION,
        providers: vec![provider],
        ..crate::providers::types::AppConfig::default()
    };
    config.normalize().unwrap();

    // 本地免 Key 实例化（FR-02-04）：api_key=None + local=true 可过归一化
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

    // 未知 slug：返回 None 不报错
    assert!(find_preset("nope").is_none());
}
