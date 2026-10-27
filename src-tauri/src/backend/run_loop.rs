//! AgentRun 步进 RunLoop（技术设计文档 §5.2）+ CallTools 审批挂起机制层 +
//! 取消传播收齐语义（§5.3）。
//!
//! 产品化自 S2/S3/S5 spike（`spike/turn.rs` 的手动循环与 `spike/cancel.rs` 的
//! 取消版），增量：runStarted/stepStarted/usage 帧生产（§3.4）、64ms 节流内聚
//! （帧生产是 backend 职责）、callId 同源反查（§3.4）、门控挂起点（§5.2「审批
//! 挂起 = CallTools 步骤上把整只 run 序列化挂起」的机制层——审批 UI 接线属
//! 工具系统后续票）。
//!
//! 错误语义：`PromptError`（rig 侧失败）冒泡给调用方（RunManager 的 run 任务），
//! 由其发 `runEnded(error)` 终态帧——保证任何退出路径前端都有终态。

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
/// [`super::exec::ToolsetExecutor`]，未来 command 工具在实现内部以 `token`
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

/// 工具门控判定（§5.4 步骤①–④的宿主挂点）。本票只钉机制层：
/// `Allow` 直执行；`Suspend` 表示需审批/询问——RunLoop 把整只 run 序列化挂起
/// 并返回 [`RunExit::Suspended`]，宿主拿到审批应答后反序列化恢复继续
/// （跨重启恢复由序列化面天然支持，S3 实证）。权限档判定与审批门策略属
/// 工具系统后续票。
pub trait ToolGate: Send + Sync {
    fn judge(&self, name: &str, args_json: &str) -> GateVerdict;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateVerdict {
    Allow,
    Suspend,
}

/// 全放行门（本票 host 装配的默认门；审批门策略接入前的占位实现）。
pub struct AllowAllGate;

impl ToolGate for AllowAllGate {
    fn judge(&self, _name: &str, _args_json: &str) -> GateVerdict {
        GateVerdict::Allow
    }
}

/// 手动循环的退出态。
#[derive(Debug)]
pub enum RunExit {
    /// 自然终态（§5.3 Streaming → Done）。
    Done(Box<PromptResponse>),
    /// 取消终态：工具结果已收齐并补标记，runEnded(cancelled) 已发出。
    Cancelled,
    /// 审批挂起：整只 run 已 `serde_json` 序列化（恢复 = [`resume_run`] 后
    /// 重新 `drive_run`，`next_step` 会重新给出未应答的 calls）。
    Suspended { run_json: String },
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
/// - `runStarted` 起手一帧；
/// - `CallModel`：`stepStarted` → 流式调用（增量经 64ms 节流器出帧）→ 收口
///   `usage` 帧（§10.5 累计口径，rate = 本轮输出 tok/s）→ 收尾 flush；
/// - `CallTools`：门控（Suspend → 序列化挂起）→ 逐个执行 → `toolResult` 帧
///   （callId 经桥接映射与流式三帧同源）；
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
) -> Result<RunExit, PromptError> {
    let started_at = Instant::now();
    let mut throttler = DeltaThrottler::new();
    let mut total_usage = UsageTotal::default();
    let mut last_turn = 1u32;
    let mut tool_count = 0u32;
    // 当前 turn 的流事件桥（CallModel 每拍重置；CallTools 以其映射反查帧面 callId）
    let mut bridge = RigEventBridge::new();

    sink.send(ChatFrame::RunStarted {
        run_id: run_id.to_owned(),
        model: model_name.to_owned(),
    });

    let mut run = run;
    loop {
        match run.next_step()? {
            AgentRunStep::CallModel {
                prompt,
                history,
                turn,
            } => {
                last_turn = turn as u32;
                sink.send(ChatFrame::StepStarted { turn: turn as u32 });
                bridge = RigEventBridge::new();
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
                        return Ok(finish_cancelled(sink, started_at, tool_count, last_turn));
                    }
                    StreamExit::Completed => {
                        if let Some(usage) = outcome.usage {
                            let total = usage_total(&usage);
                            let elapsed_s = turn_started.elapsed().as_secs_f64();
                            let rate = (total.output > 0 && elapsed_s > 0.0)
                                .then(|| total.output as f64 / elapsed_s);
                            total_usage.accumulate(&total);
                            sink.send(ChatFrame::Usage {
                                usage: total_usage,
                                rate,
                            });
                        }
                    }
                }
            }
            AgentRunStep::CallTools { calls } => {
                let mut answers = Vec::new();
                let mut cancel_seen = false;
                for call in calls {
                    let (call_id, name, args_json) = pending_parts(&call);
                    // 门控挂起点（§5.2）：CallTools 上整只 run 序列化挂起。
                    // 此时批内所有调用都未执行，与 rig ExecutingTools 状态自洽，
                    // 恢复后 next_step 重新给出全部 calls。
                    if gate.judge(&name, &args_json) == GateVerdict::Suspend {
                        let run_json = suspend_run(&run).map_err(serde_json_error_to_prompt)?;
                        return Ok(RunExit::Suspended { run_json });
                    }
                    if cancel_seen || token.is_cancelled() {
                        // 收齐语义：批内未执行的工具也补 E_CANCELLED，不留悬空调用
                        sink.send(cancelled_result_frame(
                            &bridge.frame_call_id_for_model(&call_id),
                            &format!("{E_CANCELLED}: 取消：工具未执行"),
                            false,
                        ));
                        tool_count += 1;
                        continue;
                    }
                    tool_count += 1;
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
                    return Ok(finish_cancelled(sink, started_at, tool_count, last_turn));
                }
                run.answer_all(answers)?;
            }
            AgentRunStep::Done(response) => {
                sink.send(ChatFrame::RunEnded {
                    reason: EndReason::Done,
                    duration_ms: started_at.elapsed().as_millis() as u64,
                    tool_count,
                    turn: last_turn,
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
fn finish_cancelled(
    sink: &mut dyn FrameSink,
    started_at: Instant,
    tool_count: u32,
    turn: u32,
) -> RunExit {
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Cancelled,
        duration_ms: started_at.elapsed().as_millis() as u64,
        tool_count,
        turn,
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
