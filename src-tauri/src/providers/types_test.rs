//! CW schema v2 类型与归一化测试（独立测试文件，源码零测试）。

use serde_json::json;

use super::types::*;
use crate::util::errors::E_INVALID_ARG;

/// 测试辅助：构造一个合法的 DeepSeek 供应商配置样本（含 Key 与单模型）。
fn sample_provider() -> ProviderConfig {
    ProviderConfig {
        id: "11111111-1111-1111-1111-111111111111".into(),
        name: "DeepSeek".into(),
        api_format: ApiFormat::OpenaiChat,
        base_url: Some("https://api.deepseek.com".into()),
        api_key: Some("sk-test-0123456789".into()),
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

// 断言：配置序列化为 camelCase JSON 且可选字段缺省时不出 null；
// 序列化结果能原样反序列化还原（往返一致）。
#[test]
fn schema_roundtrip_camel_no_null_full_fields() {
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
    assert_eq!(serialized["providers"][0]["apiKey"], "sk-test-0123456789");
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

// 断言：反序列化容忍字段缺失（各节点取缺省值），归一化后 name 去空白、
// 窗口参数 0 被填为默认值。
#[test]
fn deserialize_tolerates_missing_fills_defaults() {
    let value = json!({
        "schemaVersion": 2,
        "providers": [{
            "id": "p1",
            "name": "  自定义端点  ",
            "apiFormat": "openai_chat",
            "baseUrl": "https://llm.example.com/v1",
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

// 断言：schemaVersion 不是 2 的配置在归一化时被拒绝（E_INVALID_ARG）。
#[test]
fn schema_version_not_two_rejected() {
    let mut config = AppConfig {
        schema_version: 1,
        ..AppConfig::default()
    };
    let err = config.normalize().unwrap_err();
    assert_eq!(err.code, E_INVALID_ARG);
}

// 断言：三种 apiFormat 方言字符串都能反序列化，未知值（如 "gemini"）被拒绝。
#[test]
fn api_format_three_dialects_compatible() {
    for (raw, expected) in [
        ("openai_chat", ApiFormat::OpenaiChat),
        ("anthropic_messages", ApiFormat::AnthropicMessages),
        ("openai_responses", ApiFormat::OpenaiResponses),
    ] {
        let p: ProviderConfig = serde_json::from_str(&format!(
            r#"{{ "id": "p", "name": "n", "apiFormat": "{raw}", "models": [{{"id":"m","model":"x"}}] }}"#
        ))
        .unwrap();
        assert_eq!(p.api_format, expected);
    }
    let bad = serde_json::from_str::<ProviderConfig>(
        r#"{ "id": "p", "name": "n", "apiFormat": "gemini", "models": [] }"#,
    );
    assert!(bad.is_err(), "未知 apiFormat 必须拒绝");
}

// 断言：baseUrl 只接受 http(s)，其他协议（ftp://）在归一化时被拒绝。
#[test]
fn base_url_only_http_s() {
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

// 断言：activeModelId 指向不存在的模型（悬空）时归一化自愈为 None，不报错。
#[test]
fn active_model_id_dangling_heals_to_none() {
    let mut config = AppConfig {
        providers: vec![sample_provider()],
        active_model_id: Some("ghost-model".into()),
        ..AppConfig::default()
    };
    config.normalize().unwrap();
    assert_eq!(config.active_model_id, None);
}

// 断言：全空白 apiKey 归一化为 None（不留下空串 Key）。
#[test]
fn blank_api_key_normalized_to_none() {
    let mut config = AppConfig {
        providers: vec![ProviderConfig {
            api_key: Some("   ".into()),
            ..sample_provider()
        }],
        ..AppConfig::default()
    };
    config.normalize().unwrap();
    assert!(
        config.providers[0].api_key.is_none(),
        "空白 Key 归一为 None"
    );
}

// 断言：三种方言的 official_base_url 分别返回正确的官方缺省地址。
#[test]
fn official_base_url_by_dialect() {
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

#[test]
fn ui_font_size_roundtrip_and_bounds() {
    let mut config = AppConfig::default();
    assert_eq!(config.ui.font_size, None, "旧配置沿用默认字号");
    for size in [12, 14, 18] {
        config.ui.font_size = Some(size);
        config.normalize().unwrap();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["ui"]["fontSize"], size);
        assert_eq!(serde_json::from_value::<AppConfig>(value).unwrap(), config);
    }
    for size in [0, 11, 19, 255] {
        config.ui.font_size = Some(size);
        assert_eq!(config.normalize().unwrap_err().code, E_INVALID_ARG);
    }
}

// 断言（03 号票）：agent.customPrompt 的 normalize 三态——非空白保留并
// trim、空白归一为 None（零注入）、agent 组整体缺省容忍（旧配置无该键）。
#[test]
fn agent_custom_prompt_normalize_and_roundtrip() {
    use crate::providers::types::AgentConfig;

    // 非空白：保留并去首尾空白；serde 往返保持 camelCase 键面
    let mut config = AppConfig {
        agent: Some(AgentConfig {
            custom_prompt: Some("  始终用中文回复  ".into()),
            max_concurrent_subagents: crate::providers::types::DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        }),
        ..AppConfig::default()
    };
    config.normalize().unwrap();
    assert_eq!(
        config.agent.as_ref().unwrap().custom_prompt.as_deref(),
        Some("始终用中文回复")
    );
    let value = serde_json::to_value(&config).unwrap();
    assert_eq!(value["agent"]["customPrompt"], "始终用中文回复");
    assert_eq!(serde_json::from_value::<AppConfig>(value).unwrap(), config);

    // 空白形态：全部归一为 None（"没填"与"填了又删掉"等价零注入）
    for blank in ["", "   ", " \n\t "] {
        let mut config = AppConfig {
            agent: Some(AgentConfig {
                custom_prompt: Some(blank.into()),
                max_concurrent_subagents: crate::providers::types::DEFAULT_MAX_CONCURRENT_SUBAGENTS,
            }),
            ..AppConfig::default()
        };
        config.normalize().unwrap();
        assert!(
            config.agent.as_ref().unwrap().custom_prompt.is_none(),
            "空白 customPrompt {blank:?} 应归一为 None"
        );
    }

    // agent 组缺省：旧配置（无 agent 键）反序列化容忍且 normalize 通过
    let mut config = AppConfig::default();
    assert!(config.agent.is_none());
    config.normalize().unwrap();
    let raw = r#"{"schemaVersion":2,"providers":[],"approval":{"enabled":true,"autoConfirm":false,"commandAllowlist":[]},"ui":{"language":"zh-CN","theme":"system"},"log":{"level":"info"},"notifications":{"enabled":true,"interventionOnly":false}}"#;
    let parsed: AppConfig = serde_json::from_str(raw).unwrap();
    assert!(parsed.agent.is_none(), "旧配置无 agent 键应容忍");
}

// 断言（07 号票）：tools.maxParallel 的 normalize 与缺省——缺省 4、合法
// 区间 1–16，越界（0 / 超上限）拒绝整份提交；旧配置无 tools 键容忍。
#[test]
fn tools_max_parallel_defaults_and_bounds() {
    // 缺省：ToolsConfig::default() = 4（§3.2）
    let mut config = AppConfig::default();
    assert_eq!(config.tools.max_parallel, DEFAULT_MAX_PARALLEL);
    config.normalize().unwrap();

    // serde 缺省容忍：旧配置（无 tools 键）反序列化 → maxParallel = 4
    let raw = r#"{"schemaVersion":2,"providers":[],"approval":{"enabled":true,"autoConfirm":false,"commandAllowlist":[]},"ui":{"language":"zh-CN","theme":"system"},"log":{"level":"info"},"notifications":{"enabled":true,"interventionOnly":false}}"#;
    let parsed: AppConfig = serde_json::from_str(raw).unwrap();
    assert_eq!(parsed.tools.max_parallel, 4, "旧配置无 tools 键 → 缺省 4");

    // 合法边界：1 与 16 都通过，且 serde 往返保持 camelCase 键面
    for ok in [1, 4, 16] {
        let mut config = AppConfig::default();
        config.tools.max_parallel = ok;
        config.normalize().unwrap();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["tools"]["maxParallel"], ok);
        assert_eq!(serde_json::from_value::<AppConfig>(value).unwrap(), config);
    }

    // 非法：0（并发度必须为正）与 17（超上限）拒绝整份提交
    for bad in [0, 17, 100] {
        let mut config = AppConfig::default();
        config.tools.max_parallel = bad;
        assert_eq!(
            config.normalize().unwrap_err().code,
            E_INVALID_ARG,
            "maxParallel = {bad} 应拒绝"
        );
    }
}

// 断言（37 号票）：watchdog.stallTimeoutSecs 的 normalize 与缺省——缺省 300、
// 合法区间 5–3600，越界拒绝整份提交；旧配置无 watchdog 键容忍（回填默认）。
#[test]
fn watchdog_stall_timeout_defaults_and_bounds() {
    // 缺省：WatchdogConfig::default() = 300（§3.2 watchdog 组/§5.7）
    let mut config = AppConfig::default();
    assert_eq!(
        config.watchdog.stall_timeout_secs,
        DEFAULT_STALL_TIMEOUT_SECS
    );
    config.normalize().unwrap();

    // serde 缺省容忍：旧配置（无 watchdog 键）反序列化 → stallTimeoutSecs = 300
    let raw = r#"{"schemaVersion":2,"providers":[],"approval":{"enabled":true,"autoConfirm":false,"commandAllowlist":[]},"ui":{"language":"zh-CN","theme":"system"},"log":{"level":"info"},"notifications":{"enabled":true,"interventionOnly":false}}"#;
    let parsed: AppConfig = serde_json::from_str(raw).unwrap();
    assert_eq!(
        parsed.watchdog.stall_timeout_secs, 300,
        "旧配置无 watchdog 键 → 缺省 300"
    );

    // 合法边界：5 与 3600 都通过，且 serde 往返保持 camelCase 键面
    for ok in [5, 300, 3600] {
        let mut config = AppConfig::default();
        config.watchdog.stall_timeout_secs = ok;
        config.normalize().unwrap();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["watchdog"]["stallTimeoutSecs"], ok);
        assert_eq!(serde_json::from_value::<AppConfig>(value).unwrap(), config);
    }

    // 非法：4（低于下限）与 3601（超上限）拒绝整份提交
    for bad in [0, 4, 3601, 100_000] {
        let mut config = AppConfig::default();
        config.watchdog.stall_timeout_secs = bad;
        assert_eq!(
            config.normalize().unwrap_err().code,
            E_INVALID_ARG,
            "stallTimeoutSecs = {bad} 应拒绝"
        );
    }
}

// 断言（10 号票，Q13）：agent.maxConcurrentSubagents 的 normalize 与缺省——
// 旧配置缺 agent 组/缺字段回填 4（正整数语义），0/超限拒绝整份提交，
// save_config 往返保持 camelCase 键面。
#[test]
fn agent_max_concurrent_subagents_defaults_and_bounds() {
    use crate::providers::types::{
        AgentConfig, DEFAULT_MAX_CONCURRENT_SUBAGENTS, MAX_CONCURRENT_SUBAGENTS_LIMIT,
    };

    // 缺省：AgentConfig::default() = 4
    let mut config = AppConfig::default();
    assert_eq!(
        config.agent.as_ref().map(|a| a.max_concurrent_subagents),
        None,
        "agent 组整体缺省 = None"
    );
    config.normalize().unwrap();

    // serde 缺省容忍：旧配置无 agent 键 / agent 组无 maxConcurrentSubagents
    // 键 → 回填 4
    let raw_no_agent = r#"{"schemaVersion":2,"providers":[],"approval":{"enabled":true,"autoConfirm":false,"commandAllowlist":[]},"ui":{"language":"zh-CN","theme":"system"},"log":{"level":"info"},"notifications":{"enabled":true,"interventionOnly":false}}"#;
    let parsed: AppConfig = serde_json::from_str(raw_no_agent).unwrap();
    assert!(parsed.agent.is_none(), "旧配置无 agent 组保持 None");
    let raw_partial = r#"{"schemaVersion":2,"providers":[],"agent":{"customPrompt":"hi"},"approval":{"enabled":true,"autoConfirm":false,"commandAllowlist":[]},"ui":{"language":"zh-CN","theme":"system"},"log":{"level":"info"},"notifications":{"enabled":true,"interventionOnly":false}}"#;
    let parsed: AppConfig = serde_json::from_str(raw_partial).unwrap();
    assert_eq!(
        parsed.agent.as_ref().unwrap().max_concurrent_subagents,
        DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        "旧配置 agent 组缺字段 → serde default 回填 4"
    );

    // 合法边界：1 与 16 通过，serde 往返保持 camelCase 键面
    for ok in [1, 4, MAX_CONCURRENT_SUBAGENTS_LIMIT] {
        let mut config = AppConfig {
            agent: Some(AgentConfig {
                max_concurrent_subagents: ok,
                ..AgentConfig::default()
            }),
            ..AppConfig::default()
        };
        config.normalize().unwrap();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["agent"]["maxConcurrentSubagents"], ok);
        assert_eq!(serde_json::from_value::<AppConfig>(value).unwrap(), config);
    }

    // 非正整数（0）与超上限拒绝整份提交
    for bad in [0, MAX_CONCURRENT_SUBAGENTS_LIMIT + 1, 100_000] {
        let mut config = AppConfig {
            agent: Some(AgentConfig {
                max_concurrent_subagents: bad,
                ..AgentConfig::default()
            }),
            ..AppConfig::default()
        };
        assert_eq!(
            config.normalize().unwrap_err().code,
            E_INVALID_ARG,
            "maxConcurrentSubagents = {bad} 应拒绝"
        );
    }
}
