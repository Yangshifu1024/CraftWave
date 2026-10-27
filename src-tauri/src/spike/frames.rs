//! 【Spike】ChatFrame（技术设计文档 §3.4 的 S2 相关子集）与 64ms 节流合并器。
//!
//! 帧基座 `{ seq, type, ...字段 }` 用 serde 内部 tag 对应；seq 由
//! [`FrameSequencer`]（对应设计文档的 RunHandle）在出口处统一递增，
//! 帧构造侧不感知序号——与 §3.4「seq 由 Rust 端 RunHandle 递增」一致。

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;

/// 生产用的帧节流窗口（FR-01-03：64ms 节流合并）。
pub const THROTTLE_INTERVAL: Duration = Duration::from_millis(64);

/// ChatFrame：只保留 S2/S3 验证面用到的 P0 帧（§3.4 全集 9 种中的 8 种；
/// usage 帧在本 spike 由 usage 汇总驱动，一并保留字段面）。
/// variant 名与字段名均按 §3.1 契约：type 是 camelCase，字段也是 camelCase。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase", tag = "type")]
pub enum ChatFrame {
    RunStarted { run_id: String, model: String },
    StepStarted { turn: u32 },
    TextDelta { turn: u32, delta: String },
    ReasoningDelta { turn: u32, delta: String },
    ToolStarted { call_id: String, name: String },
    ToolArgs { call_id: String, chunk: String },
    ToolResult {
        call_id: String,
        ok: bool,
        ui_summary: String,
    },
    RunEnded {
        reason: EndReason,
        duration_ms: u64,
        tool_count: u32,
        turn: u32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Done,
    Error,
    Cancelled,
}

/// 帧出口：生产实现发 tauri `ipc::Channel<ChatFrame>`，测试实现收进 Vec。
/// 抽成 trait 是为了让帧管线（节流、seq、桥接）不依赖 tauri 运行时。
pub trait FrameSink: Send {
    fn send(&mut self, frame: ChatFrame);
}

/// 收集器实现：按到达顺序保存全部帧（含 seq），供测试断言。
#[derive(Default)]
pub struct CollectingSink {
    pub frames: Vec<SequencedFrame>,
}

/// 带序号的帧（序列化后即 §3.4 的 `{ seq, type, ...}` 基座）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SequencedFrame {
    pub seq: u64,
    #[serde(flatten)]
    pub frame: ChatFrame,
}

impl FrameSink for CollectingSink {
    fn send(&mut self, frame: ChatFrame) {
        // seq 由 sink 出口递增，保证「seq 严格递增且与发送顺序一致」（§3.4 排序规则）
        let seq = self.frames.len() as u64;
        self.frames.push(SequencedFrame { seq, frame });
    }
}

/// 64ms 节流合并器（S2 核心）：「首增量立即出帧，其后相邻 flush 间隔 ≥ 窗口」。
///
/// - 合并语义：窗口（64ms）内到达的同型增量在下次 flush 时各并成一帧；
/// - 节流语义：`should_flush()` 以真实时钟保证相邻 flush 间隔 ≥ 64ms——
///   首个增量到达即出帧（TTFT 优先），之后无论事件多密至多 64ms 一帧，
///   打字机节奏稳定且与驱动方式（外部 tick 或 push 内检查）无关；
/// - 非增量帧（toolStarted/toolResult/...）不经节流器直通，保证 toolResult
///   必晚于同 callId 全部 toolArgs（§3.4 排序规则）。
#[derive(Default)]
pub struct DeltaThrottler {
    text: BTreeMap<u32, String>,
    reasoning: BTreeMap<u32, String>,
    tool_args: BTreeMap<String, String>,
    /// 上次 flush 时刻；None = 从未 flush（首帧立即）。
    last_flush: Option<std::time::Instant>,
}

impl DeltaThrottler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_text(&mut self, turn: u32, delta: &str) {
        self.text.entry(turn).or_default().push_str(delta);
    }

    pub fn push_reasoning(&mut self, turn: u32, delta: &str) {
        self.reasoning.entry(turn).or_default().push_str(delta);
    }

    pub fn push_tool_args(&mut self, call_id: &str, chunk: &str) {
        self.tool_args
            .entry(call_id.to_owned())
            .or_default()
            .push_str(chunk);
    }

    /// 是否有未 flush 的 pending 增量。
    pub fn has_pending(&self) -> bool {
        !self.text.is_empty() || !self.reasoning.is_empty() || !self.tool_args.is_empty()
    }

    /// 距上次 flush 已满节流窗口（或从未出帧且窗口里有 pending，首帧立即）。
    pub fn should_flush(&self) -> bool {
        match self.last_flush {
            None => self.has_pending(),
            Some(at) => at.elapsed() >= THROTTLE_INTERVAL,
        }
    }

    /// 把窗口内累积的增量合并成帧发给 sink，并清空窗口。
    pub fn flush(&mut self, out: &mut dyn FrameSink) {
        for (turn, delta) in std::mem::take(&mut self.text) {
            if !delta.is_empty() {
                out.send(ChatFrame::TextDelta { turn, delta });
            }
        }
        for (turn, delta) in std::mem::take(&mut self.reasoning) {
            if !delta.is_empty() {
                out.send(ChatFrame::ReasoningDelta { turn, delta });
            }
        }
        for (call_id, chunk) in std::mem::take(&mut self.tool_args) {
            if !chunk.is_empty() {
                out.send(ChatFrame::ToolArgs { call_id, chunk });
            }
        }
        self.last_flush = Some(std::time::Instant::now());
    }
}
