//! 【S1 spike 原型，随时可弃】mock_chat.rs 的独立测试文件（AGENTS.md：测试不进源码）。
#![cfg(test)]

use crate::mock_chat::{MockFrame, mock_script};

#[test]
fn script_frame_seq_strictly_increasing() {
    let steps = mock_script();
    for (i, step) in steps.iter().enumerate() {
        let seq = match &step.frame {
            MockFrame::TextDelta { seq, .. }
            | MockFrame::ToolStart { seq, .. }
            | MockFrame::ToolProgress { seq, .. }
            | MockFrame::ToolResult { seq, .. }
            | MockFrame::RunEnded { seq } => *seq,
        };
        assert_eq!(seq, (i + 1) as u32, "seq 应按 1 起步连续递增");
    }
}

/// 把某工具的 args_text_delta 分片按序拼接。
fn joined_args(id: &str) -> (Vec<String>, String) {
    let pieces: Vec<String> = mock_script()
        .iter()
        .filter_map(|s| match &s.frame {
            MockFrame::ToolProgress {
                tool_call_id,
                args_text_delta,
                ..
            } if tool_call_id == id => Some(args_text_delta.clone()),
            _ => None,
        })
        .collect();
    let joined = pieces.join("");
    (pieces, joined)
}

#[test]
fn partial_deltas_reassemble_full_json() {
    let (_, args1) = joined_args("call-1");
    let parsed: serde_json::Value =
        serde_json::from_str(&args1).expect("call-1 增量拼接后必须是完整 JSON");
    assert_eq!(parsed["path"], "src/main.rs");
    assert_eq!(parsed["startLine"], 1);

    let (_, args2) = joined_args("call-2");
    let parsed2: serde_json::Value =
        serde_json::from_str(&args2).expect("call-2 增量拼接后必须是完整 JSON");
    assert_eq!(parsed2["command"], "cargo test");
}

#[test]
fn call1_three_parts_partial_json_until_last() {
    let (pieces, joined) = joined_args("call-1");
    assert_eq!(pieces.len(), 3, "call-1 应拆成 3 片增量");
    let try_parse = |s: &str| serde_json::from_str::<serde_json::Value>(s).is_ok();
    assert!(
        !try_parse(&pieces[0]),
        "第 1 片应为半截 JSON（参数准备中展示）"
    );
    let two = format!("{}{}", pieces[0], pieces[1]);
    assert!(
        !try_parse(&two),
        "前两片拼接仍应为半截 JSON（参数准备中展示）"
    );
    assert!(try_parse(&joined), "三片拼齐后参数应完整（运行中判定）");
}

#[test]
fn error_result_frame_has_is_error() {
    let has_error = mock_script().iter().any(|s| {
        matches!(
            &s.frame,
            MockFrame::ToolResult {
                is_error: Some(true),
                ..
            }
        )
    });
    assert!(has_error, "剧本必须包含一个 isError 工具结果（错误态判定）");
}

/// 帧序列化形态：tag 为 kebab-case、字段为 camelCase（对齐前端帧类型与 D14）。
#[test]
fn frame_serialization_matches_frontend_contract() {
    let frame = MockFrame::ToolProgress {
        seq: 3,
        tool_call_id: "call-1".into(),
        args_text_delta: r#"{"path""#.into(),
    };
    let json = serde_json::to_value(&frame).expect("帧必须可序列化");
    assert_eq!(json["type"], "tool-progress");
    assert_eq!(json["toolCallId"], "call-1");
    assert_eq!(json["argsTextDelta"], r#"{"path""#);

    let err_frame = MockFrame::ToolResult {
        seq: 9,
        tool_call_id: "call-2".into(),
        result: serde_json::json!({ "exitCode": 101 }),
        is_error: Some(true),
    };
    let err_json = serde_json::to_value(&err_frame).expect("帧必须可序列化");
    assert_eq!(err_json["type"], "tool-result");
    assert_eq!(err_json["isError"], true);
}
