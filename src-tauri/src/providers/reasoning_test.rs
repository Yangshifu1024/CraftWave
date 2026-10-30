//! Thinking 档位映射的表驱动测试（18 号票，§11.4 回写表锚定）。
//!
//! 五档 × 两方言 × 窗口参数全覆盖；每条断言都直接对照技术设计文档 §11.4
//! 的回写表（本票定稿），字段值漂移即红。

use super::{ReasoningDirective, ThinkingLevel, model_effort_to_request, thinking_to_request};
use crate::providers::types::{ApiFormat, ReasoningEffort};

const CHAT: ApiFormat = ApiFormat::OpenaiChat;
const RESPONSES: ApiFormat = ApiFormat::OpenaiResponses;
const ANTHROPIC: ApiFormat = ApiFormat::AnthropicMessages;

/// 标准窗口参数（§3.2 缺省：128000 窗口 / 32768 maxTokens）。
const WINDOW: u64 = 128_000;
const MAX_TOKENS: u64 = 32_768;

/// 会话档五档 × OpenAI 系：off → "none"（显式关闭），其余同名值。
#[test]
fn session_levels_openai_words() {
    let expected = [
        (ThinkingLevel::Off, "none"),
        (ThinkingLevel::Minimal, "minimal"),
        (ThinkingLevel::Low, "low"),
        (ThinkingLevel::Medium, "medium"),
        (ThinkingLevel::High, "high"),
    ];
    for (level, word) in expected {
        for api in [CHAT, RESPONSES] {
            let d = thinking_to_request(level, api, WINDOW, MAX_TOKENS);
            assert_eq!(
                d,
                ReasoningDirective {
                    effort_word: Some(word),
                    budget_tokens: None,
                    max_tokens_override: None,
                },
                "{level:?} × {api:?} 应映射 effort 词 {word}"
            );
        }
    }
}

/// 会话档五档 × Anthropic：off 不启用；minimal/low/medium/high = 20/40/60/80%
/// 预算（§11.4 比例轴）。20% 档预算（25600）不超过原 max_tokens（32768），
/// 无上调。
#[test]
fn session_levels_anthropic_budgets() {
    // off：不启用 thinking
    assert_eq!(
        thinking_to_request(ThinkingLevel::Off, ANTHROPIC, WINDOW, MAX_TOKENS),
        ReasoningDirective::none()
    );
    // minimal = 20% × 128000 = 25600 ≤ 32768：不上调
    assert_eq!(
        thinking_to_request(ThinkingLevel::Minimal, ANTHROPIC, WINDOW, MAX_TOKENS),
        ReasoningDirective {
            effort_word: None,
            budget_tokens: Some(25_600),
            max_tokens_override: None,
        }
    );
    // low = 40% = 51200 > 32768：上调 = min(128000, 51200 + 32768) = 83968
    assert_eq!(
        thinking_to_request(ThinkingLevel::Low, ANTHROPIC, WINDOW, MAX_TOKENS),
        ReasoningDirective {
            effort_word: None,
            budget_tokens: Some(51_200),
            max_tokens_override: Some(83_968),
        }
    );
    // medium = 60% = 76800：上调 = min(128000, 76800 + 32768) = 109568
    assert_eq!(
        thinking_to_request(ThinkingLevel::Medium, ANTHROPIC, WINDOW, MAX_TOKENS),
        ReasoningDirective {
            effort_word: None,
            budget_tokens: Some(76_800),
            max_tokens_override: Some(109_568),
        }
    );
    // high = 80% = 102400：上调封顶窗口 = 128000
    assert_eq!(
        thinking_to_request(ThinkingLevel::High, ANTHROPIC, WINDOW, MAX_TOKENS),
        ReasoningDirective {
            effort_word: None,
            budget_tokens: Some(102_400),
            max_tokens_override: Some(128_000),
        }
    );
}

/// per-model 档 × OpenAI 系（§11.4 原表）：default = 不传（供应商缺省），
/// low/medium/high/max 同名值。
#[test]
fn model_effort_openai_words() {
    assert_eq!(
        model_effort_to_request(ReasoningEffort::Default, CHAT, WINDOW, MAX_TOKENS),
        ReasoningDirective::none()
    );
    let expected = [
        (ReasoningEffort::Low, "low"),
        (ReasoningEffort::Medium, "medium"),
        (ReasoningEffort::High, "high"),
        (ReasoningEffort::Max, "max"),
    ];
    for (effort, word) in expected {
        let d = model_effort_to_request(effort, RESPONSES, WINDOW, MAX_TOKENS);
        assert_eq!(
            d.effort_word,
            Some(word),
            "{effort:?} 应映射 effort 词 {word}"
        );
        assert_eq!(d.budget_tokens, None);
        assert_eq!(d.max_tokens_override, None);
    }
}

/// per-model 档 × Anthropic（§11.4 原表比例轴）：default→20%、low→40%、
/// medium→60%、high/max→80%（max 行叠加 max_tokens 上调）。
#[test]
fn model_effort_anthropic_budgets() {
    // default = 20%：不上调（与会话 minimal 同预算——§11.4 回写表的对应行）
    assert_eq!(
        model_effort_to_request(ReasoningEffort::Default, ANTHROPIC, WINDOW, MAX_TOKENS),
        ReasoningDirective {
            effort_word: None,
            budget_tokens: Some(25_600),
            max_tokens_override: None,
        }
    );
    // low = 40%：上调
    assert_eq!(
        model_effort_to_request(ReasoningEffort::Low, ANTHROPIC, WINDOW, MAX_TOKENS).budget_tokens,
        Some(51_200)
    );
    // high 与 max 同预算 80%；max 叠加上调（high 也触发上调规则——上调规则
    // 由预算 > max_tokens 统一判定，§11.4 max 行「类推」口径）
    let high = model_effort_to_request(ReasoningEffort::High, ANTHROPIC, WINDOW, MAX_TOKENS);
    let max = model_effort_to_request(ReasoningEffort::Max, ANTHROPIC, WINDOW, MAX_TOKENS);
    assert_eq!(high.budget_tokens, Some(102_400));
    assert_eq!(max.budget_tokens, Some(102_400));
    assert_eq!(high.max_tokens_override, Some(128_000));
    assert_eq!(max.max_tokens_override, Some(128_000));
}

/// 小窗口参数下的比例取整：窗口不能整除时按 f64 乘后取整（截断）。
#[test]
fn budget_rounds_down_on_odd_windows() {
    // 7000 × 0.20 = 1400 整除；换 9999：1999.8 → 1999（as u64 截断）
    let d = thinking_to_request(ThinkingLevel::Minimal, ANTHROPIC, 9_999, 32_768);
    assert_eq!(d.budget_tokens, Some(1_999));
}

/// 上调规则的边界：预算恰好等于 max_tokens 时不上调（不触发「预算超过」）。
#[test]
fn no_override_when_budget_equals_max_tokens() {
    // 窗口 163840 的 20% = 32768 = max_tokens：不上调
    let d = thinking_to_request(ThinkingLevel::Minimal, ANTHROPIC, 163_840, 32_768);
    assert_eq!(d.budget_tokens, Some(32_768));
    assert_eq!(d.max_tokens_override, None);
}

/// 零窗口防御（配置异常路径）：不产生上调（避免除零/溢出面）。
#[test]
fn zero_window_keeps_budget_only() {
    let d = thinking_to_request(ThinkingLevel::Low, ANTHROPIC, 0, 32_768);
    assert_eq!(d.budget_tokens, Some(0));
    assert_eq!(d.max_tokens_override, None);
}
