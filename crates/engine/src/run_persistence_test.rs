//! RunRecorder 帧聚合落盘测试（独立测试文件，源码零测试）。
//!
//! 帧序驱动 PersistingSink（与 RunLoop 出口同一 FrameSink 面），断言
//! 时机表的落盘产物：文本 turn 条目、工具 turn 条目、runMarker 收口与
//! index upsert。

use std::sync::Arc;

use crate::backend::frames::{ChatFrame, CollectingSink, EndReason, FrameSink, UsageTotal};
use crate::run_persistence::{PersistingSink, RunRecorder};
use crate::storage::{SessionStore, WirePart};

/// 测试脚手架：临时数据目录 + SessionStore + 包了收集桩的落盘装饰器
/// （返回目录句柄是为了借 TempDir 析构清理临时文件）。
fn recorder() -> (Arc<SessionStore>, PersistingSink, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::with_root(dir.path().to_path_buf()));
    let sink = PersistingSink::new(
        RunRecorder::new(store.clone(), "s1".to_string(), "run-1".to_string(), None),
        Box::new(CollectingSink::default()),
    );
    (store, sink, dir)
}

/// 灌入一次纯文本 run 的标准帧序（启动 → 一个 turn 的两段文本 →
/// usage 收口 → 正常结束），供多个用例复用。
fn send_text_run(sink: &mut PersistingSink) {
    sink.send(ChatFrame::RunStarted {
        run_id: "run-1".to_string(),
        model: "test-model".to_string(),
    });
    sink.send(ChatFrame::StepStarted { turn: 1 });
    sink.send(ChatFrame::TextDelta {
        turn: 1,
        delta: "你".to_string(),
    });
    sink.send(ChatFrame::TextDelta {
        turn: 1,
        delta: "好".to_string(),
    });
    sink.send(ChatFrame::Usage {
        usage: UsageTotal {
            input: 10,
            output: 5,
            ..UsageTotal::default()
        },
        rate: None,
    });
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Done,
        duration_ms: 120,
        tool_count: 0,
        turn: 1,
        error: None,
    });
}

/// 断言三种帧序的落盘产物形状与 run 终态的两个侧面：
/// 1) 纯文本 run：一条文本条目 + runMarker，index 建档（用量为四项之和、
///    条目数与修订号与史册一致）；
/// 2) 纯工具 run：ToolCall 与 ToolResult 相邻配对，args 规范化为紧凑 JSON，
///    marker 挂在工具条目之后（父链完整）；
/// 3) 文本 + 工具混合 turn：usage 先收口，文本条先落、工具条后落，
///    reasoning 段排在文本段之前，父链按落盘顺序串起来；
/// 4) index upsert 不得清空既有外部目录授权（语义：授权跨 run 生效）；
///    流中取消（没有 usage 收口帧）残留文本
///    桶由 runEnded 防御兜底收口，runMarker 仍照常落盘。
#[test]
fn text_tool_mixed_and_run_end_persistence() {
    // 1) 纯文本 run。
    let (store, mut sink, _dir) = recorder();
    send_text_run(&mut sink);
    let tree = store.load_session_tree("s1").unwrap();
    let ids: Vec<&str> = tree.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["run-1:1", "run-1:marker"]);
    let turn_entry = &tree.messages[0];
    assert_eq!(
        turn_entry.parts,
        vec![WirePart::Text {
            text: "你好".to_string()
        }]
    );
    assert!(turn_entry.run_id.as_deref() == Some("run-1"));
    // marker：RunEnded 帧字段收口
    assert!(matches!(
        &tree.messages[1].parts[0],
        WirePart::RunMarker { run_id, ended_at: Some(_), duration_ms: Some(120), tool_count: Some(0), .. }
            if run_id == "run-1"
    ));
    // activeLeaf = marker；index 建档（用量 = 四项和 15）
    assert_eq!(tree.active_leaf_id.as_deref(), Some("run-1:marker"));
    let meta = store
        .index()
        .list()
        .into_iter()
        .find(|m| m.id == "s1")
        .expect("index 应建档");
    assert_eq!(meta.total_tokens, 15);
    assert_eq!(meta.message_count, 2);
    assert_eq!(meta.revision, tree.revision);

    // 2) 纯工具 run。
    let (store, mut sink, _dir) = recorder();
    sink.send(ChatFrame::RunStarted {
        run_id: "run-1".to_string(),
        model: "test-model".to_string(),
    });
    sink.send(ChatFrame::StepStarted { turn: 1 });
    sink.send(ChatFrame::ToolStarted {
        call_id: "t1-c1".to_string(),
        name: "read".to_string(),
    });
    sink.send(ChatFrame::ToolArgs {
        call_id: "t1-c1".to_string(),
        chunk: r#"{"path": "README.md"}"#.to_string(),
    });
    sink.send(ChatFrame::ToolResult {
        call_id: "t1-c1".to_string(),
        ok: true,
        ui_summary: "读取 3 行".to_string(),
        error: None,
    });
    sink.send(ChatFrame::Usage {
        usage: UsageTotal::default(),
        rate: None,
    });
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Done,
        duration_ms: 200,
        tool_count: 1,
        turn: 1,
        error: None,
    });
    let tree = store.load_session_tree("s1").unwrap();
    let ids: Vec<&str> = tree.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["run-1:1.tools", "run-1:marker"]);
    // toolCall + toolResult 配对相邻；args 规范化为 compact JSON
    assert_eq!(
        tree.messages[0].parts,
        vec![
            WirePart::ToolCall {
                call_id: "t1-c1".to_string(),
                name: "read".to_string(),
                args_json: r#"{"path":"README.md"}"#.to_string(),
            },
            WirePart::ToolResult {
                call_id: "t1-c1".to_string(),
                ok: true,
                summary: "读取 3 行".to_string(),
            },
        ]
    );
    // marker 链尾 = 工具条（parent 链完整）
    assert_eq!(tree.messages[1].parent_id.as_deref(), Some("run-1:1.tools"));

    // 3) 文本 + 工具混合 turn：真实帧序下 Usage（流收口）先于 CallTools
    //    执行 → 文本条先落、工具条后落。
    let (store, mut sink, _dir) = recorder();
    sink.send(ChatFrame::RunStarted {
        run_id: "run-1".to_string(),
        model: "test-model".to_string(),
    });
    sink.send(ChatFrame::StepStarted { turn: 1 });
    sink.send(ChatFrame::ReasoningDelta {
        turn: 1,
        delta: "想一想".to_string(),
    });
    sink.send(ChatFrame::TextDelta {
        turn: 1,
        delta: "我先读文件".to_string(),
    });
    sink.send(ChatFrame::ToolStarted {
        call_id: "t1-c1".to_string(),
        name: "read".to_string(),
    });
    sink.send(ChatFrame::ToolArgs {
        call_id: "t1-c1".to_string(),
        chunk: "{}".to_string(),
    });
    sink.send(ChatFrame::Usage {
        usage: UsageTotal::default(),
        rate: None,
    });
    sink.send(ChatFrame::ToolResult {
        call_id: "t1-c1".to_string(),
        ok: true,
        ui_summary: "ok".to_string(),
        error: None,
    });
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Done,
        duration_ms: 300,
        tool_count: 1,
        turn: 1,
        error: None,
    });
    let tree = store.load_session_tree("s1").unwrap();
    let ids: Vec<&str> = tree.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["run-1:1", "run-1:1.tools", "run-1:marker"]);
    // 文本条 reasoning 在前（与前端 toParts 顺序一致）
    assert!(matches!(
        tree.messages[0].parts[0],
        WirePart::Reasoning { .. }
    ));
    // 链：文本 → 工具 → marker
    assert_eq!(tree.messages[1].parent_id.as_deref(), Some("run-1:1"));
    assert_eq!(tree.messages[2].parent_id.as_deref(), Some("run-1:1.tools"));

    // 4) run 终态：授权跨 run 保留。
    let (store, mut sink, _dir) = recorder();
    // 预置会话（run 开始前 index 已有授权记录，模拟授权后的会话）
    store
        .index()
        .upsert(crate::storage::SessionMeta {
            id: "s1".to_string(),
            project_id: None,
            title: "有授权的会话".to_string(),
            created_at: 1,
            updated_at: 1,
            revision: 1,
            message_count: 0,
            total_tokens: 0,
            authorized_external_dirs: vec!["/tmp/外部目录".to_string()],
        })
        .unwrap();
    send_text_run(&mut sink);
    let meta = store
        .index()
        .list()
        .into_iter()
        .find(|m| m.id == "s1")
        .unwrap();
    assert_eq!(
        meta.authorized_external_dirs,
        vec!["/tmp/外部目录".to_string()],
        "run 终态 upsert 清空了外部目录授权"
    );

    // 流中取消：marker 仍收口，历史不悬空。
    let (store, mut sink, _dir) = recorder();
    sink.send(ChatFrame::RunStarted {
        run_id: "run-1".to_string(),
        model: "test-model".to_string(),
    });
    sink.send(ChatFrame::StepStarted { turn: 1 });
    sink.send(ChatFrame::TextDelta {
        turn: 1,
        delta: "写一半".to_string(),
    });
    // 无 Usage（流中取消路径）→ RunEnded(cancelled)：marker 仍收口
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Cancelled,
        duration_ms: 50,
        tool_count: 0,
        turn: 1,
        error: None,
    });
    let tree = store.load_session_tree("s1").unwrap();
    let ids: Vec<&str> = tree.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["run-1:1", "run-1:marker"],
        "防御兜底应收口残留文本桶"
    );
}
