//! 内置供应商预设清单（M1 切片交付项）。
//!
//! 设计文档未钉死预设集内容（只定字段与解析语义），本清单为 M1 内置的
//! 常识性静态数据：slug 是前端稳定键；模型仅作"开箱建议值"，用户可增删改。
//! 自定义 OpenAI 兼容端点不走预设（设置页直接空表单，apiFormat=openai_chat
//! + baseUrl 覆写）。
//!
//! 单源口径（定稿）：本清单是预设的唯一权威——类型
//! derive(TS) 后由 cargo test 统一导出类型声明与清单常量进
//! `ui/src/ipc/generated/`（ts-rs 机制），前端 re-export 消费，
//! 不设 list_presets 命令、无前端副本。

use serde::Serialize;

use crate::providers::types::{ApiFormat, ProviderConfig, ProviderModel};
use crate::util::ids::new_uuid;

/// 预设建议模型（wire id + 建议窗口参数）。
///
/// wire id 指模型在 API 请求体 `model` 字段里的取值（如 "deepseek-chat"）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetModel {
    /// 模型的 wire id（API 请求中的标识）。
    pub model: &'static str,
    /// 建议的上下文窗口大小（模型一次对话能容纳的最大 token 数）。
    pub context_window: u32,
    /// 建议的单次回复 token 上限。
    pub max_tokens: u32,
    /// 是否支持图片输入（视觉能力）。
    pub vision: bool,
}

/// 内置供应商预设。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPreset {
    /// 前端稳定键（如 "deepseek"），前端路由/存储引用它而非展示名。
    pub slug: &'static str,
    /// 展示名。
    pub name: &'static str,
    /// 该供应商 API 的请求方言，决定请求体如何构造（详见 types.rs 的 ApiFormat）。
    pub api_format: ApiFormat,
    /// 官方 API 地址（用户可改；兼容端点场景建议走"自定义"表单）。
    pub base_url: &'static str,
    /// 开箱即用的建议模型清单（仅是建议值，用户可增删改）。
    pub models: &'static [PresetModel],
    /// 本地服务标记：true = 本机推理端点——免 API
    /// Key（resolve_model 允许无 Key，本地服务不校验凭据）、离线可用；
    /// 设置页标注"本地 / 离线可用"并隐藏 Key 输入。
    pub local: bool,
}

/// 内置预设清单（M1）。
pub fn builtin_presets() -> &'static [ProviderPreset] {
    &[
        ProviderPreset {
            slug: "openai",
            name: "OpenAI",
            api_format: ApiFormat::OpenaiChat,
            base_url: "https://api.openai.com/v1",
            models: &[
                PresetModel {
                    model: "gpt-4.1",
                    context_window: 1_047_576,
                    max_tokens: 32_768,
                    vision: true,
                },
                PresetModel {
                    model: "gpt-4o-mini",
                    context_window: 128_000,
                    max_tokens: 16_384,
                    vision: true,
                },
            ],
            local: false,
        },
        ProviderPreset {
            slug: "anthropic",
            name: "Anthropic",
            api_format: ApiFormat::AnthropicMessages,
            base_url: "https://api.anthropic.com",
            models: &[
                PresetModel {
                    model: "claude-sonnet-4-5",
                    context_window: 200_000,
                    max_tokens: 64_000,
                    vision: true,
                },
                PresetModel {
                    model: "claude-haiku-4-5",
                    context_window: 200_000,
                    max_tokens: 64_000,
                    vision: true,
                },
            ],
            local: false,
        },
        ProviderPreset {
            slug: "deepseek",
            name: "DeepSeek",
            api_format: ApiFormat::OpenaiChat,
            base_url: "https://api.deepseek.com",
            models: &[
                PresetModel {
                    model: "deepseek-chat",
                    context_window: 128_000,
                    max_tokens: 8_192,
                    vision: false,
                },
                PresetModel {
                    model: "deepseek-reasoner",
                    context_window: 128_000,
                    max_tokens: 8_192,
                    vision: false,
                },
            ],
            local: false,
        },
        ProviderPreset {
            slug: "moonshot",
            name: "Kimi (Moonshot)",
            api_format: ApiFormat::OpenaiChat,
            base_url: "https://api.moonshot.cn/v1",
            models: &[PresetModel {
                model: "kimi-k2-0905-preview",
                context_window: 256_000,
                max_tokens: 8_192,
                vision: false,
            }],
            local: false,
        },
        ProviderPreset {
            slug: "zhipu",
            name: "智谱 GLM",
            api_format: ApiFormat::OpenaiChat,
            base_url: "https://open.bigmodel.cn/api/paas/v4",
            models: &[
                PresetModel {
                    model: "glm-5.3",
                    context_window: 200_000,
                    max_tokens: 128_000,
                    vision: true,
                },
                PresetModel {
                    model: "glm-5.3-flash",
                    context_window: 200_000,
                    max_tokens: 32_768,
                    vision: true,
                },
                PresetModel {
                    model: "glm-4.6",
                    context_window: 200_000,
                    max_tokens: 128_000,
                    vision: true,
                },
            ],
            local: false,
        },
        ProviderPreset {
            slug: "openrouter",
            name: "OpenRouter",
            api_format: ApiFormat::OpenaiChat,
            base_url: "https://openrouter.ai/api/v1",
            models: &[PresetModel {
                model: "openrouter/auto",
                context_window: 128_000,
                max_tokens: 32_768,
                vision: false,
            }],
            local: false,
        },
        // ---- 本地模型三家：全部 openai_chat 方言 +
        // 免 Key（本地服务不校验凭据）+ 离线可用。取舍：走各家 OpenAI
        // 兼容端点而非 rig 原生 ollama 方言（零新方言代码、行为一致，
        // 同一注入面）；llama.cpp / LM Studio 的 wire id 由服务端实际加载的
        // 模型决定，预设值仅是"开箱建议"（server 端不校验或需按加载名改）。
        ProviderPreset {
            slug: "ollama",
            name: "Ollama（本地）",
            api_format: ApiFormat::OpenaiChat,
            base_url: "http://127.0.0.1:11434/v1",
            models: &[
                PresetModel {
                    model: "qwen3:8b",
                    context_window: 128_000,
                    max_tokens: 32_768,
                    vision: false,
                },
                PresetModel {
                    model: "llama3.1:8b",
                    context_window: 128_000,
                    max_tokens: 32_768,
                    vision: false,
                },
            ],
            local: true,
        },
        ProviderPreset {
            slug: "llamacpp",
            name: "llama.cpp server（本地）",
            api_format: ApiFormat::OpenaiChat,
            base_url: "http://127.0.0.1:8080/v1",
            // llama.cpp 的 /v1/chat/completions 不校验 model 值（以启动时
            // 加载的模型应答），占位名即可用；用户可改成实际模型名。
            models: &[PresetModel {
                model: "local-model",
                context_window: 32_000,
                max_tokens: 8_192,
                vision: false,
            }],
            local: true,
        },
        ProviderPreset {
            slug: "lmstudio",
            name: "LM Studio（本地）",
            api_format: ApiFormat::OpenaiChat,
            base_url: "http://127.0.0.1:1234/v1",
            // LM Studio 走 OpenAI 兼容端点（PRD 括号原文口径）；wire id 需
            // 与服务端实际加载的模型一致，占位名 + 引导文案，用户按需改。
            models: &[PresetModel {
                model: "local-model",
                context_window: 32_000,
                max_tokens: 8_192,
                vision: false,
            }],
            local: true,
        },
    ]
}

/// 按 slug 取预设。
pub fn find_preset(slug: &str) -> Option<&'static ProviderPreset> {
    builtin_presets().iter().find(|p| p.slug == slug)
}

/// 从预设实例化一个 ProviderConfig（新 uuid；apiKey 留空待设置页填写——
/// 本地预设免 Key，`local` 字段随预设带出，`resolve_model` 对其放行）。
pub fn preset_to_provider(preset: &ProviderPreset) -> ProviderConfig {
    ProviderConfig {
        id: new_uuid(),
        name: preset.name.to_string(),
        api_format: preset.api_format,
        base_url: Some(preset.base_url.to_string()),
        api_key: None,
        models: preset
            .models
            .iter()
            .map(|m| ProviderModel {
                id: new_uuid(),
                model: m.model.to_string(),
                max_tokens: m.max_tokens,
                context_window: m.context_window,
                reasoning_effort: None,
                vision: Some(m.vision),
            })
            .collect(),
        headers: None,
        local: preset.local,
    }
}
