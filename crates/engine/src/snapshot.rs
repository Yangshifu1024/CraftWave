//! 帧快照与 64 毫秒聚合器（gpui-rewrite 阶段 2.A「接口三件套」之二）。
//!
//! ## 来源（TS 资产平移，spec《TS 资产迁移表》）
//!
//! - `ui/src/chat/frames.ts` 的 `FrameBuffer`（帧累积：时间线段 + 工具表 +
//!   乱序丢弃 + usage/压缩/思考起止）与 `toParts`（快照 → 渲染 parts 的
//!   「工具为界合批」）——整体平移为 [`FrameAggregator`]；
//! - `ui/src/chat/runtime.ts` 的 `snapshots()`（64ms 节流快照迭代器：每
//!   64ms 产出一份全量快照，终态快照产出后收口；本地 abort 且终态帧未到
//!   时停止产出）——平移为 [`SnapshotTicker`]；
//! - `ui/src/stores/subagents.ts` 的 `applySubagentFrame`（子代理四帧的
//!   结构面分发）——并入 [`FrameAggregator::push`]（子代理内容不递归，
//!   只取结构状态，语义同旧实现）。
//!
//! ## 与旧实现的对应
//!
//! 旧栈：Rust 帧出口（`SequencedFrame`）→ Tauri Channel → 前端
//! `FrameBuffer.push` → 64ms 迭代全量 yield → assistant-ui 线程替换。
//! 新栈：Rust 帧出口（同一 `SequencedFrame` 类型，无跨语言边界）→
//! GPUI 壳把帧推入 [`FrameAggregator`] → [`SnapshotTicker`] 每 64ms 产出
//! [`ConversationSnapshot`] → GPUI Entity 触发重渲染。行为语义逐条对齐：
//!
//! - **乱序丢弃**：`seq` 必须严格递增，重复/倒退帧丢弃计数（帧序自愈）；
//! - **替换语义**：快照永远是全量累计态的克隆（不是增量）；
//! - **尾帧必达**：`runEnded` 帧到达后的下一个节流 tick 必产出含终态的
//!   快照（终态与拖尾增量在同一份快照里），随后迭代器收口；
//! - **abort 收束**：取消的首选路径是 `crate::api::run::cancel_run`（引擎
//!   补发 `runEnded(cancelled)`，ticker 正常产出终态后收口）；消费侧本地
//!   中止用 `FrameAggregator::abort`（ticker 下个 tick 不再产出、直接收口
//!   ——对齐旧实现 `signal.aborted && !snapshot.ended → break`）。
//!
//! 本模块类型是进程内渲染面（GPUI Entity 直接消费），不走 wire 序列化，
//! 不挂 ts-rs 导出；「历史消息列表」由壳从 `SessionTreeData` 活动路径构建
//! （`Rc` 克隆共享，渲染路径零深拷贝）。

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::backend::frames::{
    AuditState, ChatFrame, EndReason, FrameError, SequencedFrame, SubOutcome, UsageTotal,
};

/// 快照节流窗口（相邻两次快照产出的最小间隔；对齐旧实现 64ms）。
pub const SNAPSHOT_INTERVAL: Duration = Duration::from_millis(64);

/// 单次工具调用的累积态（镜像工具卡渲染面）。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallState {
    pub call_id: String,
    pub name: String,
    /// 参数 JSON 片段累积；半截 JSON 原样保留（卡片以「参数准备中」渲染）。
    pub args_json: String,
    /// 执行结果（None = 仍在执行/等待）。
    pub result: Option<ToolResultState>,
    /// 灰区命令审计状态（toolAudit 帧；None = 未经审计）。
    pub audit: Option<AuditState>,
    /// 审计依据（allow/ask/block）或失败原因（manual）。
    pub audit_reason: Option<String>,
}

/// 工具执行结果。
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResultState {
    pub ok: bool,
    pub ui_summary: String,
    /// 错误载荷（取消补发 E_CANCELLED 等；渲染规范 B6 按错误码分流）。
    pub error: Option<FrameError>,
}

/// 快照中的一段时间线内容（按帧到达顺序排列）。模型的真实输出是交错的
/// （思考 → 工具 → 旁白 → 工具 → 最终答案）：每类增量只在「上一段同为该
/// 类型」时续接，否则新开一段，快照即真实时间线（旧实现把文本/思考各拼
/// 一个字符串会丢失交错顺序——旧实现已废弃，本形态即修正案）。
#[derive(Debug, Clone, PartialEq)]
pub enum SnapshotSegment {
    Reasoning { text: String },
    Text { text: String },
    Tool { call_id: String },
}

/// run 起手信息（runStarted 帧）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunInfo {
    pub run_id: String,
    pub model: String,
}

/// run 终态（runEnded 帧）。
#[derive(Debug, Clone, PartialEq)]
pub struct RunEnd {
    pub reason: EndReason,
    pub duration_ms: u64,
    pub tool_count: u32,
    pub turn: u32,
    /// 错误终态载荷（reason=error 时承载）。
    pub error: Option<FrameError>,
}

/// 压缩完成信息（compactDone 帧；通知面透传，不进打字机内容面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionInfo {
    pub cut_entry_id: String,
    pub tokens_before: u64,
    pub tokens_after: u64,
}

/// 思考流的起止时刻（单调钟）：首帧 reasoningDelta 记起点，首帧
/// textDelta / toolStarted / runEnded 记终点——None = 本 run 没有思考流。
/// 思考行「思考中」扫光态（ended_at 为 None）与「持续了 N 秒」的数据面。
#[derive(Debug, Clone, PartialEq)]
pub struct ThinkingSpan {
    pub started_at: Instant,
    pub ended_at: Option<Instant>,
}

/// 子代理运行状态（结构面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubRunState {
    /// 运行中。
    Running,
    /// 终态四象限（report/partial/no_report/cancelled）。
    Done(SubOutcome),
}

/// 单个子代理的结构状态（Inspector 列表行与 subagent 工具卡共用；
/// 只取结构状态，子 run 的内容帧不递归——回看走
/// `crate::api::misc::load_subagent_history`，语义同旧实现）。
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentState {
    pub sub_id: String,
    /// 委派发起的父工具调用 id（工具卡按它关联）。
    pub call_id: String,
    pub role: String,
    pub state: SubRunState,
    /// 已推进的模型轮次（subFrame(stepStarted) 驱动）。
    pub turn: u32,
    /// `<report>` 结构化汇报全文（subReport 帧后可展开）。
    pub report: String,
}

/// 合并后的流式消息 part（`toParts` 语义产物）：以工具为界分段——两次
/// 工具之间的 reasoning 全部并入组头思考段、text 全部并入组正文段（按
/// 到达序拼接）。为什么合批：模型在两次工具之间会「想一下、说半句」反复
/// 多次，逐段展开会把一句话被思考行拦腰拆成几截；同组合批后就是一条思考
/// 行 + 一句连贯的话 + 工具行。顺序保持帧到达序，工具永远是分界。
#[derive(Debug, Clone, PartialEq)]
pub enum StreamPart {
    Text { text: String },
    Reasoning { text: String },
    ToolCall {
        call_id: String,
        name: String,
        args_json: String,
        result: Option<ToolResultState>,
        audit: Option<AuditState>,
        audit_reason: Option<String>,
    },
}

/// 流式中的 run 渲染态（面向渲染的不可变快照；每次产出都是全量克隆，
/// 替换语义）。
#[derive(Debug, Clone, PartialEq)]
pub struct StreamingMessage {
    /// runStarted 帧信息（帧到达前为 None）。
    pub run: Option<RunInfo>,
    /// runStarted 已见。
    pub started: bool,
    /// 终态（None = 仍在流式）。
    pub ended: Option<RunEnd>,
    /// 合并后的内容时间线（工具为界合批；打字机/思考行/工具卡的数据源）。
    pub parts: Vec<StreamPart>,
    /// 全部工具调用累积态（插入序 = toolStarted 到达序；工具簇卡渲染面，
    /// parts 里的 ToolCall 按 call_id 与此表关联）。
    pub tool_calls: Vec<ToolCallState>,
    /// usage 帧累计（运行内累计口径，非会话累计）。
    pub usage: UsageTotal,
    /// 最近一次 usage 帧的输出速率（tok/s；无则 None）。
    pub rate: Option<f64>,
    /// 本 run 发生的压缩（compactDone 帧；None = 未发生）。
    pub compaction: Option<CompactionInfo>,
    /// 思考流起止（None = 本 run 没有思考流）。
    pub thinking: Option<ThinkingSpan>,
    /// 子代理结构状态（subStarted/subFrame/subReport/subDone 驱动）。
    pub subagents: Vec<SubagentState>,
    /// 已见最大帧序号（None = 还没收过帧；对账基准）。
    pub max_seq: Option<u64>,
    /// 乱序/重复帧丢弃计数（对账协议健康度）。
    pub dropped_seq: u64,
}

/// 面向渲染的会话不可变快照：历史消息列表 + 流式中的 run 渲染态。
/// GPUI 壳拿它整体替换上一份（替换语义，不是在旧内容上追加）。
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationSnapshot {
    /// 历史消息列表（活动路径线性序；Rc 克隆共享，渲染路径零深拷贝）。
    /// 流式 run 结束后，壳把 run 的产物落盘重载，新条目即出现在这里。
    pub messages: Vec<Rc<crate::storage::MessageEntry>>,
    /// 流式中的 run（无活跃 run / run 已收口重载时为 None）。
    pub streaming: Option<StreamingMessage>,
}

/// 内部累积态（push 侧可变面；snapshot 时克隆成不可变快照）。
#[derive(Default)]
struct Accumulated {
    /// 内容时间线：text/reasoning 增量续接同类末段或新开段，工具按
    /// toolStarted 位置入列。
    segments: Vec<SnapshotSegment>,
    /// 工具调用表（插入序 = toolStarted 到达序）。
    tools: Vec<ToolCallState>,
    started: bool,
    run: Option<RunInfo>,
    ended: Option<RunEnd>,
    usage: UsageTotal,
    rate: Option<f64>,
    compaction: Option<CompactionInfo>,
    thinking_started_at: Option<Instant>,
    thinking_ended_at: Option<Instant>,
    subagents: Vec<SubagentState>,
    max_seq: Option<u64>,
    dropped_seq: u64,
}

/// 帧累积器：收 engine 帧 → 累积 → 按需产出全量快照（`FrameBuffer`
/// 平移）。`&self` 线程安全（帧来自 run 任务/GPUI 事件回调），累积态内部
/// 加锁；快照产出是纯读（克隆）。
#[derive(Default)]
pub struct FrameAggregator {
    state: Mutex<Accumulated>,
    /// 消费侧本地中止标志（abort 路径收束用）。
    aborted: AtomicBool,
}

impl FrameAggregator {
    /// 创建空累积器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 收一帧并入累积态（`SequencedFrame` 形态，帧序自愈语义）：
    /// seq 必须 > 已见最大序号，重复或倒退的帧直接丢弃（后端一次只发
    /// 一个 run 的帧，序号单调递增）。
    pub fn push(&self, frame: SequencedFrame) {
        self.push_seq(frame.seq, frame.frame);
    }

    /// `push` 的便捷形态：调用方已拆开 seq 与帧本体时使用。
    pub fn push_seq(&self, seq: u64, frame: ChatFrame) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 帧序自愈：seq 必须 > 已见最大，否则丢弃
        if let Some(max) = state.max_seq
            && seq <= max
        {
            state.dropped_seq += 1;
            return;
        }
        state.max_seq = Some(seq);
        state.apply(frame);
    }

    /// 纯 run 视图快照（全量克隆；`ConversationSnapshot.streaming` 的形态）。
    pub fn snapshot_streaming(&self) -> Option<StreamingMessage> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.started && state.max_seq.is_none() {
            return None;
        }
        let tool_calls = state.tools.clone();
        Some(StreamingMessage {
            run: state.run.clone(),
            started: state.started,
            ended: state.ended.clone(),
            parts: to_parts(&state.segments, &tool_calls),
            tool_calls,
            usage: state.usage,
            rate: state.rate,
            compaction: state.compaction.clone(),
            thinking: state.thinking_span(),
            subagents: state.subagents.clone(),
            max_seq: state.max_seq,
            dropped_seq: state.dropped_seq,
        })
    }

    /// 面向渲染的完整会话快照：历史消息 + 流式态（替换语义的全量克隆）。
    pub fn conversation_snapshot(
        &self,
        history: &[Rc<crate::storage::MessageEntry>],
    ) -> ConversationSnapshot {
        ConversationSnapshot {
            messages: history.to_vec(),
            streaming: self.snapshot_streaming(),
        }
    }

    /// 终态帧已见（runEnded 已到达累积器）。快照产出侧据此收口。
    pub fn is_terminal(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .ended
            .is_some()
    }

    /// 消费侧本地中止（abort 桥）：置位后 [`SnapshotTicker`] 不再产出
    /// （终态以 runEnded 帧为权威——首选取消路径是
    /// `crate::api::run::cancel_run`，让引擎补发终态帧后自然收口）。
    pub fn abort(&self) {
        self.aborted.store(true, Ordering::SeqCst);
    }

    /// 本地中止标志。
    pub fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::SeqCst)
    }

    /// 产出 64ms 节流快照迭代器（`runtime.ts` 的 `snapshots()` 平移）。
    /// `history` 由壳从会话树活动路径构建（`Rc` 共享），快照原样携带。
    pub fn ticker(
        self: &Arc<Self>,
        history: Rc<Vec<Rc<crate::storage::MessageEntry>>>,
    ) -> SnapshotTicker {
        SnapshotTicker {
            aggregator: Arc::clone(self),
            history,
            terminal_emitted: false,
        }
    }
}

/// 累积态的帧分发（`FrameBuffer.push` 的 switch 全分支平移 + Rust 帧集
/// 扩展帧；`now()` 用单调钟——思考起止只用于相对时长展示）。
impl Accumulated {
    fn thinking_span(&self) -> Option<ThinkingSpan> {
        Some(ThinkingSpan {
            started_at: self.thinking_started_at?,
            ended_at: self.thinking_ended_at,
        })
    }

    /// 首个正文帧/工具帧/runEnded 收口思考流（帧序：思考 → 工具/正文）。
    fn seal_thinking(&mut self) {
        if self.thinking_started_at.is_some() && self.thinking_ended_at.is_none() {
            self.thinking_ended_at = Some(Instant::now());
        }
    }

    /// 增量并入时间线：末段同为该类型则续接（同一发言块不拆段），否则
    /// 新开一段。空 delta 直接忽略，避免产出空段。
    fn append_segment(&mut self, kind: SegmentKind, delta: &str) {
        if delta.is_empty() {
            return;
        }
        match (kind, self.segments.last_mut()) {
            (SegmentKind::Text, Some(SnapshotSegment::Text { text })) => text.push_str(delta),
            (SegmentKind::Reasoning, Some(SnapshotSegment::Reasoning { text })) => {
                text.push_str(delta)
            }
            (kind, _) => self.segments.push(match kind {
                SegmentKind::Text => SnapshotSegment::Text {
                    text: delta.to_owned(),
                },
                SegmentKind::Reasoning => SnapshotSegment::Reasoning {
                    text: delta.to_owned(),
                },
            }),
        }
    }

    fn apply(&mut self, frame: ChatFrame) {
        match frame {
            ChatFrame::RunStarted { run_id, model } => {
                self.started = true;
                self.run = Some(RunInfo { run_id, model });
            }
            ChatFrame::TextDelta { delta, .. } => {
                self.append_segment(SegmentKind::Text, &delta);
                self.seal_thinking();
            }
            ChatFrame::ReasoningDelta { delta, .. } => {
                self.append_segment(SegmentKind::Reasoning, &delta);
                // 首帧思考 = 思考流起点
                if self.thinking_started_at.is_none() {
                    self.thinking_started_at = Some(Instant::now());
                }
            }
            ChatFrame::ToolStarted { call_id, name } => {
                // 帧序保证 toolStarted 一次；防御性查重（同 id 不重复入列）
                if !self.tools.iter().any(|t| t.call_id == call_id) {
                    self.tools.push(ToolCallState {
                        call_id: call_id.clone(),
                        name,
                        args_json: String::new(),
                        result: None,
                        audit: None,
                        audit_reason: None,
                    });
                    self.segments.push(SnapshotSegment::Tool { call_id });
                }
                self.seal_thinking();
            }
            ChatFrame::ToolArgs { call_id, chunk } => {
                if let Some(tool) = self.tools.iter_mut().find(|t| t.call_id == call_id) {
                    tool.args_json.push_str(&chunk);
                }
            }
            ChatFrame::ToolResult {
                call_id,
                ok,
                ui_summary,
                error,
            } => {
                if let Some(tool) = self.tools.iter_mut().find(|t| t.call_id == call_id) {
                    tool.result = Some(ToolResultState {
                        ok,
                        ui_summary,
                        error,
                    });
                }
            }
            ChatFrame::Usage { usage, rate } => {
                self.usage = usage;
                // rate 缺省保留上一次值（帧面可选字段）
                if rate.is_some() {
                    self.rate = rate;
                }
            }
            ChatFrame::RunEnded {
                reason,
                duration_ms,
                tool_count,
                turn,
                error,
            } => {
                self.ended = Some(RunEnd {
                    reason,
                    duration_ms,
                    tool_count,
                    turn,
                    error,
                });
                // 只有思考没有正文/工具的 run：思考流在 runEnded 收口
                self.seal_thinking();
            }
            ChatFrame::CompactDone {
                cut_entry_id,
                tokens_before,
                tokens_after,
            } => {
                // 通知面透传（不进打字机内容面，指示/审计用）
                self.compaction = Some(CompactionInfo {
                    cut_entry_id,
                    tokens_before,
                    tokens_after,
                });
            }
            // runState 摘要帧：attach 重放的幂等对账辅助，不进内容面。
            ChatFrame::RunSnapshot { .. } => {}
            ChatFrame::ToolAudit {
                call_id,
                state,
                reason,
            } => {
                if let Some(tool) = self.tools.iter_mut().find(|t| t.call_id == call_id) {
                    tool.audit = Some(state);
                    if reason.is_some() {
                        tool.audit_reason = reason;
                    }
                }
            }
            // 子代理四帧：只取结构状态（语义同旧 applySubagentFrame——
            // subFrame 内部内容不递归，回看走历史命令）。
            ChatFrame::SubStarted {
                call_id,
                sub_id,
                role,
            } => {
                if let Some(sub) = self.subagents.iter_mut().find(|s| s.sub_id == sub_id) {
                    sub.call_id = call_id;
                    sub.role = role;
                } else {
                    self.subagents.push(SubagentState {
                        sub_id,
                        call_id,
                        role,
                        state: SubRunState::Running,
                        turn: 0,
                        report: String::new(),
                    });
                }
            }
            ChatFrame::SubFrame { sub_id, frame } => {
                if let ChatFrame::StepStarted { turn } = *frame
                    && let Some(sub) = self.subagents.iter_mut().find(|s| s.sub_id == sub_id)
                {
                    sub.turn = turn;
                }
            }
            ChatFrame::SubReport { sub_id, report } => {
                if let Some(sub) = self.subagents.iter_mut().find(|s| s.sub_id == sub_id) {
                    sub.report = report;
                }
            }
            ChatFrame::SubDone { sub_id, outcome } => {
                if let Some(sub) = self.subagents.iter_mut().find(|s| s.sub_id == sub_id) {
                    sub.state = SubRunState::Done(outcome);
                }
            }
            // turn 起手帧：无渲染面（旧实现同样忽略）。
            ChatFrame::StepStarted { .. } => {}
        }
    }
}

/// `append_segment` 的段类型（Text / Reasoning 二选一的类型安全载体）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SegmentKind {
    Text,
    Reasoning,
}

/// 快照时间线 → 渲染 parts（`frames.ts toParts` 平移）：以工具为界把
/// 时间线分段——两次工具之间的 reasoning 全部并入组头思考段、text 全部
/// 并入组正文段（按到达序直接拼接），工具卡按 callId 关联调用状态。
///
/// `reasoning` 的运行状态与时长不进 part（渲染层从
/// `StreamingMessage.thinking` 统一取——run 级共享面，旧实现的
/// `reasoningState` 同款语义）。
pub fn to_parts(segments: &[SnapshotSegment], tool_calls: &[ToolCallState]) -> Vec<StreamPart> {
    let tool_by_id: BTreeMap<&str, &ToolCallState> =
        tool_calls.iter().map(|t| (t.call_id.as_str(), t)).collect();
    let mut parts: Vec<StreamPart> = Vec::new();
    let mut group_reasoning: Vec<&str> = Vec::new();
    let mut group_text: Vec<&str> = Vec::new();
    let flush_group = |parts: &mut Vec<StreamPart>,
                       group_reasoning: &mut Vec<&str>,
                       group_text: &mut Vec<&str>| {
        let reasoning_text = group_reasoning.join("\n\n");
        if !reasoning_text.is_empty() {
            parts.push(StreamPart::Reasoning {
                text: reasoning_text,
            });
        }
        let text = group_text.concat();
        if !text.is_empty() {
            parts.push(StreamPart::Text { text });
        }
        group_reasoning.clear();
        group_text.clear();
    };
    for seg in segments {
        match seg {
            SnapshotSegment::Reasoning { text } => {
                if !text.is_empty() {
                    group_reasoning.push(text);
                }
            }
            SnapshotSegment::Text { text } => {
                if !text.is_empty() {
                    group_text.push(text);
                }
            }
            SnapshotSegment::Tool { call_id } => {
                flush_group(&mut parts, &mut group_reasoning, &mut group_text);
                if let Some(tool) = tool_by_id.get(call_id.as_str()) {
                    parts.push(StreamPart::ToolCall {
                        call_id: tool.call_id.clone(),
                        name: tool.name.clone(),
                        args_json: tool.args_json.clone(),
                        result: tool.result.clone(),
                        audit: tool.audit,
                        audit_reason: tool.audit_reason.clone(),
                    });
                }
            }
        }
    }
    flush_group(&mut parts, &mut group_reasoning, &mut group_text);
    parts
}

/// 参数是否已是完整 JSON 对象（「参数准备中」→「运行中」的判定）：
/// 解析成功且为对象 = ready（`FrameBuffer.argsReady` 平移——TS 侧
/// `typeof parsed === "object" && parsed !== null` 同口径）。
pub fn args_ready(args_json: &str) -> bool {
    serde_json::from_str::<Value>(args_json)
        .map(|v| v.is_object())
        .unwrap_or(false)
}

/// 64ms 节流快照迭代器（`runtime.ts` 的 `snapshots()` 平移）。
///
/// 每次 [`Self::next`] 睡满一个节流窗口后产出当前全量快照；终态快照
/// （含 runEnded）产出后返回 None 收口——**尾帧必达**：终态帧与它之前的
/// 全部拖尾增量必然出现在同一份（最后一份）快照里。本地 abort 且终态帧
/// 未到 → 不产出、直接收口（对齐旧实现 break 语义；取消的首选路径仍是
/// `cancel_run` 让引擎补发终态帧）。
pub struct SnapshotTicker {
    aggregator: Arc<FrameAggregator>,
    history: Rc<Vec<Rc<crate::storage::MessageEntry>>>,
    terminal_emitted: bool,
}

impl SnapshotTicker {
    /// 推进一个节流窗口：产出一份快照，或流收口后返回 None。
    pub async fn next(&mut self) -> Option<ConversationSnapshot> {
        if self.terminal_emitted {
            return None;
        }
        tokio::time::sleep(SNAPSHOT_INTERVAL).await;
        let snap = self.aggregator.conversation_snapshot(&self.history);
        if snap.streaming.as_ref().is_some_and(|s| s.ended.is_some()) {
            // 终态快照产出一次后收口（下一调返回 None）
            self.terminal_emitted = true;
            return Some(snap);
        }
        if self.aggregator.is_aborted() {
            // 本地中止且终态帧未到：停止产出（终态以 runEnded 帧为权威）
            return None;
        }
        Some(snap)
    }
}

// 测试挂载：TS 行为断言迁移（仅 cargo test 时编译）。
#[cfg(test)]
#[path = "snapshot_test.rs"]
mod snapshot_test;
