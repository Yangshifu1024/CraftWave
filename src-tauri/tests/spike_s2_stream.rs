//! 【S2 Spike】rig 流事件保真集成测试：
//! mock SSE（混合事件序列）→ rig 流事件 → ChatFrame → 断言无丢失无乱序，
//! 64ms 节流下帧间隔实测，端到端延迟埋点对照 NFR-02（<100ms）。

mod common;

use std::sync::{Arc, Mutex};

use common::mock_openai::{
    MockOpenAI, TURN1_REASONING, TURN1_TEXT, TURN1_TOOL_ARG_PARTS, TURN2_TEXT,
};
use common::{drive_direct, drive_throttled, turn_driver, FakeExecutor, TimedFrame, TimedSink, TOOL_READ};
use craftwave_lib::spike::frames::{ChatFrame, DeltaThrottler, EndReason};

/// 抽取某种帧的负载文本。
fn join_frames(frames: &[TimedFrame], pred: impl Fn(&ChatFrame) -> bool) -> String {
    frames
        .iter()
        .filter(|t| pred(&t.frame.frame))
        .map(|t| match &t.frame.frame {
            ChatFrame::TextDelta { delta, .. } => delta.clone(),
            ChatFrame::ReasoningDelta { delta, .. } => delta.clone(),
            ChatFrame::ToolArgs { chunk, .. } => chunk.clone(),
            _ => String::new(),
        })
        .collect()
}

fn is_text(f: &ChatFrame) -> bool {
    matches!(f, ChatFrame::TextDelta { .. })
}
fn is_reasoning(f: &ChatFrame) -> bool {
    matches!(f, ChatFrame::ReasoningDelta { .. })
}
fn is_args(f: &ChatFrame) -> bool {
    matches!(f, ChatFrame::ToolArgs { .. })
}

#[tokio::test]
async fn s2_直通模式_流事件无丢失无乱序() {
    let mock = MockOpenAI::start().await;
    let sink = TimedSink::default();
    let response = drive_direct(
        &turn_driver(&mock),
        common::new_run(),
        Arc::new(FakeExecutor),
        sink.clone(),
    )
    .await
    .expect("手动循环应正常跑完");
    let frames = sink.0.lock().unwrap().clone();
    assert!(!frames.is_empty());

    // —— 保真断言：拼接与发送序列一致（无丢失、无重复）——
    let text = join_frames(&frames, is_text);
    let reasoning = join_frames(&frames, is_reasoning);
    let args = join_frames(&frames, is_args);
    assert_eq!(text, format!("{TURN1_TEXT}{TURN2_TEXT}"), "文本 delta 无丢失");
    assert_eq!(reasoning, TURN1_REASONING, "思考 delta 无丢失");
    assert_eq!(
        args,
        TURN1_TOOL_ARG_PARTS.concat(),
        "工具参数增量（含半截 JSON 片段）无丢失"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&args).expect("拼接后的参数是完整 JSON");
    assert_eq!(parsed["path"], "/etc/app/config.toml");

    // —— 乱序断言：rig 事件顺序 = mock 发送顺序 ——
    let kind_of = |f: &ChatFrame| match f {
        ChatFrame::ReasoningDelta { .. } => "reasoning",
        ChatFrame::TextDelta { .. } => "text",
        ChatFrame::ToolStarted { .. } => "tool_started",
        ChatFrame::ToolArgs { .. } => "tool_args",
        ChatFrame::ToolResult { .. } => "tool_result",
        _ => "other",
    };
    let kinds: Vec<&str> = frames.iter().map(|t| kind_of(&t.frame.frame)).collect();
    let first_text = kinds.iter().position(|k| *k == "text").expect("有文本帧");
    let last_reasoning = kinds.iter().rposition(|k| *k == "reasoning").expect("有思考帧");
    assert!(last_reasoning < first_text, "turn1 思考先于文本（mock 脚本顺序）");
    let started = kinds.iter().position(|k| *k == "tool_started").expect("有 toolStarted");
    let first_args = kinds.iter().position(|k| *k == "tool_args").unwrap();
    assert!(started < first_args, "toolStarted 早于 toolArgs");

    // —— seq 严格递增（§3.4 排序规则）——
    for (i, t) in frames.iter().enumerate() {
        assert_eq!(t.frame.seq, i as u64);
    }
    // toolResult 晚于同 callId 的全部 toolArgs
    let last_args = kinds.iter().rposition(|k| *k == "tool_args").unwrap();
    let result = kinds.iter().position(|k| *k == "tool_result").expect("有 toolResult");
    assert!(last_args < result);

    // —— 工具帧内容 ——
    let ChatFrame::ToolStarted { call_id, name } = frames
        .iter()
        .map(|t| &t.frame.frame)
        .find(|f| matches!(f, ChatFrame::ToolStarted { .. }))
        .expect("有 toolStarted")
    else {
        unreachable!()
    };
    assert_eq!(name, TOOL_READ);
    assert!(
        call_id.starts_with("call_part_"),
        "桥接层以 rig part 序号生成 callId（实际 {call_id}）"
    );
    assert_eq!(response.output(), TURN2_TEXT, "第二轮回喂后的最终答复");

    report_latency(&mock, &frames, "直通").await;
}

/// 打字机流畅佐证：64ms 节流合并下，文本帧间隔 ≥ 节流窗口，拼接仍无损。
#[tokio::test]
async fn s2_节流模式_64ms合并_帧间隔实测() {
    let mock = MockOpenAI::start().await; // chunk 间隔 30ms
    let throttler = Arc::new(Mutex::new(DeltaThrottler::new()));
    let sink = TimedSink::default();

    let response = drive_throttled(
        &turn_driver(&mock),
        common::new_run(),
        Arc::new(FakeExecutor),
        throttler.clone(),
        sink.clone(),
    )
    .await
    .expect("节流模式手动循环应正常跑完");
    assert_eq!(response.output(), TURN2_TEXT);

    // 收尾 flush（循环结束后窗口里可能还有 pending）
    throttler.lock().unwrap().flush(&mut sink.clone());

    let frames = sink.0.lock().unwrap().clone();
    let text_frames: Vec<&TimedFrame> = frames
        .iter()
        .filter(|t| is_text(&t.frame.frame))
        .collect();
    assert!(
        (2..=4).contains(&text_frames.len()),
        "两轮共 5 段文本 delta 应合并为 2~4 帧（每轮首 delta 立即 + 64ms 窗口合并），实际 {}",
        text_frames.len()
    );
    let joined: String = text_frames
        .iter()
        .map(|t| match &t.frame.frame {
            ChatFrame::TextDelta { delta, .. } => delta.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(joined, format!("{TURN1_TEXT}{TURN2_TEXT}"), "节流合并后拼接仍无损");

    // 帧间隔实测：相邻帧间隔 ≥ 节流窗口（无 30ms 连发）；上限放行两轮间的工具往返
    let gaps: Vec<f64> = text_frames
        .windows(2)
        .map(|w| (w[1].at - w[0].at).as_secs_f64() * 1000.0)
        .collect();
    println!("S2 节流模式文本帧间隔（ms）：{gaps:?}");
    for gap in &gaps {
        assert!(
            (55.0..=400.0).contains(gap),
            "帧间隔 {gap}ms 异常：低于 64ms 节流窗口（55ms 容忍）即节流失效，过高即流停滞"
        );
    }
    let chars_per_frame: Vec<usize> = text_frames
        .iter()
        .map(|t| match &t.frame.frame {
            ChatFrame::TextDelta { delta, .. } => delta.chars().count(),
            _ => 0,
        })
        .collect();
    println!("S2 每帧合并字符数：{chars_per_frame:?}（mock 每 30ms 一段 5~9 字）");
    assert!(
        chars_per_frame.iter().any(|c| *c >= 10),
        "应存在合并了多个 delta 的帧（≥10 字），否则节流未生效"
    );

    report_latency(&mock, &frames, "节流64ms").await;
}

/// 端到端延迟埋点：对照 mock chunk 发送时刻与帧离开帧管线的时刻。
/// 这是 Rust 侧链路（mock 发出 → rig 解码 → 桥接 → （可选节流）出帧）；
/// 「帧 → assistant-ui 渲染」由 ui/ 侧 vitest 单独实测，两端相加对照 NFR-02（<100ms）。
async fn report_latency(mock: &MockOpenAI, frames: &[TimedFrame], label: &str) {
    let sent = mock.sent_chunks().await;
    let first_sent = sent.first().expect("mock 应有发送记录").at;
    let first_frame = frames.first().expect("应有帧").at;
    let ttft_ms = (first_frame - first_sent).as_secs_f64() * 1000.0;
    // 单帧处理延迟：每个 mock chunk 发出 → 同序位帧离开帧管线（直通一一对应；
    // 节流模式下合并帧以「其首个贡献 chunk」计，即合并帧延迟 = 窗口等待 + 处理）
    let sent_times: Vec<f64> = sent
        .iter()
        .filter(|c| matches!(c.kind, "reasoning" | "content" | "tool_args"))
        .map(|c| c.at.duration_since(first_sent).as_secs_f64() * 1000.0)
        .collect();
    let frame_times: Vec<f64> = frames
        .iter()
        .map(|t| t.at.duration_since(first_sent).as_secs_f64() * 1000.0)
        .collect();
    // 帧管道单帧延迟上界：直通=每帧紧跟其贡献 chunk；节流=窗口(64ms)+处理
    let mut paired: Vec<f64> = Vec::new();
    if label == "直通" {
        // 逐帧配对同序位的 delta chunk（Start/End/finish/usage 无帧，跳过）
        let delta_frames: Vec<f64> = frames
            .iter()
            .filter(|t| {
                matches!(
                    t.frame.frame,
                    ChatFrame::TextDelta { .. }
                        | ChatFrame::ReasoningDelta { .. }
                        | ChatFrame::ToolArgs { .. }
                )
            })
            .map(|t| t.at.duration_since(first_sent).as_secs_f64() * 1000.0)
            .collect();
        for (f, s) in delta_frames.iter().zip(sent_times.iter()) {
            paired.push(f - s);
        }
    }
    let pairing = if paired.is_empty() {
        String::new()
    } else {
        let max_p = paired.iter().cloned().fold(0.0, f64::max);
        let avg_p = paired.iter().sum::<f64>() / paired.len() as f64;
        format!(
            "，单帧延迟(chunk→帧)平均={avg_p:.2}ms 最大={max_p:.2}ms"
        )
    };
    println!(
        "S2[{label}] 延迟埋点：TTFT(mock首delta→首帧)={ttft_ms:.2}ms{pairing}；帧时刻分布={frame_times:?}ms",
    );
    // NFR-02 预算 <100ms 覆盖「Rust 收到 delta → 前端渲染」全链；本断言保证
    // Rust 侧（含首帧直通的 64ms 节流策略）给前端渲染留出余量。
    assert!(
        ttft_ms < 100.0,
        "TTFT {ttft_ms:.1}ms 超出 NFR-02 预算（<100ms）"
    );
}

/// 帧协议序列化形态检查（camelCase + 内部 tag，§3.1/§3.4 契约）。
#[test]
fn s2_帧协议_camel_case与snake_case_tag() {
    let v = serde_json::to_value(ChatFrame::RunEnded {
        reason: EndReason::Done,
        duration_ms: 12,
        tool_count: 1,
        turn: 2,
    })
    .unwrap();
    assert_eq!(v["type"], "runEnded");
    assert_eq!(v["reason"], "done");
    assert_eq!(v["durationMs"], 12);
    let v = serde_json::to_value(ChatFrame::ToolArgs {
        call_id: "call_1".into(),
        chunk: "{\"pa".into(),
    })
    .unwrap();
    assert_eq!(v["type"], "toolArgs");
    assert_eq!(v["callId"], "call_1");
}
