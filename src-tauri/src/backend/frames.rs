//! ChatFrame P0 全集 9 帧（技术设计文档 §3.4）与 64ms 节流合并器。
//!
//! 帧基座 `{ seq, type, ts?, ...字段 }`：`seq` 与 `ts` 由 [`FrameSink`] 出口
//! 统一填写（§3.4「seq 由 Rust 端 RunHandle 递增」；`ts` = 出口 epoch ms，
//! 供端到端延迟埋点与 §5.6 对账，S2 实证加入）。帧构造侧不感知序号。
//!
//! `UsageTotal` 为 §3.2 共享类型的 Rust 侧镜像（providers/types 不承载帧面，
//! 帧协议自包含）。

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;

/// 生产用的帧节流窗口（FR-01-03：64ms 节流合并）。
pub const THROTTLE_INTERVAL: Duration = Duration::from_millis(64);

/// §3.2 UsageTotal（帧面 usage 载荷；camelCase 由 serde 层统一）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotal {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl UsageTotal {
    /// 累加（每 turn 收口把该轮 usage 记入运行累计）。
    pub fn accumulate(&mut self, other: &UsageTotal) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

/// ChatFrame：P0 全集 9 帧（§3.4）。variant 名与字段名均按 §3.1 契约：
/// type 是 camelCase，字段也是 camelCase。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum ChatFrame {
    RunStarted {
        run_id: String,
        model: String,
    },
    StepStarted {
        turn: u32,
    },
    TextDelta {
        turn: u32,
        delta: String,
    },
    ReasoningDelta {
        turn: u32,
        delta: String,
    },
    ToolStarted {
        call_id: String,
        name: String,
    },
    ToolArgs {
        call_id: String,
        chunk: String,
    },
    ToolResult {
        call_id: String,
        ok: bool,
        ui_summary: String,
        /// 错误面（§3.4 `error?`）：被取消工具补 E_CANCELLED（§5.3），其余 None。
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<FrameError>,
    },
    Usage {
        usage: UsageTotal,
        /// 输出 tok/s（FR-01-13）；RunLoop 按「本轮输出 tokens / 本轮流时长」计算。
        #[serde(skip_serializing_if = "Option::is_none")]
        rate: Option<f64>,
    },
    RunEnded {
        reason: EndReason,
        duration_ms: u64,
        tool_count: u32,
        turn: u32,
        /// 错误终态面（§3.4 `error?`）：reason=error 时承载（§11.3 分类器为
        /// provider 适配层后续票，本票以 E_INTERNAL 兜底）。
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<FrameError>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Done,
    Error,
    Cancelled,
}

/// 帧错误面（§3.4 `error?: { code, message }`，§3.6 错误码注册表）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FrameError {
    pub code: String,
    pub message: String,
}

impl FrameError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// 帧出口：生产实现发 tauri `ipc::Channel<ChatFrame>`（host 层适配，出口填
/// seq + ts），测试实现收进 Vec。抽成 trait 让帧管线（节流、RunLoop）不依赖
/// tauri 运行时。
pub trait FrameSink: Send {
    fn send(&mut self, frame: ChatFrame);
}

/// 带序号与出口时刻的帧（序列化后即 §3.4 的 `{ seq, type, ts?, ...}` 基座）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SequencedFrame {
    pub seq: u64,
    /// Rust 出口 epoch ms（可选；测试注入的 sink 可缺省）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<i64>,
    #[serde(flatten)]
    pub frame: ChatFrame,
}

/// 收集器实现：按到达顺序保存全部帧（含 seq），供测试断言。
#[derive(Default)]
pub struct CollectingSink {
    pub frames: Vec<SequencedFrame>,
}

impl FrameSink for CollectingSink {
    fn send(&mut self, frame: ChatFrame) {
        // seq 由 sink 出口递增，保证「seq 严格递增且与发送顺序一致」（§3.4 排序规则）
        let seq = self.frames.len() as u64;
        self.frames.push(SequencedFrame {
            seq,
            ts: None,
            frame,
        });
    }
}

/// 64ms 节流合并器（§3.4 节流语义，S2 实证钉死）：
/// 「首增量立即出帧，其后相邻 flush 间隔 ≥ 窗口」。
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
