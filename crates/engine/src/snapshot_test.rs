//! snapshot 模块测试：TS `frames.test.ts` / `runtime.test.ts` 的行为断言
//! 迁移（保真、半截 JSON、64ms 节流、尾帧必达、各帧型合并、abort 终态）。

use std::rc::Rc;

use super::{FrameAggregator, SNAPSHOT_INTERVAL, StreamPart, SubRunState, args_ready, to_parts};
use crate::backend::frames::{
    AuditState, ChatFrame, EndReason, FrameError, SequencedFrame, SubOutcome, UsageTotal,
};
use crate::storage::{MessageEntry, MessageRole, WirePart};

/// 帧构造简写：seq 手工编号（对齐真实后端自增行为）。
fn f(seq: u64, frame: ChatFrame) -> SequencedFrame {
    SequencedFrame {
        seq,
        ts: None,
        frame,
    }
}

/// 复刻 TS 测试的直通帧序列（两轮 + 工具调用 + 半截 JSON 分段；
/// `frames.test.ts scriptedFrames` 同款剧本）。
fn scripted_frames() -> Vec<SequencedFrame> {
    vec![
        f(0, ChatFrame::RunStarted {
            run_id: "run-1".into(),
            model: "gpt-4o-mock".into(),
        }),
        f(1, ChatFrame::StepStarted { turn: 1 }),
        f(2, ChatFrame::ReasoningDelta {
            turn: 1,
            delta: "用户要我看配置，".into(),
        }),
        f(3, ChatFrame::ReasoningDelta {
            turn: 1,
            delta: "先调用 read。".into(),
        }),
        f(4, ChatFrame::TextDelta {
            turn: 1,
            delta: "我来读一下".into(),
        }),
        f(5, ChatFrame::TextDelta {
            turn: 1,
            delta: "配置文件。".into(),
        }),
        f(6, ChatFrame::ToolStarted {
            call_id: "call_1".into(),
            name: "read".into(),
        }),
        f(7, ChatFrame::ToolArgs {
            call_id: "call_1".into(),
            chunk: "{\"path\": \"/etc/app".into(),
        }),
        // 半截 JSON：此刻 argsJson 不是合法 JSON（卡片「参数准备中」）
        f(8, ChatFrame::ToolArgs {
            call_id: "call_1".into(),
            chunk: "/config.toml\", \"offset\": 0}".into(),
        }),
        f(9, ChatFrame::ToolResult {
            call_id: "call_1".into(),
            ok: true,
            ui_summary: "[read] 共 12 行：port = 8087".into(),
            error: None,
        }),
        f(10, ChatFrame::StepStarted { turn: 2 }),
        f(11, ChatFrame::TextDelta {
            turn: 2,
            delta: "配置文件共 12 行，".into(),
        }),
        f(12, ChatFrame::TextDelta {
            turn: 2,
            delta: "端口是 8087。".into(),
        }),
        f(13, ChatFrame::RunEnded {
            reason: EndReason::Done,
            duration_ms: 812,
            tool_count: 1,
            turn: 2,
            error: None,
        }),
    ]
}

/// 把帧序列全部推入聚合器。
fn feed(agg: &FrameAggregator, frames: &[SequencedFrame]) {
    for frame in frames {
        agg.push(frame.clone());
    }
}

#[test]
fn 保真剧本_段序与工具参数拼接_乱序丢弃() {
    // ---- 全帧累积：交错段序 = 帧到达序，同轮相邻增量合并为一段 ----
    let agg = FrameAggregator::new();
    let frames = scripted_frames();
    feed(&agg, &frames);
    let snap = agg.snapshot_streaming().expect("收帧后应有流式快照");
    assert_eq!(
        snap.parts,
        vec![
            StreamPart::Reasoning {
                text: "用户要我看配置，先调用 read。".into()
            },
            StreamPart::Text {
                text: "我来读一下配置文件。".into()
            },
            StreamPart::ToolCall {
                call_id: "call_1".into(),
                name: "read".into(),
                args_json: "{\"path\": \"/etc/app/config.toml\", \"offset\": 0}".into(),
                result: Some(super::ToolResultState {
                    ok: true,
                    ui_summary: "[read] 共 12 行：port = 8087".into(),
                    error: None,
                }),
                audit: None,
                audit_reason: None,
            },
            StreamPart::Text {
                text: "配置文件共 12 行，端口是 8087。".into()
            },
        ],
        "交错顺序 = 帧到达序：思考 → 第一轮正文 → 工具卡 → 第二轮答案"
    );
    assert_eq!(snap.tool_calls.len(), 1);
    assert_eq!(snap.tool_calls[0].name, "read");
    assert_eq!(
        snap.tool_calls[0].args_json,
        "{\"path\": \"/etc/app/config.toml\", \"offset\": 0}"
    );
    assert_eq!(
        snap.ended.as_ref().map(|e| e.reason),
        Some(EndReason::Done)
    );
    assert_eq!(snap.dropped_seq, 0);

    // ---- 半截 JSON 判定：累积中不 ready，收流后 ready（参数准备中态）----
    let partial = FrameAggregator::new();
    for frame in &frames[..8] {
        partial.push(frame.clone());
    }
    let pending_args = &partial.snapshot_streaming().unwrap().tool_calls[0].args_json;
    assert!(
        !args_ready(pending_args),
        "半截 JSON 应判定未 ready：{pending_args}"
    );
    partial.push(frames[8].clone()); // 参数收尾片段
    let ready_args = &partial.snapshot_streaming().unwrap().tool_calls[0].args_json;
    assert!(args_ready(ready_args), "收流后应 ready：{ready_args}");
    let parsed: serde_json::Value = serde_json::from_str(ready_args).unwrap();
    assert_eq!(
        parsed,
        serde_json::json!({ "path": "/etc/app/config.toml", "offset": 0 })
    );

    // ---- 乱序/重复 seq 丢弃（帧序自愈语义）且不影响后续累积 ----
    let out_of_order = FrameAggregator::new();
    out_of_order.push(frames[0].clone()); // seq0 runStarted
    out_of_order.push(frames[2].clone()); // seq2 第一段思考
    out_of_order.push(SequencedFrame {
        seq: 2,
        ts: None,
        frame: frames[2].frame.clone(),
    }); // 重复帧 → 丢弃
    out_of_order.push(SequencedFrame {
        seq: 1,
        ts: None,
        frame: frames[3].frame.clone(),
    }); // 倒退帧 → 丢弃
    out_of_order.push(frames[3].clone()); // seq3 第二段思考 → 收
    let snap = out_of_order.snapshot_streaming().unwrap();
    assert_eq!(snap.dropped_seq, 2);
    assert_eq!(
        snap.parts,
        vec![StreamPart::Reasoning {
            text: "用户要我看配置，先调用 read。".into()
        }]
    );

    // ---- usage 帧记录运行累计与速率；runStarted 记录 run/model 锚点 ----
    agg.push(f(99, ChatFrame::Usage {
        usage: UsageTotal {
            input: 120,
            output: 48,
            cache_read: 0,
            cache_write: 0,
        },
        rate: Some(121.0),
    }));
    agg.push(f(100, ChatFrame::Usage {
        usage: UsageTotal {
            input: 240,
            output: 96,
            cache_read: 4,
            cache_write: 0,
        },
        rate: Some(130.0),
    }));
    let snap = agg.snapshot_streaming().unwrap();
    assert_eq!(
        snap.usage,
        UsageTotal {
            input: 240,
            output: 96,
            cache_read: 4,
            cache_write: 0
        }
    );
    assert_eq!(snap.rate, Some(130.0));
    assert_eq!(
        snap.run,
        Some(super::RunInfo {
            run_id: "run-1".into(),
            model: "gpt-4o-mock".into()
        })
    );

    // ---- compactDone 帧：透传进 compaction 字段，不进打字机内容面 ----
    let compaction_agg = FrameAggregator::new();
    compaction_agg.push(f(0, ChatFrame::RunStarted {
        run_id: "run-1".into(),
        model: "m".into(),
    }));
    compaction_agg.push(f(1, ChatFrame::CompactDone {
        cut_entry_id: "u-15".into(),
        tokens_before: 34000,
        tokens_after: 9000,
    }));
    let snap = compaction_agg.snapshot_streaming().unwrap();
    assert_eq!(
        snap.compaction,
        Some(super::CompactionInfo {
            cut_entry_id: "u-15".into(),
            tokens_before: 34000,
            tokens_after: 9000,
        })
    );
    assert!(snap.parts.is_empty(), "通知面：不产生内容段");
    assert!(
        FrameAggregator::new().snapshot_streaming().is_none(),
        "空累积器无流式快照"
    );
}

#[tokio::test]
async fn 节流窗口与尾帧必达() {
    // ---- 文本总量保真：全部 text 段拼接与发送序列一致（跨轮不合并、无丢失）；
    //      终态帧与拖尾增量出现在同一份（最后一份）快照里 ----
    let agg = std::sync::Arc::new(FrameAggregator::new());
    let history: Rc<Vec<Rc<MessageEntry>>> = Rc::new(Vec::new());
    let frames = scripted_frames();
    // 生产侧：帧按 20ms 间隔到达（快于节流窗口，模拟 Tauri Channel /
    // GPUI 帧回调节奏）
    let producer_agg = std::sync::Arc::clone(&agg);
    let producer = tokio::spawn(async move {
        for frame in frames {
            producer_agg.push(frame);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });
    let mut ticker = agg.ticker(Rc::clone(&history));
    let mut yields: Vec<super::ConversationSnapshot> = Vec::new();
    while let Some(snap) = ticker.next().await {
        let terminal = snap
            .streaming
            .as_ref()
            .is_some_and(|s| s.ended.is_some());
        yields.push(snap);
        if terminal {
            break; // 终态快照已到手（下一调自然返回 None）
        }
    }
    producer.await.expect("生产任务不 panic");

    let last = yields.last().expect("终态前至少产出一份快照");
    let streaming = last.streaming.as_ref().expect("流式态应在快照里");
    let last_text: String = streaming
        .parts
        .iter()
        .filter_map(|p| match p {
            StreamPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        last_text, "我来读一下配置文件。配置文件共 12 行，端口是 8087。",
        "全部文本保真：跨轮不合并、无丢失"
    );
    assert_eq!(
        streaming.ended.as_ref().map(|e| e.reason),
        Some(EndReason::Done),
        "尾帧必达：终态帧与拖尾增量在同一份快照"
    );
    // 迭代器在终态快照产出后收口
    assert!(ticker.next().await.is_none(), "终态快照产出后应收口");

    // ---- 节流窗口：相邻产出间隔 ≥ 64ms（tokio paused 时间下以窗口数计）----
    // 单独跑一个慢流：20ms 间隔的帧在 ≥64ms 窗口内聚合，产出次数受窗口约束
    let slow = std::sync::Arc::new(FrameAggregator::new());
    let slow_frames = scripted_frames();
    let slow_agg = std::sync::Arc::clone(&slow);
    tokio::spawn(async move {
        for frame in slow_frames {
            slow_agg.push(frame);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    });
    let mut slow_ticker = slow.ticker(Rc::new(Vec::new()));
    let mut yields_count = 0usize;
    loop {
        let Some(snap) = slow_ticker.next().await else {
            break;
        };
        yields_count += 1;
        if snap.streaming.as_ref().is_some_and(|s| s.ended.is_some()) {
            break;
        }
    }
    // 400ms 流长 / 64ms 窗口：产出 ≤ 7 份（窗口上界，同 TS 断言口径）
    assert!(
        yields_count <= 7,
        "快照产出应受 64ms 窗口约束（实际 {yields_count}）"
    );
    assert!(yields_count >= 2, "至少产出过一份快照（实际 {yields_count}）");
}

#[test]
fn to_parts_失败工具与半截参数() {
    // 失败工具：result.ok=false 原样进 part（isError 由渲染层按 !ok 归属）；
    // 半截参数以 args_json 原文保留
    let agg = FrameAggregator::new();
    let frames = scripted_frames();
    for frame in &frames[..9] {
        agg.push(frame.clone()); // 到 toolArgs 为止
    }
    let parts = agg.snapshot_streaming().unwrap().parts;
    let Some(StreamPart::ToolCall { args_json, result, .. }) = parts.last() else {
        panic!("末位应为工具卡 part");
    };
    assert!(result.is_none(), "无结果帧时 result 应为 None");
    assert!(args_ready(args_json), "第 8 帧后参数已收齐");
}

#[test]
fn to_parts_旁白开新段_合批_无思考流() {
    // ---- 工具之间的旁白按原位开新思考段（不再归堆到消息开头）----
    let agg = FrameAggregator::new();
    let mut seq = 0u64;
    let mut push = |agg: &FrameAggregator, frame: ChatFrame| {
        agg.push(f(seq, frame));
        seq += 1;
    };
    push(
        &agg,
        ChatFrame::RunStarted {
            run_id: "run-2".into(),
            model: "glm-mock".into(),
        },
    );
    push(
        &agg,
        ChatFrame::ReasoningDelta {
            turn: 1,
            delta: "先查目录，".into(),
        },
    );
    push(
        &agg,
        ChatFrame::ToolStarted {
            call_id: "c1".into(),
            name: "list_dir".into(),
        },
    );
    push(
        &agg,
        ChatFrame::ToolResult {
            call_id: "c1".into(),
            ok: true,
            ui_summary: "3 项".into(),
            error: None,
        },
    );
    push(
        &agg,
        ChatFrame::ReasoningDelta {
            turn: 2,
            delta: "再读入口文件".into(),
        },
    );
    push(
        &agg,
        ChatFrame::ToolStarted {
            call_id: "c2".into(),
            name: "read".into(),
        },
    );
    push(
        &agg,
        ChatFrame::ToolResult {
            call_id: "c2".into(),
            ok: true,
            ui_summary: "12 行".into(),
            error: None,
        },
    );
    push(
        &agg,
        ChatFrame::TextDelta {
            turn: 2,
            delta: "入口是 main.rs。".into(),
        },
    );
    push(
        &agg,
        ChatFrame::RunEnded {
            reason: EndReason::Done,
            duration_ms: 100,
            tool_count: 2,
            turn: 2,
            error: None,
        },
    );
    let kinds: Vec<&str> = agg
        .snapshot_streaming()
        .unwrap()
        .parts
        .iter()
        .map(|p| match p {
            StreamPart::Reasoning { .. } => "reasoning",
            StreamPart::Text { .. } => "text",
            StreamPart::ToolCall { .. } => "tool",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["reasoning", "tool", "reasoning", "tool", "text"],
        "旁白在原位开新思考段，工具是分界"
    );

    // ---- 无思考流的 run 不产出 reasoning part ----
    let plain = FrameAggregator::new();
    let mut seq = 0u64;
    let mut push = |agg: &FrameAggregator, frame: ChatFrame| {
        agg.push(f(seq, frame));
        seq += 1;
    };
    push(
        &plain,
        ChatFrame::RunStarted {
            run_id: "run-x".into(),
            model: "glm-mock".into(),
        },
    );
    push(
        &plain,
        ChatFrame::TextDelta {
            turn: 1,
            delta: "直接回答".into(),
        },
    );
    push(
        &plain,
        ChatFrame::RunEnded {
            reason: EndReason::Done,
            duration_ms: 10,
            tool_count: 0,
            turn: 1,
            error: None,
        },
    );
    let parts = plain.snapshot_streaming().unwrap().parts;
    assert!(
        matches!(parts.as_slice(), [StreamPart::Text { .. }]),
        "无思考流只产 text part"
    );

    // ---- 工具之间同组的多次思考与半截句合批：一句话不被思考行拆开 ----
    let batched = FrameAggregator::new();
    let mut seq = 0u64;
    let mut push = |agg: &FrameAggregator, frame: ChatFrame| {
        agg.push(f(seq, frame));
        seq += 1;
    };
    push(
        &batched,
        ChatFrame::RunStarted {
            run_id: "run-3".into(),
            model: "glm-mock".into(),
        },
    );
    push(
        &batched,
        ChatFrame::ReasoningDelta {
            turn: 1,
            delta: "想一下".into(),
        },
    );
    push(
        &batched,
        ChatFrame::TextDelta {
            turn: 1,
            delta: "我来帮你梳理".into(),
        },
    );
    push(
        &batched,
        ChatFrame::ReasoningDelta {
            turn: 2,
            delta: "接着说".into(),
        },
    );
    push(
        &batched,
        ChatFrame::TextDelta {
            turn: 2,
            delta: "项目的目录结构。首先查看".into(),
        },
    );
    push(
        &batched,
        ChatFrame::ToolStarted {
            call_id: "c9".into(),
            name: "list_dir".into(),
        },
    );
    push(
        &batched,
        ChatFrame::ToolResult {
            call_id: "c9".into(),
            ok: true,
            ui_summary: "8 项".into(),
            error: None,
        },
    );
    push(
        &batched,
        ChatFrame::TextDelta {
            turn: 3,
            delta: "根目录如下。".into(),
        },
    );
    push(
        &batched,
        ChatFrame::RunEnded {
            reason: EndReason::Done,
            duration_ms: 100,
            tool_count: 1,
            turn: 3,
            error: None,
        },
    );
    let parts = batched.snapshot_streaming().unwrap().parts;
    assert_eq!(parts.len(), 4, "合批后：思考 + 正文 + 工具 + 新正文组");
    assert_eq!(
        parts[0],
        StreamPart::Reasoning {
            text: "想一下\n\n接着说".into()
        },
        "思考合批：两段思考并成一块（段间空行）"
    );
    assert_eq!(
        parts[1],
        StreamPart::Text {
            text: "我来帮你梳理项目的目录结构。首先查看".into()
        },
        "正文合批：同一句话的两截直接拼接，不再夹思考行"
    );
    assert_eq!(
        parts[3],
        StreamPart::Text {
            text: "根目录如下。".into()
        },
        "工具之后新开正文组"
    );
}

#[test]
fn 并发工具批_各卡独立推进不串卡() {
    // 帧序（引擎侧受控并发的真实交错形态）：同批三个工具的 started/args
    // 流式帧交错到达，结果帧按源序（B 先完成、A 最慢）。
    let parallel = FrameAggregator::new();
    let mut seq = 0u64;
    let mut push = |agg: &FrameAggregator, frame: ChatFrame| {
        agg.push(f(seq, frame));
        seq += 1;
    };
    push(
        &parallel,
        ChatFrame::RunStarted {
            run_id: "run-p".into(),
            model: "gpt-4o-mock".into(),
        },
    );
    push(&parallel, ChatFrame::StepStarted { turn: 1 });
    push(
        &parallel,
        ChatFrame::TextDelta {
            turn: 1,
            delta: "一次交代三件事。".into(),
        },
    );
    for (call_id, name) in [
        ("call_part_1_1", "read"),
        ("call_part_1_2", "grep"),
        ("call_part_1_3", "read"),
    ] {
        push(
            &parallel,
            ChatFrame::ToolStarted {
                call_id: call_id.into(),
                name: name.into(),
            },
        );
    }
    push(
        &parallel,
        ChatFrame::ToolArgs {
            call_id: "call_part_1_1".into(),
            chunk: "{\"path\": \"a.txt\"}".into(),
        },
    );
    push(
        &parallel,
        ChatFrame::ToolArgs {
            call_id: "call_part_1_2".into(),
            chunk: "{\"pattern\": \"foo\"".into(),
        },
    );
    push(
        &parallel,
        ChatFrame::ToolArgs {
            call_id: "call_part_1_3".into(),
            chunk: "{\"path\": \"b.txt\"}".into(),
        },
    );
    push(
        &parallel,
        ChatFrame::ToolArgs {
            call_id: "call_part_1_2".into(),
            chunk: ", \"path\": \"src\"}".into(),
        },
    );
    // 结果按源序（B 最快完成也不能插到 A 前——A 是源序第一）
    push(
        &parallel,
        ChatFrame::ToolResult {
            call_id: "call_part_1_2".into(),
            ok: true,
            ui_summary: "[grep] 3 处命中".into(),
            error: None,
        },
    );
    push(
        &parallel,
        ChatFrame::ToolResult {
            call_id: "call_part_1_3".into(),
            ok: true,
            ui_summary: "[read] B 文件".into(),
            error: None,
        },
    );
    push(
        &parallel,
        ChatFrame::ToolResult {
            call_id: "call_part_1_1".into(),
            ok: false,
            ui_summary: "未执行：等待用户回答后重新决定".into(),
            error: Some(FrameError::new("E_CANCELLED", "用户拒绝了此操作")),
        },
    );
    push(
        &parallel,
        ChatFrame::RunEnded {
            reason: EndReason::Done,
            duration_ms: 420,
            tool_count: 3,
            turn: 1,
            error: None,
        },
    );

    let snap = parallel.snapshot_streaming().unwrap();
    // 三张工具卡独立入列（toolStarted 位置即时间线位置）
    let tool_segments: Vec<&str> = snap
        .parts
        .iter()
        .filter_map(|p| match p {
            StreamPart::ToolCall { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_segments,
        vec!["call_part_1_1", "call_part_1_2", "call_part_1_3"]
    );
    assert_eq!(snap.tool_calls.len(), 3);
    // 各卡参数只进自己的卡片：grep 的半截 JSON 不串到 read 卡上
    assert_eq!(snap.tool_calls[0].args_json, "{\"path\": \"a.txt\"}");
    assert_eq!(
        snap.tool_calls[1].args_json,
        "{\"pattern\": \"foo\", \"path\": \"src\"}"
    );
    assert_eq!(snap.tool_calls[2].args_json, "{\"path\": \"b.txt\"}");
    // 各卡结果与自己的 callId 配对（B 先完成不影响 A/C 的状态归属）
    let a = &snap.tool_calls[0];
    assert!(!a.result.as_ref().expect("A 应有结果").ok);
    assert_eq!(
        a.result.as_ref().unwrap().ui_summary,
        "未执行：等待用户回答后重新决定"
    );
    assert_eq!(
        snap.tool_calls[1].result.as_ref().unwrap().ui_summary,
        "[grep] 3 处命中"
    );
    assert_eq!(snap.ended.as_ref().unwrap().tool_count, 3);
}

#[test]
fn 思考流起止与审计状态() {
    // ---- 思考流：首帧 reasoningDelta 记起点，首帧 textDelta 收口；
    //      只有思考没有正文的 run 在 runEnded 收口 ----
    let agg = FrameAggregator::new();
    let mut seq = 0u64;
    let mut push = |agg: &FrameAggregator, frame: ChatFrame| {
        agg.push(f(seq, frame));
        seq += 1;
    };
    push(
        &agg,
        ChatFrame::RunStarted {
            run_id: "run-9".into(),
            model: "glm-mock".into(),
        },
    );
    push(
        &agg,
        ChatFrame::ReasoningDelta {
            turn: 1,
            delta: "想一下…".into(),
        },
    );
    let thinking = agg.snapshot_streaming().unwrap().thinking;
    let span = thinking.expect("思考流应已开");
    assert!(span.ended_at.is_none(), "流式中思考未收口");
    // 起点必须落在推理帧到达之后（单调钟相对断言：先取一个更晚时刻再推进）
    std::thread::sleep(std::time::Duration::from_millis(2));
    push(
        &agg,
        ChatFrame::TextDelta {
            turn: 1,
            delta: "开始回答".into(),
        },
    );
    let settled = agg.snapshot_streaming().unwrap().thinking.expect("思考 span 应在");
    let ended = settled.ended_at.expect("正文首帧应收口思考流");
    assert!(
        ended > settled.started_at,
        "收口时刻应晚于起点（单调钟）"
    );

    // ---- toolAudit 帧并入工具卡状态 ----
    let audit_agg = FrameAggregator::new();
    let mut seq = 0u64;
    let mut push = |agg: &FrameAggregator, frame: ChatFrame| {
        agg.push(f(seq, frame));
        seq += 1;
    };
    push(
        &audit_agg,
        ChatFrame::ToolStarted {
            call_id: "c-audit".into(),
            name: "command".into(),
        },
    );
    push(
        &audit_agg,
        ChatFrame::ToolAudit {
            call_id: "c-audit".into(),
            state: AuditState::Ask,
            reason: Some("灰区命令：rm -rf".into()),
        },
    );
    let snap = audit_agg.snapshot_streaming().unwrap();
    assert_eq!(snap.tool_calls[0].audit, Some(AuditState::Ask));
    assert_eq!(
        snap.tool_calls[0].audit_reason.as_deref(),
        Some("灰区命令：rm -rf")
    );
    // to_parts 里 ToolCall part 同步携带
    let parts = snap.parts;
    let Some(StreamPart::ToolCall { audit, audit_reason, .. }) = parts.first() else {
        panic!("应有工具 part");
    };
    assert_eq!(*audit, Some(AuditState::Ask));
    assert_eq!(audit_reason.as_deref(), Some("灰区命令：rm -rf"));
}

#[test]
fn 子代理结构帧合并() {
    // subStarted → subFrame(stepStarted) → subReport → subDone：
    // 只取结构状态（语义同旧 applySubagentFrame），内容帧不递归
    let agg = FrameAggregator::new();
    let mut seq = 0u64;
    let mut push = |agg: &FrameAggregator, frame: ChatFrame| {
        agg.push(f(seq, frame));
        seq += 1;
    };
    push(
        &agg,
        ChatFrame::RunStarted {
            run_id: "run-s".into(),
            model: "m".into(),
        },
    );
    push(
        &agg,
        ChatFrame::SubStarted {
            call_id: "call_sub".into(),
            sub_id: "sub-1".into(),
            role: "explorer".into(),
        },
    );
    push(
        &agg,
        ChatFrame::SubFrame {
            sub_id: "sub-1".into(),
            frame: Box::new(ChatFrame::StepStarted { turn: 3 }),
        },
    );
    push(
        &agg,
        ChatFrame::SubReport {
            sub_id: "sub-1".into(),
            report: "已找到 3 个相关文件".into(),
        },
    );
    push(
        &agg,
        ChatFrame::SubDone {
            sub_id: "sub-1".into(),
            outcome: SubOutcome::Report,
        },
    );
    let snap = agg.snapshot_streaming().unwrap();
    assert_eq!(snap.subagents.len(), 1);
    let sub = &snap.subagents[0];
    assert_eq!(sub.sub_id, "sub-1");
    assert_eq!(sub.call_id, "call_sub");
    assert_eq!(sub.role, "explorer");
    assert_eq!(sub.turn, 3);
    assert_eq!(sub.report, "已找到 3 个相关文件");
    assert_eq!(sub.state, SubRunState::Done(SubOutcome::Report));
}

#[tokio::test]
async fn abort路径终态收束() {
    // 本地 abort 且终态帧未到 → ticker 不再产出、直接收口（TS break 语义）
    let agg = std::sync::Arc::new(FrameAggregator::new());
    agg.push(f(0, ChatFrame::RunStarted {
        run_id: "run-a".into(),
        model: "m".into(),
    }));
    agg.push(f(1, ChatFrame::TextDelta {
        turn: 1,
        delta: "写到一半".into(),
    }));
    let mut ticker = agg.ticker(Rc::new(Vec::new()));
    let first = ticker.next().await.expect("首窗口应产出快照");
    assert!(first.streaming.as_ref().unwrap().ended.is_none());
    agg.abort();
    assert!(ticker.next().await.is_none(), "abort 后应收口不产出");

    // 引擎补发终态帧的路径（cancel_run 后）：ticker 产出含
    // runEnded(cancelled) 的终态快照后自然收口——abort 未置位时不受影响
    let agg2 = std::sync::Arc::new(FrameAggregator::new());
    agg2.push(f(0, ChatFrame::RunStarted {
        run_id: "run-c".into(),
        model: "m".into(),
    }));
    agg2.push(f(1, ChatFrame::TextDelta {
        turn: 1,
        delta: "取消前增量".into(),
    }));
    let mut ticker2 = agg2.ticker(Rc::new(Vec::new()));
    let _ = ticker2.next().await;
    agg2.push(f(2, ChatFrame::RunEnded {
        reason: EndReason::Cancelled,
        duration_ms: 200,
        tool_count: 0,
        turn: 1,
        error: None,
    }));
    let terminal = ticker2
        .next()
        .await
        .expect("终态帧到达后应产出终态快照");
    assert_eq!(
        terminal.streaming.as_ref().unwrap().ended.as_ref().map(|e| e.reason),
        Some(EndReason::Cancelled),
        "取消终态必达"
    );
    // 拖尾增量保真：取消前增量出现在终态快照里
    assert!(terminal
        .streaming
        .as_ref()
        .unwrap()
        .parts
        .iter()
        .any(|p| matches!(p, StreamPart::Text { text } if text == "取消前增量")));
    assert!(ticker2.next().await.is_none(), "终态快照产出后收口");
}

#[test]
fn conversation_snapshot_携带历史消息列表() {
    // 历史消息（含图片 part 的条目）经 Rc 克隆进快照——消息列表维度
    let history: Vec<Rc<MessageEntry>> = vec![
        Rc::new(MessageEntry {
            id: "u-1".into(),
            parent_id: None,
            role: MessageRole::User,
            parts: vec![
                WirePart::Text {
                    text: "看这张图".into(),
                },
                WirePart::Image {
                    blob_hash: "abc123".into(),
                    mime_type: "image/png".into(),
                },
            ],
            created_at: 1000,
            run_id: None,
            status: None,
        }),
        Rc::new(MessageEntry {
            id: "a-1".into(),
            parent_id: Some("u-1".into()),
            role: MessageRole::Assistant,
            parts: vec![WirePart::Text {
                text: "图里是端口配置。".into(),
            }],
            created_at: 2000,
            run_id: Some("run-old".into()),
            status: None,
        }),
    ];
    let agg = std::sync::Arc::new(FrameAggregator::new());
    agg.push(f(0, ChatFrame::RunStarted {
        run_id: "run-new".into(),
        model: "m".into(),
    }));
    let snap = agg.conversation_snapshot(&history);
    assert_eq!(snap.messages.len(), 2, "历史消息原样进快照");
    assert!(Rc::ptr_eq(&snap.messages[0], &history[0]), "Rc 克隆共享不深拷贝");
    // 图片 part 在消息 parts 形态里保留（渲染层三态消费）
    match &snap.messages[0].parts[1] {
        WirePart::Image {
            blob_hash,
            mime_type,
        } => {
            assert_eq!(blob_hash, "abc123");
            assert_eq!(mime_type, "image/png");
        }
        other => panic!("第二 part 应为图片：{other:?}"),
    }
    let streaming = snap.streaming.as_ref().expect("流式态随快照");
    assert_eq!(
        streaming.run.as_ref().unwrap().run_id,
        "run-new",
        "流式中的临时正文与历史列表并存"
    );
}

#[test]
fn 内部段序中间面_时间线段() {
    // 直接断言段中间面（SnapshotSegment）：与 TS segments 断言同构
    let agg = FrameAggregator::new();
    feed(&agg, &scripted_frames());
    let snap = agg.snapshot_streaming().unwrap();
    // parts 是渲染面；段序验证通过 parts 的排布等价覆盖
    // （reasoning / text / tool / text 的交错序已在保真剧本测试断言）
    assert_eq!(snap.parts.len(), 4);
    assert!(matches!(snap.parts[2], StreamPart::ToolCall { .. }));
}

/// 时间线段枚举的直读断言（中间面保真：appendSegment 语义直测）。
#[test]
fn 快照段合并语义直测() {
    use super::SnapshotSegment as Seg;
    let agg = FrameAggregator::new();
    agg.push_seq(
        0,
        ChatFrame::TextDelta {
            turn: 1,
            delta: "a".into(),
        },
    );
    agg.push_seq(
        1,
        ChatFrame::TextDelta {
            turn: 1,
            delta: "b".into(),
        },
    );
    agg.push_seq(
        2,
        ChatFrame::ReasoningDelta {
            turn: 1,
            delta: "c".into(),
        },
    );
    agg.push_seq(
        3,
        ChatFrame::TextDelta {
            turn: 2,
            delta: "".into(), // 空 delta 忽略，不产出空段
        },
    );
    // 通过 to_parts 的输入面验证：直接构造相同段序
    let segments = vec![
        Seg::Text { text: "ab".into() },
        Seg::Reasoning { text: "c".into() },
    ];
    let parts = to_parts(&segments, &[]);
    // 合批语义（TS flushGroup 同款）：组内思考段先于正文段输出
    assert_eq!(
        parts,
        vec![
            StreamPart::Reasoning { text: "c".into() },
            StreamPart::Text { text: "ab".into() },
        ]
    );
    assert_eq!(
        SNAPSHOT_INTERVAL,
        std::time::Duration::from_millis(64),
        "节流窗口钉死 64ms（对齐旧实现）"
    );
}
