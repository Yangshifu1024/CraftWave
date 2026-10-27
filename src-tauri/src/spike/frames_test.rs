//! 【Spike】节流合并器单元测试（独立测试文件，源码零测试）。

use std::thread::sleep;
use std::time::Duration;

use super::frames::{ChatFrame, CollectingSink, DeltaThrottler, THROTTLE_INTERVAL};

#[test]
fn 同窗口文本增量合并为一帧且拼接无损() {
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
}

#[test]
fn 不同类型增量各自成帧_文本思考参数有序() {
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
}

#[test]
fn flush_后窗口清空_二次flush无帧() {
    let mut t = DeltaThrottler::new();
    t.push_text(1, "x");
    let mut sink = CollectingSink::default();
    t.flush(&mut sink);
    assert_eq!(sink.frames.len(), 1);
    t.flush(&mut sink);
    assert_eq!(sink.frames.len(), 1, "空窗口不再出帧");
    assert!(!t.has_pending());
}

#[test]
fn seq_严格递增且与帧序一致() {
    let mut t = DeltaThrottler::new();
    t.push_text(1, "a");
    t.push_text(1, "b");
    t.push_reasoning(1, "r");
    let mut sink = CollectingSink::default();
    t.flush(&mut sink);
    // CollectingSink 的 seq 在出口递增（对应 RunHandle 语义）
    for (i, f) in sink.frames.iter().enumerate() {
        assert_eq!(f.seq, i as u64);
    }
}

#[test]
fn 节流语义_首帧立即_窗口内不再出帧() {
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

#[test]
fn 帧协议_camel_case字段命名() {
    let mut t = DeltaThrottler::new();
    t.push_tool_args("call_1", "{\"pa");
    let mut sink = CollectingSink::default();
    t.flush(&mut sink);
    let v = serde_json::to_value(&sink.frames[0].frame).unwrap();
    assert_eq!(v["type"], "toolArgs");
    assert_eq!(v["callId"], "call_1", "字段名为 camelCase（§3.1）");
    assert_eq!(v["chunk"], "{\"pa");
}
