//! 帧协议与节流合并器的单元测试（测试独立成文件，源码零测试）。
//!
//! 覆盖产品化各帧的形态与节流行为：usage / stepStarted / runEnded.error
//! 帧的形态与 `ts` 出口字段。

use std::thread::sleep;
use std::time::Duration;

use super::frames::{
    ChatFrame, CollectingSink, DeltaThrottler, EndReason, FrameError, FrameSink, SequencedFrame,
    THROTTLE_INTERVAL, UsageTotal,
};

/// 断言同一窗口内的多个文本增量合并为一帧且内容拼接无损；出帧清空窗口
/// （连续第二次 flush 不再产生新帧、无 pending）。
#[test]
fn same_window_text_merged_lossless_and_flush_clears() {
    let mut t = DeltaThrottler::new();
    t.push_text(1, "你好");
    t.push_text(1, "，");
    t.push_text(1, "世界");
    let mut sink = CollectingSink::default();
    t.flush(&mut sink);

    assert_eq!(sink.frames.len(), 1, "三个增量合并为一帧");
    let ChatFrame::TextDelta { turn, delta } = &sink.frames[0].frame else {
        panic!("应为 TextDelta 帧");
    };
    assert_eq!(*turn, 1);
    assert_eq!(delta, "你好，世界");

    // flush 语义：窗口已清空，二次 flush 不出帧
    t.flush(&mut sink);
    assert_eq!(sink.frames.len(), 1, "空窗口不再出帧");
    assert!(!t.has_pending());
}

/// 断言节流语义：首帧立即出，其后 64ms 窗口内不出帧，窗口等待期间的
/// 增量不丢失（满窗口后合并出帧）。
#[test]
fn throttle_first_frame_immediate_within_window_quiet() {
    let mut t = DeltaThrottler::new();
    t.push_text(1, "第一帧立即");
    assert!(t.should_flush(), "从未出帧且有 pending → 立即");
    let mut sink = CollectingSink::default();
    t.flush(&mut sink);

    // 刚 flush 过，即使窗口里又有增量也不该出帧
    t.push_text(1, "等窗口");
    assert!(!t.should_flush(), "距上次 flush 未满 64ms");
    sleep(THROTTLE_INTERVAL + Duration::from_millis(5));
    assert!(t.should_flush(), "满 64ms 后应出帧");
    t.flush(&mut sink);
    let ChatFrame::TextDelta { delta, .. } = &sink.frames[1].frame else {
        panic!("应为第二个 TextDelta 帧");
    };
    assert_eq!(delta, "等窗口", "窗口等待期间增量不丢");
}

/// 断言三类增量各自出帧且顺序固定（text → reasoning → toolArgs），半截
/// JSON 片段拼接还原；seq 从 0 起严格递增且与帧的发送顺序一致。
#[test]
fn delta_kinds_ordered_seq_increasing_partial_json_joined() {
    let mut t = DeltaThrottler::new();
    t.push_reasoning(1, "思考");
    t.push_text(1, "答");
    t.push_tool_args("call_1", "{\"pa");
    t.push_tool_args("call_1", "th\": \"/a\"}");
    let mut sink = CollectingSink::default();
    t.flush(&mut sink);

    let kinds: Vec<String> = sink
        .frames
        .iter()
        .map(|f| {
            serde_json::to_value(&f.frame).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(kinds, vec!["textDelta", "reasoningDelta", "toolArgs"]);
    let ChatFrame::ToolArgs { call_id, chunk } = &sink.frames[2].frame else {
        panic!("应为 toolArgs 帧");
    };
    assert_eq!(call_id, "call_1");
    assert_eq!(chunk, "{\"path\": \"/a\"}", "半截片段拼接还原完整 JSON");

    // CollectingSink 的 seq 在出口递增（对应 RunHandle 语义）
    for (i, f) in sink.frames.iter().enumerate() {
        assert_eq!(f.seq, i as u64, "seq 严格递增且与发送顺序一致");
    }
}

/// 断言帧序列化契约：字段名 camelCase、基座 `{ seq, type,
/// ts?, ... }` 的 ts 可选且缺省整个省略、usage 帧的累计与形态（rate 可选
/// 缺省省略）、runEnded 的 reason/error 面（cancelled 无 error 字段省略）。
#[test]
fn frame_serialization_contract_cases() {
    // toolArgs 帧：camelCase 字段（契约）
    let mut t = DeltaThrottler::new();
    t.push_tool_args("call_1", "{\"pa");
    let mut sink = CollectingSink::default();
    t.flush(&mut sink);
    let v = serde_json::to_value(&sink.frames[0].frame).unwrap();
    assert_eq!(v["type"], "toolArgs");
    assert_eq!(v["callId"], "call_1", "字段名为 camelCase");
    assert_eq!(v["chunk"], "{\"pa");

    // 基座：ts 有值时出现、缺省时字段整个省略（可选字段无 null 满字段）
    let frame = SequencedFrame {
        seq: 7,
        ts: Some(1_760_000_000_000),
        frame: ChatFrame::StepStarted { turn: 2 },
    };
    let v = serde_json::to_value(&frame).unwrap();
    assert_eq!(v["seq"], 7);
    assert_eq!(v["type"], "stepStarted");
    assert_eq!(v["turn"], 2);
    assert_eq!(v["ts"], 1_760_000_000_000i64);
    let frame = SequencedFrame {
        seq: 0,
        ts: None,
        frame: ChatFrame::StepStarted { turn: 1 },
    };
    let v = serde_json::to_value(&frame).unwrap();
    assert!(v.get("ts").is_none(), "ts 缺省时不出 null 字段");

    // usage 帧：多拍累计逐面相加；形态与 rate 缺省省略
    let mut total = UsageTotal::default();
    total.accumulate(&UsageTotal {
        input: 120,
        output: 48,
        cache_read: 0,
        cache_write: 0,
    });
    total.accumulate(&UsageTotal {
        input: 200,
        output: 30,
        cache_read: 64,
        cache_write: 8,
    });
    assert_eq!(
        total,
        UsageTotal {
            input: 320,
            output: 78,
            cache_read: 64,
            cache_write: 8
        }
    );
    sink.send(ChatFrame::Usage {
        usage: total,
        rate: Some(120.5),
    });
    let v = serde_json::to_value(&sink.frames[1]).unwrap();
    assert_eq!(v["type"], "usage");
    assert_eq!(v["usage"]["input"], 320);
    assert_eq!(v["usage"]["cacheRead"], 64);
    assert_eq!(v["rate"], 120.5);
    sink.send(ChatFrame::Usage {
        usage: total,
        rate: None,
    });
    let v = serde_json::to_value(&sink.frames[2]).unwrap();
    assert!(v.get("rate").is_none(), "rate 缺省省略");

    // runEnded 帧：reason=error 带 error 面；cancelled 无 error 字段省略
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Error,
        duration_ms: 1234,
        tool_count: 2,
        turn: 3,
        error: Some(FrameError::new("E_INTERNAL", "模型流中断")),
    });
    let v = serde_json::to_value(&sink.frames[3]).unwrap();
    assert_eq!(v["type"], "runEnded");
    assert_eq!(v["reason"], "error");
    assert_eq!(v["durationMs"], 1234);
    assert_eq!(v["toolCount"], 2);
    assert_eq!(v["error"]["code"], "E_INTERNAL");
    assert_eq!(v["error"]["message"], "模型流中断");
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Cancelled,
        duration_ms: 5,
        tool_count: 0,
        turn: 1,
        error: None,
    });
    let v = serde_json::to_value(&sink.frames[4]).unwrap();
    assert_eq!(v["reason"], "cancelled");
    assert!(v.get("error").is_none());
}
