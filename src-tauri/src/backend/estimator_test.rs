//! token 估算器测试（每请求预算检查）。

use super::{
    estimate_request_tokens, estimate_text_tokens, is_over_wire_budget, should_auto_compact,
};
use crate::storage::{MessageEntry, MessageRole, WirePart};

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

// 断言：文本计费表——中文 1.5 token/字上取整（宁高勿低口径）、ASCII 每 4
// 字符 1 token 上取整、中英混排两组分别计费后求和；请求预算 = 前缀 + 历史
// + prompt + 工具表 + 回复预留，逐面累计。
#[test]
fn text_and_request_estimate_cases() {
    // (输入文本, 期望 token)
    for (text, want) in [
        ("四个汉字", 6),
        ("三个字", 5),   // 3 × 1.5 = 4.5 → 上取整
        ("abcdefgh", 2), // 8 / 4
        ("abcde", 2),    // 5 / 4 → 上取整
        ("", 0),
        ("你好abcd", 4), // 混排：3 + 1
    ] {
        assert_eq!(estimate_text_tokens(text), want, "text={text:?}");
    }

    // 请求预算逐面累计：工具表、历史、系统前缀各自计入。
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

// 断言：阈值判定表——自动压缩 0.6/1.0 上下界（≥ 触发）、无 usage 首轮小
// 请求不触发、零窗口防御；硬预算（0.9 窗口）严格大于判定、零窗口防御。
#[test]
fn threshold_and_budget_table() {
    // (tokens, window, ratio) → 是否触发自动压缩
    for (tokens, window, ratio, want) in [
        (600, 1000, 0.6, true),   // 预算 600：≥ 触发
        (599, 1000, 0.6, false),  // 差 1 不触发
        (50, 1000, 0.6, false),   // 无 usage 的首轮小请求远低于阈值
        (999_999, 0, 0.6, false), // 零窗口（未配置模型窗口）防御性不触发
        (1000, 1000, 1.0, true),  // 阈值 1.0：占满窗口才触发
        (999, 1000, 1.0, false),
    ] {
        assert_eq!(
            should_auto_compact(tokens, window, ratio),
            want,
            "tokens={tokens} window={window} ratio={ratio}"
        );
    }
    // (tokens, window) → 是否超硬预算（0.9 窗口，严格 > 判定）
    for (tokens, window, want) in [(900, 1000, false), (901, 1000, true), (901, 0, false)] {
        assert_eq!(
            is_over_wire_budget(tokens, window),
            want,
            "tokens={tokens} window={window}（window=0 为零窗口防御）"
        );
    }
}
