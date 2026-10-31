//! CW schema v2 类型与归一化测试（独立测试文件，源码零测试）。
//! 同型用例在单个 #[test] 内顺序/表驱动合并，断言消息带阶段标识。

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
        local: false,
    }
}

/// 测试辅助：反序列化一份「只有必需键」的旧配置 JSON（验证各新增组的 serde 缺省容忍）。
fn legacy_config_raw() -> &'static str {
    r#"{"schemaVersion":2,"providers":[],"approval":{"enabled":true,"autoConfirm":false,"commandAllowlist":[]},"ui":{"language":"zh-CN","theme":"system"},"log":{"level":"info"},"notifications":{"enabled":true,"interventionOnly":false}}"#
}

// 断言：schema v2 核心面——配置序列化为 camelCase JSON 且可选字段缺省时不出
// null、反序列化原样还原（roundtrip）；反序列化容忍字段缺失（各节点取缺省
// 值）且归一化填默认（name 去空白、0 窗口参数回填）；schemaVersion 非 2 拒绝；
// 三种 apiFormat 方言字符串可反序列化、未知值拒绝；baseUrl 只接受 http(s)；
// activeModelId 悬空自愈为 None；空白 apiKey 归一为 None；三方言
// official_base_url 正确。
#[test]
fn schema_serde_and_normalize_semantics() {
    // camelCase 序列化 + 可选字段缺省不出现 + roundtrip
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
    let minimal_json = serde_json::to_value(AppConfig::default()).unwrap();
    assert!(minimal_json.get("activeModelId").is_none(), "缺省不出 null");
    assert!(minimal_json.get("proxy").is_none());
    let deserialized: AppConfig = serde_json::from_value(serialized).unwrap();
    assert_eq!(deserialized, config, "roundtrip 还原");

    // 容忍缺失 + 归一化填默认
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
    let mut parsed: AppConfig = serde_json::from_value(value).unwrap();
    assert_eq!(parsed.ui.language, Language::ZhCn, "缺省 ui 节 → 默认值");
    assert!(parsed.approval.enabled, "缺省 approval 节 → 审批默认开");
    parsed.normalize().unwrap();
    let p = &parsed.providers[0];
    assert_eq!(p.name, "自定义端点", "name 去首尾空白");
    assert_eq!(p.models[0].max_tokens, DEFAULT_MAX_TOKENS, "0 → 默认 32768");
    assert_eq!(
        p.models[0].context_window, DEFAULT_CONTEXT_WINDOW,
        "0 → 默认 128000"
    );

    // schemaVersion 非 2 拒绝
    let mut bad_version = AppConfig {
        schema_version: 1,
        ..AppConfig::default()
    };
    assert_eq!(bad_version.normalize().unwrap_err().code, E_INVALID_ARG);

    // 三方言反序列化 + 未知值拒绝
    for (raw, expected) in [
        ("openai_chat", ApiFormat::OpenaiChat),
        ("anthropic_messages", ApiFormat::AnthropicMessages),
        ("openai_responses", ApiFormat::OpenaiResponses),
    ] {
        let p: ProviderConfig = serde_json::from_str(&format!(
            r#"{{ "id": "p", "name": "n", "apiFormat": "{raw}", "models": [{{"id":"m","model":"x"}}] }}"#
        ))
        .unwrap();
        assert_eq!(p.api_format, expected, "{raw} 应可反序列化");
    }
    assert!(
        serde_json::from_str::<ProviderConfig>(
            r#"{ "id": "p", "name": "n", "apiFormat": "gemini", "models": [] }"#
        )
        .is_err(),
        "未知 apiFormat 必须拒绝"
    );

    // baseUrl 只接受 http(s)
    let mut ftp = AppConfig {
        providers: vec![ProviderConfig {
            base_url: Some("ftp://evil".into()),
            ..sample_provider()
        }],
        ..AppConfig::default()
    };
    assert_eq!(ftp.normalize().unwrap_err().code, E_INVALID_ARG);

    // activeModelId 悬空自愈为 None
    let mut dangling = AppConfig {
        providers: vec![sample_provider()],
        active_model_id: Some("ghost-model".into()),
        ..AppConfig::default()
    };
    dangling.normalize().unwrap();
    assert_eq!(dangling.active_model_id, None, "悬空自愈");

    // 空白 apiKey 归一为 None
    let mut blank = AppConfig {
        providers: vec![ProviderConfig {
            api_key: Some("   ".into()),
            ..sample_provider()
        }],
        ..AppConfig::default()
    };
    blank.normalize().unwrap();
    assert!(blank.providers[0].api_key.is_none(), "空白 Key 归一为 None");

    // 三方言 official_base_url
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

// 断言：各配置组的缺省与边界（表驱动口径：缺省值 + 合法边界 + 越界拒绝 +
// 旧配置缺键容忍 + serde 往返保真）——ui.fontSize（12–18）、agent.customPrompt
// 三态（trim/空白归 None/组缺省容忍）、tools.maxParallel（1–16 缺省 4）、
// tools.batchReadLimit（1–100 缺省 20）、watchdog.stallTimeoutSecs（5–3600
// 缺省 300）、update.intervalHours（1–8760 缺省 24 + enabled 持久）、
// agent.maxConcurrentSubagents（1–上限 缺省 4）、ui.closeToTray（缺省 true）、
// ui.hotkey（缺省 开+alt+space，录制形态归一存储形态）、非法 accelerator
// 保存边界拒绝。
#[test]
fn config_groups_defaults_and_bounds() {
    // ui.fontSize：旧配置沿用默认（None）、合法 12–18 往返、越界拒绝
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
        assert_eq!(
            config.normalize().unwrap_err().code,
            E_INVALID_ARG,
            "fontSize={size}"
        );
    }

    // agent.customPrompt：非空白 trim 保留 + camelCase 往返；空白归 None；
    // agent 组缺省容忍
    use crate::providers::types::AgentConfig;
    let mut config = AppConfig {
        agent: Some(AgentConfig {
            custom_prompt: Some("  始终用中文回复  ".into()),
            max_concurrent_subagents: DEFAULT_MAX_CONCURRENT_SUBAGENTS,
            command_audit: None,
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
    for blank in ["", "   ", " \n\t "] {
        let mut config = AppConfig {
            agent: Some(AgentConfig {
                custom_prompt: Some(blank.into()),
                max_concurrent_subagents: DEFAULT_MAX_CONCURRENT_SUBAGENTS,
                command_audit: None,
            }),
            ..AppConfig::default()
        };
        config.normalize().unwrap();
        assert!(
            config.agent.as_ref().unwrap().custom_prompt.is_none(),
            "空白 customPrompt {blank:?} 应归一为 None"
        );
    }

    // tools.maxParallel：缺省 4、1–16 合法、越界拒绝、旧配置缺键容忍
    let mut config = AppConfig::default();
    assert_eq!(config.tools.max_parallel, DEFAULT_MAX_PARALLEL);
    config.normalize().unwrap();
    let parsed: AppConfig = serde_json::from_str(legacy_config_raw()).unwrap();
    assert_eq!(parsed.tools.max_parallel, 4, "旧配置无 tools 键 → 缺省 4");
    for ok in [1, 4, 16] {
        let mut config = AppConfig::default();
        config.tools.max_parallel = ok;
        config.normalize().unwrap();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["tools"]["maxParallel"], ok);
        assert_eq!(serde_json::from_value::<AppConfig>(value).unwrap(), config);
    }
    for bad in [0, 17, 100] {
        let mut config = AppConfig::default();
        config.tools.max_parallel = bad;
        assert_eq!(
            config.normalize().unwrap_err().code,
            E_INVALID_ARG,
            "maxParallel={bad}"
        );
    }

    // tools.batchReadLimit：缺省 20、1–100 合法、越界拒绝
    let mut config = AppConfig::default();
    assert_eq!(config.tools.batch_read_limit, DEFAULT_BATCH_READ_LIMIT);
    config.normalize().unwrap();
    let raw = r#"{"schemaVersion":2,"providers":[],"tools":{"maxParallel":4},"approval":{"enabled":true,"autoConfirm":false,"commandAllowlist":[]},"ui":{"language":"zh-CN","theme":"system"},"log":{"level":"info"},"notifications":{"enabled":true,"interventionOnly":false}}"#;
    let parsed: AppConfig = serde_json::from_str(raw).unwrap();
    assert_eq!(
        parsed.tools.batch_read_limit, 20,
        "旧配置无 batchReadLimit → 缺省 20"
    );
    for ok in [1, 20, 100] {
        let mut config = AppConfig::default();
        config.tools.batch_read_limit = ok;
        config.normalize().unwrap();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["tools"]["batchReadLimit"], ok);
        assert_eq!(serde_json::from_value::<AppConfig>(value).unwrap(), config);
    }
    for bad in [0, 101] {
        let mut config = AppConfig::default();
        config.tools.batch_read_limit = bad;
        assert_eq!(
            config.normalize().unwrap_err().code,
            E_INVALID_ARG,
            "batchReadLimit={bad}"
        );
    }

    // watchdog.stallTimeoutSecs：缺省 300、5–3600 合法、越界拒绝
    let mut config = AppConfig::default();
    assert_eq!(
        config.watchdog.stall_timeout_secs,
        DEFAULT_STALL_TIMEOUT_SECS
    );
    config.normalize().unwrap();
    let parsed: AppConfig = serde_json::from_str(legacy_config_raw()).unwrap();
    assert_eq!(
        parsed.watchdog.stall_timeout_secs, 300,
        "旧配置无 watchdog 键 → 缺省 300"
    );
    for ok in [5, 300, 3600] {
        let mut config = AppConfig::default();
        config.watchdog.stall_timeout_secs = ok;
        config.normalize().unwrap();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["watchdog"]["stallTimeoutSecs"], ok);
        assert_eq!(serde_json::from_value::<AppConfig>(value).unwrap(), config);
    }
    for bad in [0, 4, 3601, 100_000] {
        let mut config = AppConfig::default();
        config.watchdog.stall_timeout_secs = bad;
        assert_eq!(
            config.normalize().unwrap_err().code,
            E_INVALID_ARG,
            "stallTimeoutSecs={bad}"
        );
    }

    // update.intervalHours：缺省 24、1–8760 合法、越界拒绝、enabled 可持久
    use crate::providers::types::DEFAULT_UPDATE_INTERVAL_HOURS;
    let mut config = AppConfig::default();
    assert!(config.update.enabled, "更新检查缺省启用");
    assert_eq!(config.update.interval_hours, DEFAULT_UPDATE_INTERVAL_HOURS);
    config.normalize().unwrap();
    let parsed: AppConfig = serde_json::from_str(legacy_config_raw()).unwrap();
    assert!(parsed.update.enabled, "旧配置无 update 键 → 缺省启用");
    assert_eq!(parsed.update.interval_hours, 24, "缺省 24h");
    for ok in [1, 24, 8760] {
        let mut config = AppConfig::default();
        config.update.interval_hours = ok;
        config.normalize().unwrap();
        let value = serde_json::to_value(&config).unwrap();
        assert_eq!(value["update"]["intervalHours"], ok);
        assert_eq!(value["update"]["enabled"], true);
        assert_eq!(serde_json::from_value::<AppConfig>(value).unwrap(), config);
    }
    let mut config = AppConfig::default();
    config.update.enabled = false;
    let value = serde_json::to_value(&config).unwrap();
    assert_eq!(value["update"]["enabled"], false);
    assert_eq!(serde_json::from_value::<AppConfig>(value).unwrap(), config);
    for bad in [0, 8761] {
        let mut config = AppConfig::default();
        config.update.interval_hours = bad;
        assert_eq!(
            config.normalize().unwrap_err().code,
            E_INVALID_ARG,
            "intervalHours={bad}"
        );
    }

    // agent.maxConcurrentSubagents：缺省 4、1–上限 合法、0/超限拒绝、
    // 旧配置缺组保持 None / 缺字段回填 4
    use crate::providers::types::{
        DEFAULT_MAX_CONCURRENT_SUBAGENTS as DEFAULT_SUBAGENTS, MAX_CONCURRENT_SUBAGENTS_LIMIT,
    };
    let mut config = AppConfig::default();
    assert_eq!(
        config.agent.as_ref().map(|a| a.max_concurrent_subagents),
        None,
        "agent 组整体缺省 = None"
    );
    config.normalize().unwrap();
    let parsed: AppConfig = serde_json::from_str(legacy_config_raw()).unwrap();
    assert!(parsed.agent.is_none(), "旧配置无 agent 组保持 None");
    let raw = r#"{"schemaVersion":2,"providers":[],"agent":{"customPrompt":"hi"},"approval":{"enabled":true,"autoConfirm":false,"commandAllowlist":[]},"ui":{"language":"zh-CN","theme":"system"},"log":{"level":"info"},"notifications":{"enabled":true,"interventionOnly":false}}"#;
    let parsed: AppConfig = serde_json::from_str(raw).unwrap();
    assert_eq!(
        parsed.agent.as_ref().unwrap().max_concurrent_subagents,
        DEFAULT_SUBAGENTS,
        "agent 组缺字段 → serde default 回填 4"
    );
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
            "maxConcurrentSubagents={bad}"
        );
    }

    // ui.closeToTray：旧配置缺字段缺省 true（零迁移）；显式 false 往返保真
    let legacy = json!({
        "schemaVersion": 2,
        "ui": { "language": "zh-CN", "theme": "system" }
    });
    let config = serde_json::from_value::<AppConfig>(legacy).unwrap();
    assert!(config.ui.close_to_tray, "旧配置缺字段应回填 true");
    let mut config = AppConfig::default();
    config.ui.close_to_tray = false;
    config.normalize().unwrap();
    let value = serde_json::to_value(&config).unwrap();
    assert_eq!(value["ui"]["closeToTray"], false);
    assert!(
        !serde_json::from_value::<AppConfig>(value)
            .unwrap()
            .ui
            .close_to_tray
    );

    // ui.hotkey：旧配置缺省（开 + alt+space）；录制形态归一存储形态
    let legacy = json!({
        "schemaVersion": 2,
        "ui": { "language": "zh-CN", "theme": "system" }
    });
    let config = serde_json::from_value::<AppConfig>(legacy).unwrap();
    assert!(config.ui.hotkey.enabled);
    assert_eq!(config.ui.hotkey.accelerator, "alt+space");
    let mut config = AppConfig::default();
    config.ui.hotkey.accelerator = "Shift+Ctrl+P".into();
    config.normalize().unwrap();
    assert_eq!(config.ui.hotkey.accelerator, "ctrl+shift+p", "归一存储形态");

    // 非法 accelerator 在保存边界拒绝整份提交
    for bad in ["", "ctrl+shift", "alt++space"] {
        let mut config = AppConfig::default();
        config.ui.hotkey.accelerator = bad.into();
        assert_eq!(
            config.normalize().unwrap_err().code,
            E_INVALID_ARG,
            "accelerator「{bad}」应拒绝整份提交"
        );
    }
}

// 断言（29 号票）：组合键归一化纯函数表驱动——大小写等价、修饰符重排
// 等价、别名收编、重复修饰符去重、纯修饰符/空段/多主键拒绝。
#[test]
fn normalize_accelerator_table() {
    let cases: Vec<(&str, Option<&str>)> = vec![
        // 大小写与修饰符顺序等价
        ("Alt+Space", Some("alt+space")),
        ("alt+space", Some("alt+space")),
        ("SHIFT+CTRL+P", Some("ctrl+shift+p")),
        ("p+shift+ctrl", Some("ctrl+shift+p")), // 主键在前也归一
        // 别名收编
        ("Cmd+Space", Some("super+space")),
        ("Control+Alt+T", Some("ctrl+alt+t")),
        ("Option+P", Some("alt+p")),
        ("Meta+K", Some("super+k")),
        // 重复修饰符去重
        ("alt+alt+space", Some("alt+space")),
        // 带空白的段容忍（trim 后有效）
        ("alt + space", Some("alt+space")),
        // 功能键/数字保留小写原词
        ("F5", Some("f5")),
        ("Ctrl+Shift+F1", Some("ctrl+shift+f1")),
        // 非法形态：空串 / 纯修饰符 / 空段 / 多主键 / 主键含空白
        ("", None),
        ("   ", None),
        ("ctrl+shift", None),
        ("alt++space", None),
        ("ctrl+alt+del+esc", None),
        ("alt+sp ace", None),
    ];
    for (raw, expected) in cases {
        assert_eq!(
            normalize_accelerator(raw).as_deref(),
            expected,
            "组合键「{raw}」归一化结果不符"
        );
    }
}
