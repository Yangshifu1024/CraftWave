//! 内置供应商预设清单（M1 切片交付项）。
//!
//! 设计文档未钉死预设集内容（§11 只定字段与解析语义），本清单为 M1 内置的
//! 常识性静态数据：slug 是前端稳定键；模型仅作"开箱建议值"，用户可增删改。
//! 自定义 OpenAI 兼容端点不走预设（设置页直接空表单，apiFormat=openai_chat
//! + baseUrl 覆写）。

use crate::providers::types::{ApiFormat, ProviderConfig, ProviderModel};
use crate::util::ids::new_uuid;

/// 预设建议模型（wire id + 建议窗口参数）。
///
/// wire id 指模型在 API 请求体 `model` 字段里的取值（如 "deepseek-chat"）。
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
        },
        ProviderPreset {
            slug: "zhipu",
            name: "智谱 GLM",
            api_format: ApiFormat::OpenaiChat,
            base_url: "https://open.bigmodel.cn/api/paas/v4",
            models: &[PresetModel {
                model: "glm-4.6",
                context_window: 200_000,
                max_tokens: 128_000,
                vision: true,
            }],
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
        },
    ]
}

/// 按 slug 取预设。
pub fn find_preset(slug: &str) -> Option<&'static ProviderPreset> {
    builtin_presets().iter().find(|p| p.slug == slug)
}

/// 从预设实例化一个 ProviderConfig（新 uuid；apiKey 留空待设置页填写）。
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
    }
}
