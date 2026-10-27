//! AgentRun 步进 RunLoop（技术设计文档 §5.2）+ CallTools 审批挂起机制层 +
//! 取消传播收齐语义（§5.3）。
//!
//! 产品化自 S2/S3/S5 spike（`spike/turn.rs` 的手动循环与 `spike/cancel.rs` 的
//! 取消版），13 号票落地帧生产/节流/callId 同源/挂起机制层；14 号票增量：
//! - 门控批级预判（§5.4 ①–④）：CallTools 先对整批判定再执行——首个 Suspend
//!   即挂起（**挂起时批内零执行**，恢复后 `next_step` 原样重放全部 calls，
//!   不存在重复副作用）；Blocked 回喂错误结果帧（不中断 run）；
//! - [`DriveState`]：跨挂起/恢复延续计时、usage 累计、工具计数与 callId 映射
//!   （恢复不重发 `runStarted`、usage 不回退、toolResult 与挂起前
//!   toolStarted/toolArgs 帧同源）。
//!
//! 错误语义：`PromptError`（rig 侧失败）冒泡给调用方（RunManager 的 run 任务），
//! 由其发 `runEnded(error)` 终态帧——保证任何退出路径前端都有终态。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use rig::run::response::PromptResponse;
use rig::run::{AgentRun, AgentRunStep, PendingToolCall, PromptError};
use tokio_util::sync::CancellationToken;

use super::frames::{ChatFrame, DeltaThrottler, EndReason, FrameError, FrameSink, UsageTotal};
use super::rig_bridge::RigEventBridge;
use super::turn::{StreamExit, TurnDriver, usage_total};

/// 被取消工具结果补挂的错误码（§3.4 toolResult.error.code / §3.6 错误码注册表）。
pub const E_CANCELLED: &str = "E_CANCELLED";

/// 工具执行产物（帧面 + 模型回喂面的双通道裁剪；§8.2 双通道的 RunLoop 侧形状）。
#[derive(Debug, Clone, PartialEq)]
pub enum ToolExecution {
    /// 执行完毕（成功或工具级失败——失败也是收齐的结果，回喂模型促其纠正）。
    Completed {
        ok: bool,
        ui_summary: String,
        error: Option<FrameError>,
        /// 模型通道 compact 文本（§8.2：成功序列化 data；失败 code+message）。
        model_text: String,
    },
    /// 取消终止：`executed=false` 为批内未执行（补标记不留悬空调用），
    /// `executed=true` 为执行中被收割（§5.3：已产生副作用的文件写不回滚）。
    Cancelled { executed: bool, message: String },
}

/// 取消可感知的工具执行器（§5.4 门控与执行的 backend 侧契约；生产装配见
/// [`super::exec::ToolsetExecutor`]，command 工具在实现内部以 `token`
/// select! 收割进程组——原语见 `tools::process`）。
pub trait CancellableExecutor: Send + Sync {
    fn execute<'a>(
        &'a self,
        call_id: &'a str,
        name: &'a str,
        args_json: &'a str,
        token: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>>;
}

/// 工具门控判定（§5.4 步骤①–④）。`Allow` 直执行；`Suspend` 表示需审批/
/// 围栏确认——RunLoop 把整只 run 序列化挂起并返回 [`RunExit::Suspended`]，
/// 宿主拿到审批应答后反序列化恢复继续（跨重启恢复由序列化面天然支持，
/// S3 实证）；`Blocked` 硬拦（围栏黑名单/权限过滤/计划门）——回喂错误
/// 结果帧，不中断 run。生产策略装配见 [`super::gate::ApprovalGate`]。
pub trait ToolGate: Send + Sync {
    fn judge(&self, name: &str, args_json: &str) -> GateVerdict;
}

#[derive(Debug, Clone, PartialEq)]
pub enum GateVerdict {
    Allow,
    /// 挂起等待审批（`fence_reason`：围栏 Confirm 的写信号原因，AskPayload 用）。
    Suspend {
        fence_reason: Option<String>,
    },
    /// 硬拦：结果帧回喂（code ∈ {E_FENCE_BLOCKED, E_TOOL_BLOCKED, E_PLAN_STALE}）。
    Blocked {
        code: &'static str,
        message: String,
    },
}

/// 全放行门（测试装配；生产审批门为 [`super::gate::ApprovalGate`]）。
pub struct AllowAllGate;

impl ToolGate for AllowAllGate {
    fn judge(&self, _name: &str, _args_json: &str) -> GateVerdict {
        GateVerdict::Allow
    }
}

/// 挂起点信息：触发挂起的调用（宿主构造 AskPayload / 恢复后按签名放行用）。
#[derive(Debug, Clone)]
pub struct PendingCall {
    /// 模型侧 call id（approve 登记签名与 spill 文件名用）。
    pub model_call_id: String,
    pub name: String,
    pub args_json: String,
    /// 围栏 Confirm 的写信号原因（None = 纯权限矩阵审批）。
    pub fence_reason: Option<String>,
}

/// 跨挂起/恢复延续的驱动状态（14 号票：恢复不重发 runStarted、usage 与
/// 工具计数不回退、callId 映射同源）。
#[derive(Debug, Clone, Default)]
pub struct DriveState {
    pub started_at: Option<Instant>,
    pub total_usage: UsageTotal,
    pub tool_count: u32,
    pub last_turn: u32,
    /// 模型 call id → 桥接帧面 id（挂起时带出、恢复时播种新 bridge）。
    pub model_call_ids: HashMap<String, String>,
}

/// 手动循环的退出态。
#[derive(Debug)]
pub enum RunExit {
    /// 自然终态（§5.3 Streaming → Done）。
    Done(Box<PromptResponse>),
    /// 取消终态：工具结果已收齐并补标记，runEnded(cancelled) 已发出。
    Cancelled,
    /// 审批挂起：整只 run 已 `serde_json` 序列化（恢复 = [`resume_run`] 后
    /// 重新 `drive_run`，`next_step` 会重新给出未应答的 calls；挂起时批内
    /// 零执行，恢复重放无重复副作用）。Box 收纳大负载（clippy large_enum_variant）。
    Suspended(Box<SuspendedInfo>),
}

/// 挂起信息（[`RunExit::Suspended`] 载体）。
#[derive(Debug)]
pub struct SuspendedInfo {
    pub run_json: String,
    /// 触发挂起的调用（AskPayload 构造与恢复放行签名）。
    pub pending: PendingCall,
    pub state: DriveState,
}

/// 审批挂起机制层：把整只 run 序列化为 JSON（§5.2/S3 形态）。
pub fn suspend_run(run: &AgentRun) -> serde_json::Result<String> {
    serde_json::to_string(run)
}

/// 审批挂起恢复：从 JSON 反序列化回 run（rig 信封含 format 版本校验，
/// 跨版本拒绝加载）。
pub fn resume_run(run_json: &str) -> serde_json::Result<AgentRun> {
    serde_json::from_str(run_json)
}

/// RunLoop 主驱动（§5.2 时序的「每 turn」循环 + §3.4 帧生产）：
///
/// - 首次调用（`resume = None`）发 `runStarted` 起手一帧；恢复调用延续
///   [`DriveState`]（不重发 runStarted、usage/计数延续）；
/// - `CallModel`：`stepStarted` → 流式调用（增量经 64ms 节流器出帧）→ 收口
///   `usage` 帧（§10.5 累计口径，rate = 本轮输出 tok/s）→ 收尾 flush；
/// - `CallTools`：**批级门控预判**（§5.4 ①–④）——首个 `Suspend` 即把整只
///   run 序列化挂起（批内零执行）；`Blocked` 回喂错误结果；其余逐个执行 →
///   `toolResult` 帧（callId 经桥接映射与流式三帧同源）；
/// - 取消（§5.3）：流中取消即收尾 flush + `runEnded(cancelled)`；工具批内
///   任一工具报取消 → 剩余工具补 `E_CANCELLED`、收齐后才发终态帧；
/// - `Done`：`runEnded(done)` 必为最后一帧。
#[allow(clippy::too_many_arguments)]
pub async fn drive_run(
    driver: &TurnDriver,
    run: AgentRun,
    run_id: &str,
    model_name: &str,
    token: CancellationToken,
    executor: Arc<dyn CancellableExecutor>,
    gate: Arc<dyn ToolGate>,
    sink: &mut dyn FrameSink,
    resume: Option<DriveState>,
) -> Result<RunExit, PromptError> {
    let mut state = resume.unwrap_or_default();
    if state.started_at.is_none() {
        state.started_at = Some(Instant::now());
        sink.send(ChatFrame::RunStarted {
            run_id: run_id.to_owned(),
            model: model_name.to_owned(),
        });
    }
    let started_at = state.started_at.expect("入口已置位");

    let mut throttler = DeltaThrottler::new();
    // 当前 turn 的流事件桥（CallModel 每拍重置；CallTools 以其映射反查帧面 callId；
    // 首拍播种挂起前映射——恢复后重放的挂起批 toolResult 与挂起前三帧同源）。
    let mut bridge = RigEventBridge::new();
    bridge.seed_model_call_ids(state.model_call_ids.clone());

    let mut run = run;
    loop {
        match run.next_step()? {
            AgentRunStep::CallModel {
                prompt,
                history,
                turn,
            } => {
                state.last_turn = turn as u32;
                sink.send(ChatFrame::StepStarted { turn: turn as u32 });
                let mut turn_bridge = RigEventBridge::new();
                turn_bridge.seed_model_call_ids(bridge.model_call_ids().clone());
                bridge = turn_bridge;
                let mut on_frame = |frame: ChatFrame| ingest_delta(&mut throttler, sink, &frame);
                let turn_started = Instant::now();
                let outcome = driver
                    .stream_turn_cancellable(
                        &mut run,
                        prompt,
                        history,
                        turn as u32,
                        &token,
                        &mut bridge,
                        &mut on_frame,
                    )
                    .await?;
                // turn 收尾 flush：窗口里的增量尾巴放行（取消路径也要收齐）
                throttler.flush(sink);
                match outcome.exit {
                    StreamExit::Cancelled => {
                        // 模型流已停：无工具进行中，直接取消终结
                        return Ok(finish_cancelled(sink, started_at, state));
                    }
                    StreamExit::Completed => {
                        if let Some(usage) = outcome.usage {
                            let total = usage_total(&usage);
                            let elapsed_s = turn_started.elapsed().as_secs_f64();
                            let rate = (total.output > 0 && elapsed_s > 0.0)
                                .then(|| total.output as f64 / elapsed_s);
                            state.total_usage.accumulate(&total);
                            sink.send(ChatFrame::Usage {
                                usage: state.total_usage,
                                rate,
                            });
                        }
                    }
                }
            }
            AgentRunStep::CallTools { calls } => {
                // —— 批级门控预判（§5.4 ①–④）：先全判后执行 ——
                // Suspend：批内零执行即挂起（恢复重放整批，无重复副作用）；
                // Blocked：硬拦结果帧回喂，不中断 run。
                let verdicts: Vec<GateVerdict> = calls
                    .iter()
                    .map(|call| {
                        let (_model_call_id, name, args_json) = pending_parts(call);
                        gate.judge(&name, &args_json)
                    })
                    .collect();
                if let Some(idx) = verdicts
                    .iter()
                    .position(|v| matches!(v, GateVerdict::Suspend { .. }))
                {
                    let (model_call_id, name, args_json) = pending_parts(&calls[idx]);
                    let GateVerdict::Suspend { fence_reason } = &verdicts[idx] else {
                        unreachable!("position 已确认 Suspend");
                    };
                    state.model_call_ids = bridge.model_call_ids().clone();
                    let run_json = suspend_run(&run).map_err(serde_json_error_to_prompt)?;
                    return Ok(RunExit::Suspended(Box::new(SuspendedInfo {
                        run_json,
                        pending: PendingCall {
                            model_call_id,
                            name,
                            args_json,
                            fence_reason: fence_reason.clone(),
                        },
                        state,
                    })));
                }

                let mut answers = Vec::new();
                let mut cancel_seen = false;
                for (call, verdict) in calls.into_iter().zip(verdicts) {
                    let (call_id, name, args_json) = pending_parts(&call);
                    if let GateVerdict::Blocked { code, message } = &verdict {
                        // 硬拦（§8.7/§8.5/§8.6）：错误结果回喂，run 继续
                        state.tool_count += 1;
                        sink.send(ChatFrame::ToolResult {
                            call_id: bridge.frame_call_id_for_model(&call_id),
                            ok: false,
                            ui_summary: format!("已拦截：{name}"),
                            error: Some(FrameError::new(*code, message.clone())),
                        });
                        answers.push(super::turn::answer_one(call, format!("{code} {message}")));
                        continue;
                    }
                    if cancel_seen || token.is_cancelled() {
                        // 收齐语义：批内未执行的工具也补 E_CANCELLED，不留悬空调用
                        sink.send(cancelled_result_frame(
                            &bridge.frame_call_id_for_model(&call_id),
                            &format!("{E_CANCELLED}: 取消：工具未执行"),
                            false,
                        ));
                        state.tool_count += 1;
                        continue;
                    }
                    state.tool_count += 1;
                    match executor.execute(&call_id, &name, &args_json, &token).await {
                        ToolExecution::Completed {
                            ok,
                            ui_summary,
                            error,
                            model_text,
                        } => {
                            sink.send(ChatFrame::ToolResult {
                                call_id: bridge.frame_call_id_for_model(&call_id),
                                ok,
                                ui_summary,
                                error,
                            });
                            answers.push(super::turn::answer_one(call, model_text));
                        }
                        ToolExecution::Cancelled { executed, message } => {
                            cancel_seen = true;
                            let summary = if executed {
                                format!("{E_CANCELLED}: 取消：{message}")
                            } else {
                                format!("{E_CANCELLED}: 取消：工具未执行")
                            };
                            sink.send(cancelled_result_frame(
                                &bridge.frame_call_id_for_model(&call_id),
                                &summary,
                                executed,
                            ));
                        }
                    }
                }
                if cancel_seen {
                    // 收齐后才发终态帧（§5.3 不变量：取消中工具收齐 → runEnded(cancelled)）
                    return Ok(finish_cancelled(sink, started_at, state));
                }
                run.answer_all(answers)?;
            }
            AgentRunStep::Done(response) => {
                sink.send(ChatFrame::RunEnded {
                    reason: EndReason::Done,
                    duration_ms: started_at.elapsed().as_millis() as u64,
                    tool_count: state.tool_count,
                    turn: state.last_turn,
                    error: None,
                });
                return Ok(RunExit::Done(Box::new(response)));
            }
        }
    }
}

/// 流式增量的节流入口：三型增量进节流器（满窗即 flush，首帧立即——§3.4 节流
/// 语义），其余帧直通 sink（保证 toolResult 必晚于同 callId 全部 toolArgs）。
fn ingest_delta(throttler: &mut DeltaThrottler, sink: &mut dyn FrameSink, frame: &ChatFrame) {
    match frame {
        ChatFrame::TextDelta { turn, delta } => throttler.push_text(*turn, delta),
        ChatFrame::ReasoningDelta { turn, delta } => throttler.push_reasoning(*turn, delta),
        ChatFrame::ToolArgs { call_id, chunk } => throttler.push_tool_args(call_id, chunk),
        other => {
            sink.send(other.clone());
            return;
        }
    }
    if throttler.should_flush() {
        throttler.flush(sink);
    }
}

/// 发出取消终态帧并返回取消退出态（runEnded(cancelled) 必为最后一帧）。
fn finish_cancelled(sink: &mut dyn FrameSink, started_at: Instant, state: DriveState) -> RunExit {
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Cancelled,
        duration_ms: started_at.elapsed().as_millis() as u64,
        tool_count: state.tool_count,
        turn: state.last_turn,
        error: None,
    });
    RunExit::Cancelled
}

/// 被取消工具的 toolResult 帧：ok=false + error.code=E_CANCELLED（§3.4 error 面）。
fn cancelled_result_frame(call_id: &str, ui_summary: &str, executed: bool) -> ChatFrame {
    ChatFrame::ToolResult {
        call_id: call_id.to_owned(),
        ok: false,
        ui_summary: ui_summary.to_owned(),
        error: Some(FrameError::new(
            E_CANCELLED,
            if executed {
                "执行中被取消（已产生副作用不回滚）"
            } else {
                "取消：工具未执行"
            },
        )),
    }
}

/// run 序列化失败归入 PromptError（挂起机制的面内错误，正常不会发生）。
fn serde_json_error_to_prompt(e: serde_json::Error) -> PromptError {
    PromptError::from(rig::error::ProviderError::request(format!(
        "审批挂起序列化失败：{e}"
    )))
}

/// 从 pending call 提取 (模型 callId, 工具名, argsJson)。
fn pending_parts(call: &PendingToolCall) -> (String, String, String) {
    match call {
        PendingToolCall::Execute(exec) => (
            exec.id().to_string(),
            exec.name().to_string(),
            exec.arguments().to_string(),
        ),
        PendingToolCall::Malformed(malformed) => (
            malformed.id().to_string(),
            malformed.name().to_string(),
            malformed.raw_arguments().to_owned(),
        ),
        // #[non_exhaustive]：未来新增变体按未执行补取消标记
        other => (
            String::new(),
            String::new(),
            format!("unhandled pending call variant: {other:?}"),
        ),
    }
}
