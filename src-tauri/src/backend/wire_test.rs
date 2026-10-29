//! wire 重建的单元测试：文本/推理原样映射、toolCall 与 toolResult 按调用
//! id 配对拆分、prompt 与 history 切分、系统级前缀注入（customPrompt 有/无
//! 两态，03 号票）、非法形态（空序列/末条非 user/悬空 toolResult）拒绝。

use rig::completion::Message;
use rig::message::{ToolResultContent, UserContent};

use crate::backend::wire::{WireError, rebuild_message_stream, split_prompt, with_system_prefix};
use crate::storage::{MessageEntry, MessageRole, WirePart};

/// 造一条 user 文本消息。
fn user(id: &str, text: &str) -> MessageEntry {
    MessageEntry {
        id: id.to_owned(),
        parent_id: None,
        role: MessageRole::User,
        parts: vec![WirePart::Text {
            text: text.to_owned(),
        }],
        created_at: 1,
        run_id: None,
        status: None,
    }
}

/// 造一条含完整工具回合的 assistant 消息（推理 + 文本 + toolCall +
/// toolResult + 收尾文本）。
fn assistant_with_tool_round(id: &str, call_id: &str, tool_name: &str) -> MessageEntry {
    MessageEntry {
        id: id.to_owned(),
        parent_id: None,
        role: MessageRole::Assistant,
        parts: vec![
            WirePart::Reasoning {
                text: "先读文件".to_owned(),
            },
            WirePart::Text {
                text: "我来读一下。".to_owned(),
            },
            WirePart::ToolCall {
                call_id: call_id.to_owned(),
                name: tool_name.to_owned(),
                args_json: r#"{"path":"a.txt"}"#.to_owned(),
            },
            WirePart::ToolResult {
                call_id: call_id.to_owned(),
                ok: true,
                summary: "文件共 3 行".to_owned(),
            },
            WirePart::Text {
                text: "读完了。".to_owned(),
            },
        ],
        created_at: 2,
        run_id: Some("run-1".to_owned()),
        status: None,
    }
}

/// 提取 user 消息的全部文本（断言辅助）。
fn user_text(msg: &Message) -> String {
    let Message::User { content } = msg else {
        panic!("应为 user 消息");
    };
    content
        .iter()
        .filter_map(|c| match c {
            UserContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Arc<Message> 解引用辅助（测试断言用）。
fn as_message(arc: &std::sync::Arc<Message>) -> &Message {
    arc.as_ref()
}

/// 断言末条 user 消息作为 prompt，其余重建为 history（工具结果拆成一条
/// 独立的 user 消息）。
#[test]
fn last_user_as_prompt_rest_history() {
    let entries = vec![
        user("u1", "第一条"),
        assistant_with_tool_round("a1", "call_1", "read"),
        user("u2", "再来一条"),
    ];
    let (history, prompt) = split_prompt(&entries).unwrap();
    assert_eq!(user_text(&prompt), "再来一条");
    // history：u1 + (a1 的 assistant) + (a1 拆出的工具结果 user) = 3 条
    assert_eq!(history.len(), 3);
    assert_eq!(user_text(as_message(&history[0])), "第一条");
    assert!(matches!(as_message(&history[1]), Message::Assistant(_)));
    let Message::User { content } = as_message(&history[2]) else {
        panic!("工具结果应为 user 消息");
    };
    assert_eq!(content.len(), 1, "toolResult 合并为一条 user 消息");
}

/// 断言工具结果按调用 id 配对出工具名，ok 标志映射为 is_error 取反，
/// 摘要作为文本内容。
#[test]
fn tool_results_pair_by_call_id_and_error_mapping() {
    let entry = assistant_with_tool_round("a1", "call_9", "grep");
    let stream = rebuild_message_stream(&[entry]).unwrap();
    assert_eq!(stream.len(), 2, "assistant + 工具结果 user");
    let Message::User { content } = as_message(stream.last().unwrap()) else {
        panic!("应有工具结果消息");
    };
    let UserContent::ToolResult(result) = &content[0] else {
        panic!("应为 ToolResult content");
    };
    assert_eq!(result.call.to_string(), "call_9");
    assert_eq!(result.name.to_string(), "grep");
    assert!(!result.is_error, "ok=true → is_error=false");
    match &result.content[0] {
        ToolResultContent::Text(t) => assert_eq!(t.text, "文件共 3 行"),
        other => panic!("应为文本工具结果：{other:?}"),
    }
}

/// 断言失败的工具结果（ok=false）映射为 is_error=true。
#[test]
fn failed_tool_result_is_error_true() {
    let mut entry = assistant_with_tool_round("a1", "call_1", "read");
    entry.parts[3] = WirePart::ToolResult {
        call_id: "call_1".to_owned(),
        ok: false,
        summary: "E_INVALID_ARG: 参数错".to_owned(),
    };
    let stream = rebuild_message_stream(&[entry]).unwrap();
    let Message::User { content } = as_message(stream.last().unwrap()) else {
        panic!("应有工具结果消息");
    };
    let UserContent::ToolResult(result) = &content[0] else {
        panic!("应为 ToolResult content");
    };
    assert!(result.is_error);
}

/// 断言悬空的 toolResult（同条目内没有配对的 toolCall）被拒绝。
#[test]
fn tool_result_without_paired_call_rejected() {
    let entry = MessageEntry {
        id: "a1".to_owned(),
        parent_id: None,
        role: MessageRole::Assistant,
        parts: vec![WirePart::ToolResult {
            call_id: "call_x".to_owned(),
            ok: true,
            summary: "悬空结果".to_owned(),
        }],
        created_at: 1,
        run_id: None,
        status: None,
    };
    let entries = vec![user("u1", "hi"), entry, user("u2", "再问")];
    assert_eq!(
        split_prompt(&entries),
        Err(WireError(
            "消息 a1 的 toolResult call_x 缺配对的 toolCall".to_owned()
        ))
    );
}

/// 断言空序列与末条不是 user 的序列都被拒绝。
#[test]
fn empty_sequence_or_non_user_tail_rejected() {
    assert_eq!(
        split_prompt(&[]),
        Err(WireError("start_chat 消息序列为空".to_owned()))
    );
    let assistant_only = vec![assistant_with_tool_round("a1", "call_1", "read")];
    assert!(matches!(split_prompt(&assistant_only), Err(WireError(_))));
}

/// 断言 runMarker 与 planSnapshot 两类 part 被跳过（不产出消息、不影响
/// 其余内容）。
#[test]
fn run_marker_and_plan_snapshot_skipped() {
    let entry = MessageEntry {
        id: "a1".to_owned(),
        parent_id: None,
        role: MessageRole::Assistant,
        parts: vec![
            WirePart::RunMarker {
                run_id: "run-1".to_owned(),
                started_at: 1,
                ended_at: Some(2),
                duration_ms: Some(1),
                tool_count: Some(0),
            },
            WirePart::Text {
                text: "正文".to_owned(),
            },
        ],
        created_at: 1,
        run_id: None,
        status: None,
    };
    let stream = rebuild_message_stream(&[entry]).unwrap();
    assert_eq!(stream.len(), 1, "runMarker 不产消息");
    let Message::Assistant(m) = as_message(&stream[0]) else {
        panic!("应为 assistant");
    };
    assert_eq!(m.content.len(), 1, "只余 text part");
}

/// 提取 system 消息文本（断言辅助；非 system 即 panic）。
fn system_text(msg: &Message) -> String {
    let Message::System { content } = msg else {
        panic!("应为 system 消息");
    };
    content.clone()
}

/// 有 customPrompt：注入为 history 首条 system 消息（§6.2 次序首位），其余
/// 消息原序后移——模拟 start_chat 的 wire 出口调用形态。
#[test]
fn custom_prompt_injected_as_first_system_message() {
    let entries = vec![user("u1", "第一条"), user("u2", "第二条")];
    let (history, prompt) = split_prompt(&entries).unwrap();
    assert_eq!(history.len(), 1, "前置：无注入时 history 只有 u1");

    let injected = with_system_prefix(history, Some("始终用中文回复"));
    assert_eq!(injected.len(), 2, "注入后首条 system + 原 history");
    assert_eq!(system_text(as_message(&injected[0])), "始终用中文回复");
    assert_eq!(user_text(as_message(&injected[1])), "第一条");
    // prompt 不受注入影响
    assert_eq!(user_text(&prompt), "第二条");
}

/// 无/空白 customPrompt：零注入（history 原样返回，不产生空 system 消息）。
#[test]
fn empty_or_missing_custom_prompt_injects_nothing() {
    let entries = vec![user("u1", "第一条"), user("u2", "第二条")];
    let (history, _) = split_prompt(&entries).unwrap();
    let baseline_len = history.len();

    for prefix in [None, Some(""), Some("   \n\t ")] {
        let kept = with_system_prefix(history.clone(), prefix);
        assert_eq!(kept.len(), baseline_len, "前缀 {prefix:?} 应零注入");
        assert!(
            !matches!(as_message(&kept[0]), Message::System { .. }),
            "前缀 {prefix:?} 首条不应是 system"
        );
    }
}

/// 注入的空白归一：带首尾空白的前缀按 trim 后注入（与 AppConfig.normalize
/// 的归一口径双保险，防绕过配置入口的调用方带入空白前缀）。
#[test]
fn custom_prompt_is_trimmed_before_injection() {
    let injected = with_system_prefix(Vec::new(), Some("  保持简洁  "));
    assert_eq!(system_text(as_message(&injected[0])), "保持简洁");
}
