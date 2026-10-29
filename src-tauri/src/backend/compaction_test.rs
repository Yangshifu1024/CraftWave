//! 压缩编排纯逻辑测试（§6.2/Q31/Q38，04 号票）：
//! 压缩范围计算、条目文本化、分块装箱、id 规则。摘要调用编排（共用总超时、
//! 全部用量计量、失败不推进边界）由 ScriptedBackend e2e 承载（tests/backend_e2e.rs）。

use super::{compaction_entry_id, compression_scope, entry_to_block, plan_blocks};
use crate::storage::{MessageEntry, MessageRole, WirePart};

/// 造一条单文本消息。
fn text_entry(id: &str, role: MessageRole, text: &str) -> MessageEntry {
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

// 断言：首次压缩（无旧摘要）范围 = 调用方传入的候选集全量（run 开始前
// 场景调用方已剔除末条 prompt）；cut = 候选集链尾。
#[test]
fn first_compression_scope_covers_history_excluding_prompt() {
    // run 开始前场景：调用方剔除末条 prompt 后传 [u1, a1]
    let body = vec![
        text_entry("u1", MessageRole::User, "你好"),
        text_entry("a1", MessageRole::Assistant, "你好呀"),
    ];
    let scope = compression_scope(&body, None).expect("应有范围");
    assert_eq!(scope.cut_entry_id, "a1");
    assert_eq!(
        scope
            .entries
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        vec!["u1", "a1"]
    );
    assert!(scope.old_summary.is_none(), "首次压缩无旧摘要");
}

// 断言：增量压缩（Q31）范围 = 旧边界之后的新增内容；旧摘要随行。
#[test]
fn incremental_scope_starts_after_previous_cut() {
    let body = vec![
        text_entry("old", MessageRole::User, "已压缩区"),
        text_entry("cut1", MessageRole::Assistant, "上次边界"),
        text_entry("mid", MessageRole::User, "新增一"),
        text_entry("mid2", MessageRole::Assistant, "新增二"),
    ];
    let scope = compression_scope(&body, Some(("cut1", "上次摘要内容"))).expect("应有范围");
    assert_eq!(scope.cut_entry_id, "mid2");
    assert_eq!(
        scope
            .entries
            .iter()
            .map(|e| e.id.as_str())
            .collect::<Vec<_>>(),
        vec!["mid", "mid2"]
    );
    assert_eq!(scope.old_summary.as_deref(), Some("上次摘要内容"));
}

// 断言：候选集为空 → None；边界锚点不在候选集上 → 从头保守压缩。
#[test]
fn scope_edge_cases() {
    // 空候选集（如首次请求只有 prompt、ask 后无史册）
    assert!(compression_scope(&[], None).is_none());
    // 锚点缺失：从头压缩且保留旧摘要（增量语义不破坏）
    let body = vec![text_entry("x", MessageRole::User, "甲")];
    let scope = compression_scope(&body, Some(("missing", "旧摘要"))).expect("应有范围");
    assert_eq!(scope.cut_entry_id, "x");
    assert_eq!(scope.old_summary.as_deref(), Some("旧摘要"));
}

// 断言：条目文本化（Q38）——工具调用与结果带工具名/callId/来源标记转普通文本。
#[test]
fn entry_block_marks_tools_with_sources() {
    let entry = MessageEntry {
        id: "r1:1.tools".to_owned(),
        parent_id: None,
        role: MessageRole::Assistant,
        parts: vec![
            WirePart::ToolCall {
                call_id: "call_1".to_owned(),
                name: "read".to_owned(),
                args_json: r#"{"path":"a.rs"}"#.to_owned(),
            },
            WirePart::ToolResult {
                call_id: "call_1".to_owned(),
                ok: false,
                summary: "文件不存在".to_owned(),
            },
        ],
        created_at: 1,
        run_id: Some("r1".to_owned()),
        status: None,
    };
    let block = entry_to_block(&entry);
    assert!(
        block.contains("[assistant r1:1.tools]"),
        "角色与条目 id 标记：{block}"
    );
    assert!(
        block.contains("[tool name=read callId=call_1]"),
        "工具名与 callId：{block}"
    );
    assert!(
        block.contains("[result err callId=call_1]"),
        "结果来源与成败标记：{block}"
    );
    assert!(block.contains("文件不存在"), "结果内容保留");
    // runMarker / planSnapshot 不进摘要输入（与 wire 重建同口径）
    let marker = MessageEntry {
        id: "r1:marker".to_owned(),
        parent_id: None,
        role: MessageRole::System,
        parts: vec![WirePart::RunMarker {
            run_id: "r1".to_owned(),
            started_at: 1,
            ended_at: Some(2),
            duration_ms: None,
            tool_count: None,
        }],
        created_at: 2,
        run_id: None,
        status: None,
    };
    assert!(
        !entry_to_block(&marker).contains("runMarker"),
        "marker 不进摘要输入"
    );
}

// 断言：分块装箱——多块超窗拆批、单块自身超窗按字符切段、小内容单批。
#[test]
fn plan_blocks_buckets_and_splits_oversize() {
    // 窗口 14 token：中文字每字 1.5 → "一二三四五"=8、"六七八九十"=8（8+8>14 分批）、
    // "甲乙丙"=5（8+5=13≤14 与第二块同批）
    let blocks = vec![
        "一二三四五".to_owned(),
        "六七八九十".to_owned(),
        "甲乙丙".to_owned(),
    ];
    let batches = plan_blocks(blocks, 14);
    assert_eq!(batches.len(), 2, "实际：{batches:?}");
    assert!(batches[0].contains("一二三四五") && !batches[0].contains("六七八九十"));
    assert!(batches[1].contains("六七八九十") && batches[1].contains("甲乙丙"));

    // 单块超窗：按字符切段，至少两段且拼回原文等长
    let huge = "字".repeat(100); // 150 token > 窗口 10
    let batches = plan_blocks(vec![huge.clone()], 10);
    assert!(batches.len() >= 2, "超窗单块应切段");
    let rejoined: String = batches.concat();
    assert_eq!(rejoined, huge, "切段不丢字");

    // 空输入 → 空批
    assert!(plan_blocks(Vec::new(), 10).is_empty());
}

// 断言：compaction 条目 id 规则（§6.1 {owner}:compact:{n}）。
#[test]
fn compaction_id_uses_owner_and_sequence() {
    assert_eq!(compaction_entry_id("run-abc", 1), "run-abc:compact:1");
    assert_eq!(compaction_entry_id("manual", 3), "manual:compact:3");
}
