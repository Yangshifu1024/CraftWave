//! wire 重建单元测试（§6.2 M1 子集）：text/reasoning 原样、toolCall+toolResult
//! 配对拆分、prompt 切分、非法形态拒绝。

use rig::completion::Message;
use rig::message::{ToolResultContent, UserContent};

use crate::backend::wire::{WireError, rebuild_message_stream, split_prompt};
use crate::storage::{MessageEntry, MessageRole, WirePart};

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

#[test]
fn 末条user切分为prompt_其余为history() {
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

#[test]
fn 工具结果按call_id配对工具名与错误标记映射() {
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

#[test]
fn 失败工具结果is_error为真() {
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

#[test]
fn 缺配对工具调用的工具结果被拒绝() {
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

#[test]
fn 空序列与非user末条拒绝() {
    assert_eq!(
        split_prompt(&[]),
        Err(WireError("start_chat 消息序列为空".to_owned()))
    );
    let assistant_only = vec![assistant_with_tool_round("a1", "call_1", "read")];
    assert!(matches!(split_prompt(&assistant_only), Err(WireError(_))));
}

#[test]
fn 运行标记与计划快照跳过() {
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
