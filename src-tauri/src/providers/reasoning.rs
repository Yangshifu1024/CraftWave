//! Thinking 档位到模型请求参数的映射（reasoning 映射回写表）。
//!
//! 名词解释：「thinking 档位」指用户为某次对话选择的"模型思考力度"档位
//! （五档：off/minimal/low/medium/high）；「请求参数」指真正发往
//! 供应商 API 的推理控制字段——OpenAI 系（openai_chat / openai_responses
//! 两方言）是 `reasoning_effort`，Anthropic（anthropic_messages 方言）是
//! thinking budget（占上下文窗口的比例， 比例轴）。
//!
//! 本模块是纯函数层：不碰网络、不依赖 rig（providers 域保持纯配置层），
//! 输出中立的 [`ReasoningDirective`]，由 backend 层（`backend::turn`）翻译为
//! rig 的 `GenerationOptions`。两套档位体系在这里汇合（回写表定稿）：
//! - **会话档** [`ThinkingLevel`]：off/minimal/low/medium/high——
//!   用户按会话显式选择，优先级最高；
//! - **per-model 档** [`ReasoningEffort`]（ProviderModel.reasoningEffort）：
//!   default/low/medium/high/max——模型配置里的静态缺省，会话未显式设置时
//!   回落到它（含 default = 不传档）。
//!
//! 两者的对应关系：
//! 会话 minimal ↔ per-model default（同 20% 预算）；low/medium/high 同名；
//! per-model max = 80% + max_tokens 上调；会话 off = 显式关闭（per-model
//! 无对应档）。

use serde::{Deserialize, Serialize};

use super::types::{ApiFormat, ReasoningEffort};

/// 会话 thinking 档位（五档， / / RunConfig.thinking）。
/// 序列化为 snake_case（与 Rust 侧 ReasoningEffort 同风格， 口径）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    /// 显式关闭思考（OpenAI 系发 `reasoning_effort: "none"`；Anthropic 不启用
    /// thinking）。与「未设置」不同：off 是用户的明确选择，优先级高于
    /// per-model 配置。
    Off,
    /// 最轻思考（Anthropic 20% 预算， 比例轴）。
    Minimal,
    /// 低思考（40%）。
    Low,
    /// 中思考（60%）。
    Medium,
    /// 高思考（80%）。
    High,
}

impl ThinkingLevel {
    /// 全部档位（Composer 快捷钮循环顺序：off → minimal → low → medium →
    /// high）。
    pub const ALL: [ThinkingLevel; 5] = [
        ThinkingLevel::Off,
        ThinkingLevel::Minimal,
        ThinkingLevel::Low,
        ThinkingLevel::Medium,
        ThinkingLevel::High,
    ];

    /// OpenAI 系 `reasoning_effort` 的 wire 词（off → "none"，其余
    /// 同名；rig 的 `Effort::as_str` 同值——backend 层据此构造 rig 枚举）。
    pub fn effort_word(self) -> &'static str {
        match self {
            ThinkingLevel::Off => "none",
            ThinkingLevel::Minimal => "minimal",
            ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
        }
    }

    /// Anthropic thinking 预算占上下文窗口的比例（比例轴：20/40/60/80%）；
    /// off 不启用 thinking，返回 None。
    pub fn budget_ratio(self) -> Option<f64> {
        match self {
            ThinkingLevel::Off => None,
            ThinkingLevel::Minimal => Some(0.20),
            ThinkingLevel::Low => Some(0.40),
            ThinkingLevel::Medium => Some(0.60),
            ThinkingLevel::High => Some(0.80),
        }
    }
}

/// 档位映射的中立结果（回写表的一行）：按方言二选一生效。
///
/// 为什么是中立结构而不是直接给 rig 枚举：providers 域保持零 rig 依赖
/// （层次纪律），rig `GenerationOptions` 的构造在 backend 层完成。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningDirective {
    /// OpenAI 系请求字段 `reasoning_effort` 的取值。None = 不传（供应商缺省
    /// 行为）；Some("none") = 显式关闭思考；其余为同名 effort 词。
    pub effort_word: Option<&'static str>,
    /// Anthropic thinking 预算（token 数 = contextWindow × 比例）。None =
    /// 不启用 thinking。实际注入由三协议接入消费（映射表见上文）。
    pub budget_tokens: Option<u64>,
    /// Anthropic `max_tokens` 上调值（上调规则：预算生效且预算超过
    /// 原 max_tokens 时上调，保证 thinking 预算之外仍保留原输出空间且不超
    /// 窗口）。None = 不调整。
    pub max_tokens_override: Option<u64>,
}

impl ReasoningDirective {
    /// 「不传」行：OpenAI 系不发 reasoning_effort、Anthropic 不启用 thinking。
    pub fn none() -> ReasoningDirective {
        ReasoningDirective {
            effort_word: None,
            budget_tokens: None,
            max_tokens_override: None,
        }
    }
}

/// Anthropic 口径的预算与 max_tokens 上调（回写表的共享实现）：
/// budget = contextWindow × ratio；预算超过原 max_tokens 时上调为
/// min(contextWindow, budget + 原 max_tokens)——thinking 预算之外保留原
/// 输出空间且不超窗口（Anthropic API 要求 max_tokens > budget_tokens）。
fn anthropic_directive(
    ratio: f64,
    context_window: u64,
    max_tokens: u64,
) -> (Option<u64>, Option<u64>) {
    let budget = (context_window as f64 * ratio) as u64;
    if budget > max_tokens && context_window > 0 {
        // 上调：预算 + 原输出空间，封顶窗口（80% 档时恰好 = 窗口）
        (Some(budget), Some(context_window.min(budget + max_tokens)))
    } else {
        (Some(budget), None)
    }
}

/// 会话档映射（优先级链第一级：会话 thinking 显式设置， 回写表）。
///
/// - OpenAI 系：off → `"none"`（显式关闭）；minimal/low/medium/high → 同名值；
/// - Anthropic：off → 不启用；minimal/low/medium/high → 20/40/60/80% 预算
///   （budget 超过原 max_tokens 时按上调规则带 max_tokens 上调）。
pub fn thinking_to_request(
    level: ThinkingLevel,
    api_format: ApiFormat,
    context_window: u64,
    max_tokens: u64,
) -> ReasoningDirective {
    match api_format {
        ApiFormat::OpenaiChat | ApiFormat::OpenaiResponses => ReasoningDirective {
            effort_word: Some(level.effort_word()),
            budget_tokens: None,
            max_tokens_override: None,
        },
        ApiFormat::AnthropicMessages => match level.budget_ratio() {
            Some(ratio) => {
                let (budget_tokens, max_tokens_override) =
                    anthropic_directive(ratio, context_window, max_tokens);
                ReasoningDirective {
                    effort_word: None,
                    budget_tokens,
                    max_tokens_override,
                }
            }
            // off：Anthropic 无显式关闭字段，不启用 thinking 即缺省行为
            None => ReasoningDirective::none(),
        },
    }
}

/// per-model 档映射（优先级链第二级：会话未显式设置 thinking 时回落到
/// `ProviderModel.reasoningEffort`， 原表的 default/low/medium/high/max 行）。
///
/// - OpenAI 系：default → 不传（"default" 不是 OpenAI API 的合法 effort 值，
///   语义即供应商缺省）；low/medium/high/max → 同名值（max 由 rig 目录或
///   供应商自行裁决，`OnUnsupported::Ignore` 保证不支持时忽略不报错）；
/// - Anthropic：default → 20%、low → 40%、medium → 60%、high → 80%、
///   max → 80% + max_tokens 上调（max 行）。
pub fn model_effort_to_request(
    effort: ReasoningEffort,
    api_format: ApiFormat,
    context_window: u64,
    max_tokens: u64,
) -> ReasoningDirective {
    match api_format {
        ApiFormat::OpenaiChat | ApiFormat::OpenaiResponses => {
            let word = match effort {
                // default 档 = 不显式传（「reasoningEffort 可空 = 不传给
                // 供应商」的 default 语义延伸）
                ReasoningEffort::Default => return ReasoningDirective::none(),
                ReasoningEffort::Low => "low",
                ReasoningEffort::Medium => "medium",
                ReasoningEffort::High => "high",
                ReasoningEffort::Max => "max",
            };
            ReasoningDirective {
                effort_word: Some(word),
                budget_tokens: None,
                max_tokens_override: None,
            }
        }
        ApiFormat::AnthropicMessages => {
            let ratio = match effort {
                ReasoningEffort::Default => 0.20,
                ReasoningEffort::Low => 0.40,
                ReasoningEffort::Medium => 0.60,
                ReasoningEffort::High | ReasoningEffort::Max => 0.80,
            };
            let (budget_tokens, max_tokens_override) =
                anthropic_directive(ratio, context_window, max_tokens);
            ReasoningDirective {
                effort_word: None,
                budget_tokens,
                max_tokens_override,
            }
        }
    }
}

#[cfg(test)]
#[path = "reasoning_test.rs"]
mod reasoning_test;
