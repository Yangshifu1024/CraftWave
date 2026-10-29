//! token 估算器测试（§6.2 每请求预算检查，Q18）。

use super::{
    estimate_request_tokens, estimate_text_tokens, is_over_wire_budget, should_auto_compact,
};
use crate::storage::{MessageEntry, MessageRole, WirePart};

// 断言：中文按 1.5 token/字上取整——4 个汉字 = 6 token（宁高勿低口径）。
#[test]
fn cjk_chars_cost_one_and_half_tokens() {
    assert_eq!(estimate_text_tokens("四个汉字"), 6);
    // 3 个汉字 = 4.5 → 上取整 5
    assert_eq!(estimate_text_tokens("三个字"), 5);
}

// 断言：ASCII 每 4 字符 1 token 上取整。
#[test]
fn ascii_chars_cost_quarter_token_each() {
    assert_eq!(estimate_text_tokens("abcdefgh"), 2); // 8 / 4
    assert_eq!(estimate_text_tokens("abcde"), 2); // 5 / 4 → 上取整
    assert_eq!(estimate_text_tokens(""), 0);
}

// 断言：中英混排两组分别计费后求和。
#[test]
fn mixed_text_sums_both_groups() {
    // "你好" = 3，"abcd" = 1 → 4
    assert_eq!(estimate_text_tokens("你好abcd"), 4);
}

// 断言：阈值判定表驱动——0.6 上下、零窗口防御、首条消息小内容不触发。
#[test]
fn auto_compact_threshold_table() {
    // 窗口 1000，阈值 0.6 → 预算 600：600 触发（≥），599 不触发
    assert!(should_auto_compact(600, 1000, 0.6));
    assert!(!should_auto_compact(599, 1000, 0.6));
    // 无 usage 的首轮小请求（几十 token）远低于阈值
    assert!(!should_auto_compact(50, 1000, 0.6));
    // 零窗口（未配置模型窗口）防御性不触发
    assert!(!should_auto_compact(999_999, 0, 0.6));
    // 阈值 1.0：占满窗口才触发
    assert!(should_auto_compact(1000, 1000, 1.0));
    assert!(!should_auto_compact(999, 1000, 1.0));
}

// 断言：硬预算（0.9 窗口）判定。
#[test]
fn wire_budget_hard_limit() {
    // 窗口 1000 ×0.9 = 900：900 不超（>），901 超
    assert!(!is_over_wire_budget(900, 1000));
    assert!(is_over_wire_budget(901, 1000));
    assert!(!is_over_wire_budget(901, 0), "零窗口防御");
}

// 断言：请求预算 = 前缀 + 历史 + prompt + 工具表 + 回复预留，逐面累计。
#[test]
fn request_budget_sums_all_faces() {
    let history = vec![message_entry(
        "u1",
        "这是一段比较长的用户历史消息内容，占用显著的面",
        MessageRole::User,
    )];
    let tools = vec![("read".to_owned(), "读取文件".to_owned(), "{}".to_owned())];
    let base = estimate_request_tokens(&["系统前缀指令"], &history, "用户输入", &tools, 32768);
    assert!(base > 32768, "应超过回复预留部分：{base}");
    let no_tools = estimate_request_tokens(&["系统前缀指令"], &history, "用户输入", &[], 32768);
    assert!(base > no_tools, "工具表应计入预算");
    let no_history = estimate_request_tokens(&["系统前缀指令"], &[], "用户输入", &tools, 32768);
    assert!(no_tools > no_history, "历史应计入预算");
    let no_prefix = estimate_request_tokens(&[], &history, "用户输入", &tools, 32768);
    assert!(base > no_prefix, "系统前缀应计入预算");
}

/// 造一条单文本消息条目。
fn message_entry(id: &str, text: &str, role: MessageRole) -> MessageEntry {
    MessageEntry {
        id: id.to_owned(),
        parent_id: None,
        role,
        parts: vec![WirePart::Text {
            text: text.to_owned(),
        }],
        created_at: 1,
        run_id: None,
        status: None,
    }
}
