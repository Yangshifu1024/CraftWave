//! 帧协议全集与 64 毫秒节流合并器（技术设计文档 §3.4）。
//!
//! 名词解释：帧（frame）是后端推送给前端的一条事件消息，是界面渲染的
//! 最小数据单元（打字机文本、工具卡片、用量统计都由帧驱动）。本文件
//! 定义全部 9 种帧（ChatFrame）。
//!
//! 每帧的公共基座为 `{ seq, type, ts?, ...各帧字段 }`：`seq`（全 run 递增
//! 的流水号，前端按它排序）与 `ts`（出口时刻的 Unix 毫秒，供端到端延迟
//! 统计与前后端对账）由 [`FrameSink`] 出口统一填写，构造帧的代码不感知
//! 序号（§3.4）。
//!
//! `UsageTotal` 是 §3.2 共享类型在 Rust 侧的镜像（providers 模块的类型
//! 不混入帧面，帧协议自包含、不依赖 providers）。

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;

/// 生产用的帧节流窗口时长（需求 FR-01-03：相邻两次增量出帧至少间隔 64 毫秒）。
pub const THROTTLE_INTERVAL: Duration = Duration::from_millis(64);

/// §3.2 UsageTotal（帧面 usage 载荷，即模型的 token 用量统计；字段名的
/// camelCase 由 serde 层统一转换）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct UsageTotal {
    /// 输入消耗的 token 数（token = 模型处理文本的最小单位）。
    pub input: u64,
    /// 输出消耗的 token 数。
    pub output: u64,
    /// 命中缓存读取的输入 token 数（省钱部分）。
    pub cache_read: u64,
    /// 写入缓存的 token 数。
    pub cache_write: u64,
}

/// UsageTotal 的累加实现。
impl UsageTotal {
    /// 累加另一份用量（每轮结束把该轮 usage 记入 run 的累计值）。
    pub fn accumulate(&mut self, other: &UsageTotal) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

/// ChatFrame：帧协议全集 9 帧（§3.4）。帧类型名与字段名均按 §3.1 契约：
/// type 值是 camelCase，字段名也是 camelCase。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "type"
)]
pub enum ChatFrame {
    /// run 开始（起手帧）：告知前端本次 run 的 id 与所用模型。
    RunStarted { run_id: String, model: String },
    /// 一轮开始（turn = 一次"请求模型并收到完整回复"的过程）。
    StepStarted { turn: u32 },
    /// 正文文本增量（打字机效果的数据源）。
    TextDelta { turn: u32, delta: String },
    /// 推理文本增量（模型思维链的流式展示）。
    ReasoningDelta { turn: u32, delta: String },
    /// 工具调用开始（界面据此创建工具卡片）。
    ToolStarted { call_id: String, name: String },
    /// 工具参数增量（半截 JSON 原样转发，前端先显示"参数准备中"）。
    ToolArgs { call_id: String, chunk: String },
    /// 工具执行结果（成功摘要或错误面）。
    ToolResult {
        call_id: String,
        ok: bool,
        ui_summary: String,
        /// 错误面（§3.4 `error?`）：被取消的工具补 E_CANCELLED（§5.3），
        /// 其余情况为 None。
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        error: Option<FrameError>,
    },
    /// 用量统计（本轮收口与 run 累计值，可带输出速度）。
    Usage {
        usage: UsageTotal,
        /// 输出速度，单位 token/秒（FR-01-13）；主循环按"本轮输出 token 数
        /// ÷ 本轮流式调用耗时"计算。
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        rate: Option<f64>,
    },
    /// run 结束（终态帧，必为最后一帧）：正常完成/出错/用户取消三种原因。
    RunEnded {
        reason: EndReason,
        duration_ms: u64,
        tool_count: u32,
        turn: u32,
        /// 错误终态面（§3.4 `error?`）：reason=error 时承载（§11.3 的错误
        /// 分类器属 provider 适配层的后续工单，本阶段以 E_INTERNAL 兜底）。
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        error: Option<FrameError>,
    },
    /// 压缩完成（§3.4【M2】增补帧，04 号票）：压缩边界与压缩前后上下文
    /// 占用估算。通知面（FrameBuffer 透传），不进打字机内容面。
    CompactDone {
        cut_entry_id: String,
        tokens_before: u64,
        tokens_after: u64,
    },
    /// runState 摘要帧（§3.4【M2】增补帧，05 号票）：`attach_run` 重放窗口
    /// 耗尽时补发一次，前端据此整体对账。负载对齐 §3.2 RunSnapshot 的运行中
    /// 形态；不进环形缓冲（幂等对账辅助帧）。
    RunSnapshot {
        /// 展平注入帧基座（序列化后即 `{ type:"runSnapshot", status, …, lastSeq }`）。
        #[serde(flatten)]
        snapshot: RunSnapshot,
    },
    /// 子代理委派开始（§3.4【M2】增补帧，10 号票）：告知前端本次委派的父
    /// 工具调用 id、子代理 id 与角色。帧序不变量：subStarted 必先于该子代理
    /// 的全部 subFrame，subDone 必收尾（§5.9）。
    SubStarted {
        call_id: String,
        sub_id: String,
        role: String,
    },
    /// 子代理帧透传（§3.4【M2】增补帧，10 号票）：主代理所派子代理 run 的
    /// 原始帧原样包装转发（子 run 的 seq 独立递增，与父 run 帧序互不干扰）；
    /// M2 禁止子代理再委派（Q13，subFrame 内不会嵌套 SubStarted）。
    SubFrame {
        sub_id: String,
        /// 子 run 的原始帧（Box 折叠枚举尺寸；serde 序列化内联展开）。
        frame: Box<ChatFrame>,
    },
    /// 子代理结构化汇报（§3.4【M2】增补帧，10 号票）：`<report>` 文本回父。
    SubReport { sub_id: String, report: String },
    /// 子代理终态（§3.4【M2】增补帧，10 号票）：四象限收口，必为该子代理
    /// 帧序列的最后一帧。
    SubDone { sub_id: String, outcome: SubOutcome },
}

/// 子代理终态四象限（§5.9，FR-07-02/09）：report=正常完成带汇报；
/// partial=预算耗尽（E_SUB_BUDGET）可能带部分汇报；no_report=异常结束
/// 无汇报；cancelled=被取消（stop_subagent / 父取消级联）。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SubOutcome {
    /// 正常完成且有结构化汇报。
    Report,
    /// 预算耗尽（可能有部分汇报）。
    Partial,
    /// 结束但无可用汇报（异常/空输出）。
    NoReport,
    /// 被取消。
    Cancelled,
}

/// run 结束原因。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// 自然完成（模型给出最终答复）。
    Done,
    /// 出错终止。
    Error,
    /// 用户取消。
    Cancelled,
}

/// §3.2 RunSnapshot 的运行状态（本阶段实装恒为 Running——命令面无活跃 run
/// 时返回 null 而非 idle 快照，Idle 变体按 §3.2 契约保留）。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RunSnapshotStatus {
    /// 空闲（无活跃 run；按 §3.3 #20 幂等语义由 null 承载，保留契约变体）。
    Idle,
    /// 运行中。
    Running,
}

/// §3.2 RunSnapshot（session_run_snapshot 返回与 runSnapshot 摘要帧负载，
/// FR-03-08 对账用，05 号票实装）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RunSnapshot {
    /// 运行状态（命令面实装恒为 running，见 [`RunSnapshotStatus`]）。
    pub status: RunSnapshotStatus,
    /// 本次 run 的唯一标识（§3.2 可选字段——idle 形态省略）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub run_id: Option<String>,
    /// run 开始时刻（Unix 纪元毫秒）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub started_at: Option<i64>,
    /// 当前轮次编号。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub turn: Option<u32>,
    /// 已执行的工具调用次数。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub tool_count: Option<u32>,
    /// 本 run 的 token 用量累计（帧面 usage 口径）。
    pub usage: UsageTotal,
    /// 已发出的最大帧序号（前端对账基准）。
    pub last_seq: u64,
}

/// 帧错误面（§3.4 `error?: { code, message }`，§3.6 错误码注册表）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct FrameError {
    /// 错误码（如 E_CANCELLED、E_INTERNAL）。
    pub code: String,
    /// 人类可读的错误说明。
    pub message: String,
}

/// FrameError 的构造实现。
impl FrameError {
    /// 用错误码与消息构造。
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

/// 帧出口：生产实现发 tauri `ipc::Channel<ChatFrame>`（宿主层适配，出口填
/// seq + ts），测试实现收进 Vec。抽成 trait 让帧管线（节流、主循环）不
/// 依赖 tauri 运行时。
pub trait FrameSink: Send {
    /// 发出一帧（由实现负责填 seq/ts 并送达目的地）。
    fn send(&mut self, frame: ChatFrame);
}

/// 已序列化帧的下发端（§5.8 FrameHub 的续流/重放出口，05 号票）：收到的是
/// 已填 seq/ts 的帧（序号由 FrameHub 统一递增）。生产适配器为 tauri
/// `Channel`，测试用 [`SeqCollectingSink`]——抽成 trait 使 FrameHub 不依赖
/// tauri 运行时。
pub trait SequencedSink: Send {
    /// 下发一帧（原样转发已序列化帧；实现不改动 seq/ts）。
    fn send_seq(&mut self, frame: SequencedFrame);
}

/// 已序列化帧的收集器（测试用）：按到达顺序保存全部帧（含 seq），供
/// `FrameHub` 下发端注入与重放断言。
#[derive(Default)]
pub struct SeqCollectingSink {
    /// 按到达顺序收集的帧。
    pub frames: std::sync::Mutex<Vec<SequencedFrame>>,
}

/// 收集器对已序列化帧下发端的实现（测试用；发送失败面不存在，直接入列）。
impl SequencedSink for SeqCollectingSink {
    fn send_seq(&mut self, frame: SequencedFrame) {
        self.frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(frame);
    }
}

/// SeqCollectingSink 的只读快照（按 seq 升序克隆，测试断言用）。
impl SeqCollectingSink {
    /// 克隆当前收集的全部帧。
    pub fn snapshot(&self) -> Vec<SequencedFrame> {
        self.frames
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// 带序号与出口时刻的帧（序列化后即 §3.4 的 `{ seq, type, ts?, ...}` 基座）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SequencedFrame {
    /// 帧序号（出口递增，与发送顺序一致）。
    pub seq: u64,
    /// Rust 出口时刻的 Unix 毫秒（可选；测试注入的出口可缺省）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub ts: Option<i64>,
    /// 帧本体（序列化时展平到基座上）。
    #[serde(flatten)]
    pub frame: ChatFrame,
}

/// 收集器实现：按到达顺序保存全部帧（含 seq），供测试断言。
#[derive(Default)]
pub struct CollectingSink {
    /// 按到达顺序收集的帧。
    pub frames: Vec<SequencedFrame>,
}

/// 收集器对帧出口接口的实现（测试用）。
impl FrameSink for CollectingSink {
    fn send(&mut self, frame: ChatFrame) {
        // seq 由出口递增，保证「seq 严格递增且与发送顺序一致」（§3.4 排序规则）
        let seq = self.frames.len() as u64;
        self.frames.push(SequencedFrame {
            seq,
            ts: None,
            frame,
        });
    }
}

/// 64 毫秒节流合并器（§3.4 节流语义，经早期原型实测确定）：
/// "首个增量立即出帧，其后相邻两次出帧间隔不小于窗口时长"。
///
/// - 合并语义：窗口（64ms）内到达的同型增量，在下次出帧时各自并成一帧；
/// - 节流语义：`should_flush()` 以真实时钟保证相邻出帧间隔不小于 64ms——
///   首个增量到达即出帧（优先降低 TTFT，即 Time To First Token，用户从
///   发问到看见第一个字的延迟），之后无论事件多密集至多 64ms 一帧，
///   打字机节奏稳定且与驱动方式（外部定时器或推送时检查）无关；
/// - 非增量帧（toolStarted/toolResult 等）不经节流器直通，保证 toolResult
///   必晚于同 callId 的全部 toolArgs（§3.4 排序规则）。
#[derive(Default)]
pub struct DeltaThrottler {
    /// 待出帧的正文文本增量（按轮次分组累积）。
    text: BTreeMap<u32, String>,
    /// 待出帧的推理文本增量（按轮次分组累积）。
    reasoning: BTreeMap<u32, String>,
    /// 待出帧的工具参数增量（按调用 id 分组累积）。
    tool_args: BTreeMap<String, String>,
    /// 上次出帧时刻；None = 从未出帧（首个增量立即出）。
    last_flush: Option<std::time::Instant>,
}

/// DeltaThrottler 的累积与出帧操作。
impl DeltaThrottler {
    /// 创建空窗口的节流器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 累积一段正文文本增量（turn 为所属轮次）。
    pub fn push_text(&mut self, turn: u32, delta: &str) {
        self.text.entry(turn).or_default().push_str(delta);
    }

    /// 累积一段推理文本增量（turn 为所属轮次）。
    pub fn push_reasoning(&mut self, turn: u32, delta: &str) {
        self.reasoning.entry(turn).or_default().push_str(delta);
    }

    /// 累积一段工具参数增量（call_id 为所属调用）。
    pub fn push_tool_args(&mut self, call_id: &str, chunk: &str) {
        self.tool_args
            .entry(call_id.to_owned())
            .or_default()
            .push_str(chunk);
    }

    /// 是否有未出帧的待发增量。
    pub fn has_pending(&self) -> bool {
        !self.text.is_empty() || !self.reasoning.is_empty() || !self.tool_args.is_empty()
    }

    /// 是否应出帧：距上次出帧已满节流窗口（或从未出帧且窗口里有增量——
    /// 首帧立即）。
    pub fn should_flush(&self) -> bool {
        match self.last_flush {
            None => self.has_pending(),
            Some(at) => at.elapsed() >= THROTTLE_INTERVAL,
        }
    }

    /// 把窗口内累积的增量合并成帧发给出口，并清空窗口。
    pub fn flush(&mut self, out: &mut dyn FrameSink) {
        // std::mem::take 拿走窗口内容（原 map 变空），逐组出帧
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
