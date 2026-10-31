//! Thinking 档位映射的表驱动测试（回写表锚定）。
//!
//! 五档 × 两方言 × 窗口参数全覆盖；每条断言都直接对照回写映射表，
//! 字段值漂移即红。全表合并为单个 #[test]，
//! 断言消息带阶段标识。

use super::{ReasoningDirective, ThinkingLevel, model_effort_to_request, thinking_to_request};
use crate::providers::types::{ApiFormat, ReasoningEffort};

const CHAT: ApiFormat = ApiFormat::OpenaiChat;
const RESPONSES: ApiFormat = ApiFormat::OpenaiResponses;
const ANTHROPIC: ApiFormat = ApiFormat::AnthropicMessages;

/// 标准窗口参数（缺省：128000 窗口 / 32768 maxTokens）。
const WINDOW: u64 = 128_000;
const MAX_TOKENS: u64 = 32_768;

// 断言： 回写表全语义——会话档五档 × OpenAI 系（off→"none" 显式关闭，
// 其余同名值）；会话档 × Anthropic 比例轴（off 不启用，minimal/low/medium/
// high = 20/40/60/80% 预算，超过 max_tokens 时上调 = min(窗口, 预算+max_tokens)
// 封顶窗口）；per-model 档 × OpenAI 系（default 不传、low/medium/high/max
// 同名值）；per-model 档 × Anthropic（default→20%、low→40%、high/max→80%
// 同预算且叠加上调）；小窗口比例按 f64 乘后截断取整；预算恰等于 max_tokens
// 不上调；零窗口防御只出预算不出上调（避免除零/溢出面）。
#[test]
fn thinking_budget_tables_all_dialects() {
    // 会话档五档 × OpenAI 系：off → "none"（显式关闭），其余同名值
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

    // 会话档 × Anthropic：off 不启用；20% 档预算（25600）不超过原 max_tokens
    // 无上调；low/medium 上调；high 封顶窗口
    assert_eq!(
        thinking_to_request(ThinkingLevel::Off, ANTHROPIC, WINDOW, MAX_TOKENS),
        ReasoningDirective::none(),
        "off 不启用 thinking"
    );
    assert_eq!(
        thinking_to_request(ThinkingLevel::Minimal, ANTHROPIC, WINDOW, MAX_TOKENS),
        ReasoningDirective {
            effort_word: None,
            budget_tokens: Some(25_600),
            max_tokens_override: None,
        },
        "minimal = 20%，不上调"
    );
    assert_eq!(
        thinking_to_request(ThinkingLevel::Low, ANTHROPIC, WINDOW, MAX_TOKENS),
        ReasoningDirective {
            effort_word: None,
            budget_tokens: Some(51_200),
            max_tokens_override: Some(83_968),
        },
        "low = 40%，上调 = min(128000, 51200+32768)"
    );
    assert_eq!(
        thinking_to_request(ThinkingLevel::Medium, ANTHROPIC, WINDOW, MAX_TOKENS),
        ReasoningDirective {
            effort_word: None,
            budget_tokens: Some(76_800),
            max_tokens_override: Some(109_568),
        },
        "medium = 60%"
    );
    assert_eq!(
        thinking_to_request(ThinkingLevel::High, ANTHROPIC, WINDOW, MAX_TOKENS),
        ReasoningDirective {
            effort_word: None,
            budget_tokens: Some(102_400),
            max_tokens_override: Some(128_000),
        },
        "high = 80%，上调封顶窗口"
    );

    // per-model 档 × OpenAI 系（原表）：default = 不传，其余同名值
    assert_eq!(
        model_effort_to_request(ReasoningEffort::Default, CHAT, WINDOW, MAX_TOKENS),
        ReasoningDirective::none(),
        "default 不传（供应商缺省）"
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

    // per-model 档 × Anthropic：default→20% 不上调；low→40%；high/max 同预算
    // 80% 且叠加上调（上调规则由预算 > max_tokens 统一判定， max 行「类推」口径）
    assert_eq!(
        model_effort_to_request(ReasoningEffort::Default, ANTHROPIC, WINDOW, MAX_TOKENS),
        ReasoningDirective {
            effort_word: None,
            budget_tokens: Some(25_600),
            max_tokens_override: None,
        },
        "default = 20%（与会话 minimal 同预算对应行）"
    );
    assert_eq!(
        model_effort_to_request(ReasoningEffort::Low, ANTHROPIC, WINDOW, MAX_TOKENS).budget_tokens,
        Some(51_200),
        "low = 40%"
    );
    let high = model_effort_to_request(ReasoningEffort::High, ANTHROPIC, WINDOW, MAX_TOKENS);
    let max = model_effort_to_request(ReasoningEffort::Max, ANTHROPIC, WINDOW, MAX_TOKENS);
    assert_eq!(high.budget_tokens, Some(102_400));
    assert_eq!(max.budget_tokens, Some(102_400), "high 与 max 同预算 80%");
    assert_eq!(high.max_tokens_override, Some(128_000));
    assert_eq!(max.max_tokens_override, Some(128_000), "max 叠加上调");

    // 小窗口参数下的比例取整：窗口不能整除时按 f64 乘后取整（截断）
    // 9999 × 0.20 = 1999.8 → 1999
    let d = thinking_to_request(ThinkingLevel::Minimal, ANTHROPIC, 9_999, 32_768);
    assert_eq!(d.budget_tokens, Some(1_999), "截断取整");

    // 上调规则边界：预算恰好等于 max_tokens 时不上调（163840 × 20% = 32768）
    let d = thinking_to_request(ThinkingLevel::Minimal, ANTHROPIC, 163_840, 32_768);
    assert_eq!(d.budget_tokens, Some(32_768));
    assert_eq!(d.max_tokens_override, None, "预算=上限不触发上调");

    // 零窗口防御（配置异常路径）：不产生上调（避免除零/溢出面）
    let d = thinking_to_request(ThinkingLevel::Low, ANTHROPIC, 0, 32_768);
    assert_eq!(d.budget_tokens, Some(0));
    assert_eq!(d.max_tokens_override, None, "零窗口只出预算");
}
