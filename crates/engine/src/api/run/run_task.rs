//! run 任务主体（平移自旧壳 `host/commands/chat.rs` start_chat_core 的
//! spawn 闭包）：驱动循环 + 审批挂起恢复循环 + 灰区审计 + suggest 后台
//! 小调用 + 终态事件 + followUp 队列收尾分流。
//!
//! 依赖收进 [`RunTaskDeps`]（spawn 闭包不能借用 AppState——全部字段同源
//! 的共享句柄副本与克隆件，与旧壳逐字段克隆同一口径）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rig::completion::Message;
use rig::completion::request::ToolDefinition;
use rig::run::spec::RunSpec;

use crate::agent::RunHandle;
use crate::app_state::AppState;
use crate::agent::RunManager;
use crate::backend::ask::AuditNote;
use crate::backend::frames::{ChatFrame, FrameSink, UsageTotal};
use crate::backend::gate::ApprovalGate;
use crate::backend::run_loop::{DriveState, RunExit, SteerSource, drive_run, resume_run};
use crate::backend::turn::TurnDriver;
use crate::followup::{RunFailure, followup_disposition};
use crate::run_persistence::{PersistingSink, RunRecorder};
use crate::util::errors::{E_AUDIT_BLOCKED, E_INTERNAL, E_RUN_STALLED};
use crate::util::events::keys;

use super::approval::{
    AUDIT_TOTAL_TIMEOUT, ApprovalResolution, AuditFlow, audit_block_finish,
    audit_cancelled_finish, denied_finish, plan_scope_note_of, run_command_audit,
    wait_approval_with,
};
use super::compaction::{CompactionRuntimePolicy, maybe_compact_after_ask};
use super::delivery::spawn_followup_delivery;
use super::model::{ModelResolver, ModelSlot, RunTransport, RunWire};
use super::sinks::{EventfulSink, now_ms, record_aux_usage, send_error_ended};
use super::tasks::{drain_steer, spawn_suggest};

/// run 任务的依赖面（spawn 闭包捕获集的结构化收拢）：`app` 为
/// [`AppState::share_handles`] 的全字段同源副本（收尾的 followUp 自动投递
/// 要以完整状态面重新走装配入口）。
pub(crate) struct RunTaskDeps<W: RunWire, T: RunTransport<W>> {
    /// 共享句柄副本。
    pub app: AppState,
    /// 活跃 run 注册表（收尾时摘除自身）。
    pub runs: RunManager,
    pub session_id: String,
    pub run_id: String,
    /// 本 run 的句柄（取消令牌 / ask 槽 / 计划 / steer / 帧枢纽）。
    pub handle: Arc<RunHandle>,
    /// 主驱动（挂 llm 日志钩子与每拍刷新源）。
    pub driver: Arc<TurnDriver<W, T>>,
    /// 模型显示名（runStarted.model 已发；收尾日志行同源）。
    pub model_name: String,
    /// run 规格（rig AgentRun 的构建来源）。
    pub spec: RunSpec,
    /// 本轮 prompt（wire 切分的末条 user 消息）。
    pub prompt: Arc<Message>,
    /// wire 切分的 history（系统前缀层已注入）。
    pub history: Vec<Arc<Message>>,
    /// 广播给模型的工具表。
    pub tools: Vec<Arc<ToolDefinition>>,
    /// 审批门（挂起恢复循环的签名放行面）。
    pub gate: Arc<ApprovalGate>,
    /// 工具执行器（装配面构造的 ToolsetExecutor）。
    pub executor: Arc<dyn crate::backend::run_loop::CancellableExecutor>,
    /// steer 插话源。
    pub steer: Arc<dyn SteerSource>,
    /// 每拍请求刷新源。
    pub refresher: Arc<dyn crate::backend::run_loop::RequestRefresher>,
    /// 模型停滞看门狗超时。
    pub stall_timeout: std::time::Duration,
    /// autoConfirm 快照（审批挂起 300s 无人应答自动放行）。
    pub auto_confirm: bool,
    /// 会话内记忆启用（命令类审批「始终允许（本会话）」选项开关）。
    pub session_memory_enabled: bool,
    /// 摘要驱动与模型身份（触发点②压缩 / 摘要独立解析）。
    pub summary_driver: Arc<TurnDriver<W, T>>,
    pub summary_model: (String, String),
    /// 压缩运行时策略。
    pub policy: CompactionRuntimePolicy,
    /// 审计的用户目标（本次 run 首条 user 输入文本）。
    pub audit_goal: String,
    /// suggest 上下文（会话消息尾文本）。
    pub suggest_tail: String,
    /// followUp 自动投递附着快照（转队尾条目的 RunConfig 来源）。
    pub run_config_for_delivery: super::RunConfig,
    /// 项目主目录（范围门 always_session 的词法路径展开基准）。
    pub project_root_for_scope: std::path::PathBuf,
    /// 会话累计共享表（快照于装配时刻）。
    pub session_usage: Arc<Mutex<HashMap<String, UsageTotal>>>,
    /// 当前生效模型槽（与 sink/refresher/日志共享同一句柄）。
    pub current_model: ModelSlot,
    /// 模型解析接缝（审计模型解析用）。
    pub resolve: ModelResolver<W, T>,
}

/// run 任务主体（tokio::spawn 的 future）：见模块文档。
pub(crate) async fn drive_main_run<W: RunWire, T: RunTransport<W>>(
    deps: RunTaskDeps<W, T>,
) {
    let RunTaskDeps {
        app,
        runs,
        session_id,
        run_id,
        handle,
        driver,
        model_name,
        spec,
        prompt,
        history,
        tools,
        gate,
        executor,
        steer,
        refresher,
        stall_timeout,
        auto_confirm,
        session_memory_enabled,
        summary_driver,
        summary_model,
        policy,
        audit_goal,
        suggest_tail,
        run_config_for_delivery,
        project_root_for_scope,
        session_usage,
        current_model,
        resolve,
    } = deps;

    // 帧出口 = 事件层（usage → tokens:update + RunState 同步 + 统计落账）
    // 包 canonical 落盘器包帧枢纽（seq 递增 → 入环 → 下发）
    let recorder = RunRecorder::new(
        app.store.clone(),
        session_id.clone(),
        run_id.clone(),
        Some(app.session_log.clone()),
    );
    let mut sink = EventfulSink {
        inner: Box::new(PersistingSink::new(
            recorder,
            Box::new(crate::agent::HubSink {
                hub: handle.hub.clone(),
            }),
        )),
        events: app.events.clone(),
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        last_run_total: UsageTotal::default(),
        session_usage: session_usage.clone(),
        handle: Some(handle.clone()),
        ledger: app.ledger.clone(),
        current_model: current_model.clone(),
        run_counted: false,
        // 会话日志器：工具起止行落点
        session_log: Some(app.session_log.clone()),
        tool_starts: HashMap::new(),
    };
    // run 开始行（kind=run：runId/模型）
    app.session_log.session_line(
        &session_id,
        crate::providers::types::LogLevel::Info,
        crate::session_log::LogKind::Run,
        "run 开始".to_owned(),
        Some(serde_json::json!({ "model": model_name })),
        Some(&run_id),
        None,
        None,
    );
    let mut run = rig::run::AgentRun::from_spec(&spec, (*prompt).clone(), Some(history));
    // 计划提醒：根会话 run 启用变更计数与
    // 注入（子代理 run 的 DriveState 缺省关闭——"根会话"口径）；初始
    // 状态经 drive_run 的 resume 链传递，跨审批挂起延续。
    let mut resume_state: Option<DriveState> = Some(DriveState {
        plan_reminder_enabled: true,
        ..DriveState::default()
    });
    // 错误终态信息（带 code：看门狗终止 E_RUN_STALLED，其余
    // E_INTERNAL 兜底）：loop 内记录、收尾随 run:error 事件发出
    let mut errored: Option<RunFailure> = None;
    let events = app.events.clone();
    let final_exit: RunExit = loop {
        let attempt = drive_run(
            &driver,
            run,
            &run_id,
            &model_name,
            handle.cancel.clone(),
            executor.clone(),
            gate.clone(),
            &mut sink,
            resume_state.take(),
            Some(steer.as_ref()),
            Some(refresher.as_ref()),
            stall_timeout,
        )
        .await;
        match attempt {
            Err(err) => {
                let message = err.to_string();
                errored = Some(RunFailure {
                    code: E_INTERNAL.to_owned(),
                    message: message.clone(),
                });
                // 错误终态兜底：任何 rig 失败都以 runEnded(error) 收场（终态不变量）
                send_error_ended(&mut sink, message);
                break RunExit::Cancelled;
            }
            // 每拍请求刷新失败：runEnded(error) 已在 drive_run
            // 内发出（预算越界 E_CONTEXT_OVERFLOW 等），记录失败终态供
            // run:error 事件与队列暂停分流（暂停保留输入）。
            Ok(RunExit::Failed { code, message }) => {
                errored = Some(RunFailure {
                    code: code.clone(),
                    message: message.clone(),
                });
                break RunExit::Failed { code, message };
            }
            Ok(RunExit::Suspended(info)) => {
                let crate::backend::run_loop::SuspendedInfo {
                    run_json,
                    pending,
                    state,
                } = *info;
                // 范围卡基线对照摘要：从共享计划状态读
                // 已批准冻结基线（挂起恢复时读最新值）。
                let plan_scope_note = plan_scope_note_of(&handle);
                // —— 灰区命令审计 ——
                // 挂起项属围栏灰区 command（audit_class：围栏 Confirm 且
                // 非 git 策略确认/范围门/外部目录/plan 档）时先审计再决定
                // 是否弹审批卡；审计帧经 sink 入环（attach 重放可恢复
                // 展示）。allow 经审计放行单槽直接恢复（不重复审计）；block
                // 按拒绝收场（E_AUDIT_BLOCKED）；ask/失败转人工（载荷带
                // 结论）；取消传播终止审计并按取消收场。
                let mut audit_note: Option<AuditNote> = None;
                if pending.name == "command" && gate.audit_class(&pending.name, &pending.args_json)
                {
                    let frame_call_id = state
                        .model_call_ids
                        .get(&pending.model_call_id)
                        .cloned()
                        .unwrap_or_else(|| pending.model_call_id.clone());
                    sink.send(ChatFrame::ToolAudit {
                        call_id: frame_call_id.clone(),
                        state: crate::backend::frames::AuditState::Running,
                        reason: None,
                    });
                    let plan_baseline: Vec<String> = handle
                        .plan
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .as_ref()
                        .map(|p| p.baseline_paths.clone())
                        .unwrap_or_default();
                    let session_model_id = current_model
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone()
                        .map(|(id, _)| id);
                    let flow = run_command_audit(
                        &app,
                        resolve.clone(),
                        &session_id,
                        &pending,
                        session_model_id.as_deref(),
                        &audit_goal,
                        &plan_baseline,
                        &handle.cancel,
                        AUDIT_TOTAL_TIMEOUT,
                    )
                    .await;
                    match flow {
                        AuditFlow::Verdict(verdict, attempt) => {
                            // 已获得的用量计入会话（kind=aux 不增主轮次，
                            // runs 计数不增）。
                            if let Some(delta) = attempt.usage {
                                record_aux_usage(
                                    &events,
                                    &session_usage,
                                    &app.ledger,
                                    &session_id,
                                    &format!("{run_id}:audit"),
                                    &attempt.model,
                                    &delta,
                                );
                            }
                            let reason = if verdict.reason.is_empty() {
                                "（模型未给出理由）".to_owned()
                            } else {
                                verdict.reason.clone()
                            };
                            match verdict.decision {
                                crate::backend::audit::AuditDecision::Allow => {
                                    sink.send(ChatFrame::ToolAudit {
                                        call_id: frame_call_id,
                                        state: crate::backend::frames::AuditState::Allow,
                                        reason: Some(reason),
                                    });
                                    // 审计放行单槽（消费即清）：恢复重放的
                                    // 判定在签名放行同位命中放行；同签名
                                    // 后续新调用不继承旧结论。不经既有
                                    // 人批签名缓存承载模型结论（硬约束）。
                                    gate.grant_audit(&pending.name, &pending.args_json);
                                    match resume_run(&run_json) {
                                        Ok(resumed) => {
                                            run = resumed;
                                            resume_state = Some(state);
                                            continue;
                                        }
                                        Err(e) => {
                                            let message = format!("审计放行恢复失败：{e}");
                                            errored = Some(RunFailure {
                                                code: E_INTERNAL.to_owned(),
                                                message: message.clone(),
                                            });
                                            send_error_ended(&mut sink, message);
                                            break RunExit::Cancelled;
                                        }
                                    }
                                }
                                crate::backend::audit::AuditDecision::Block => {
                                    sink.send(ChatFrame::ToolAudit {
                                        call_id: frame_call_id.clone(),
                                        state: crate::backend::frames::AuditState::Block,
                                        reason: Some(reason.clone()),
                                    });
                                    audit_block_finish(&mut sink, &frame_call_id, &state, &reason);
                                    let message = format!(
                                        "命令审计模型判断高风险，已拒绝执行：{reason}"
                                    );
                                    errored = Some(RunFailure {
                                        code: E_AUDIT_BLOCKED.to_owned(),
                                        message: message.clone(),
                                    });
                                    break RunExit::Failed {
                                        code: E_AUDIT_BLOCKED.to_owned(),
                                        message,
                                    };
                                }
                                crate::backend::audit::AuditDecision::Ask => {
                                    sink.send(ChatFrame::ToolAudit {
                                        call_id: frame_call_id,
                                        state: crate::backend::frames::AuditState::Ask,
                                        reason: Some(reason.clone()),
                                    });
                                    audit_note = Some(AuditNote {
                                        state: crate::backend::ask::AuditNoteState::Ask,
                                        reason,
                                    });
                                }
                            }
                        }
                        AuditFlow::Failed(reason, attempt) => {
                            // 超时/请求失败/输出无效转人工；已返回的 usage
                            // 同样记账（不伪造未返回的用量）。
                            if let Some(delta) = attempt.usage {
                                record_aux_usage(
                                    &events,
                                    &session_usage,
                                    &app.ledger,
                                    &session_id,
                                    &format!("{run_id}:audit"),
                                    &attempt.model,
                                    &delta,
                                );
                            }
                            sink.send(ChatFrame::ToolAudit {
                                call_id: frame_call_id,
                                state: crate::backend::frames::AuditState::Manual,
                                reason: Some(reason.clone()),
                            });
                            audit_note = Some(AuditNote {
                                state: crate::backend::ask::AuditNoteState::Manual,
                                reason,
                            });
                        }
                        AuditFlow::Cancelled => {
                            // 用户取消传播终止审计（证据读取与模型请求一并
                            // 丢弃），run 按取消收场。
                            audit_cancelled_finish(&mut sink, &frame_call_id, &state);
                            break RunExit::Cancelled;
                        }
                    }
                }
                match wait_approval_with(
                    &events,
                    &session_id,
                    &run_id,
                    &handle.cancel,
                    &handle.ask,
                    &pending,
                    auto_confirm,
                    session_memory_enabled,
                    plan_scope_note.as_deref(),
                    audit_note.as_ref(),
                    Some(&app.session_log),
                )
                .await
                {
                    ApprovalResolution::Allowed { option_id } => {
                        // 外部目录授权：允许 → 并入会话
                        // roots 快照（SessionMeta 持久化，同源核心）
                        // + 门热追加授权 roots——重放调用放行。授权落盘
                        // 失败不阻断恢复（内存面 grant 已生效，本 run
                        // 可继续；跨 run 持久化缺失时下次再询问）。
                        if let Some(dir) = &pending.external_dir
                            && option_id == "allow"
                        {
                            let _ = crate::api::workspace::allow_external_dir(
                                &app,
                                crate::api::workspace::AllowExternalDirArgs {
                                    session_id: session_id.clone(),
                                    dir: dir.clone(),
                                },
                            );
                            // 内存面授权用最长存在前缀归一（safe_join 同款
                            // 实现）——目标目录不存在时持久面被拒绝
                            // （canonicalize 失败），内存面仍生效让本 run
                            // 继续；否则重放判定不命中会再次挂起成死循环。
                            if let Ok(canonical) =
                                crate::safety::safe_join::canonicalize_best_effort(
                                    std::path::Path::new(dir),
                                )
                            {
                                gate.grant_external_dir(canonical);
                            }
                        }
                        gate.approve(&pending.name, &pending.args_json);
                        // （范围门）：always_session → 把写
                        // 目标父目录以目录范围（含子路径）并入会话基线
                        // 扩展——本 run 热追加（重放放行）+ 会话桶（跨
                        // run 生效，进程内不持久化——明确授权
                        // 不跨生命周期）。归一失败（畸形路径）只放弃
                        // 授权不阻断恢复（重放判定仍命中会再次询问）。
                        if option_id == "always_session"
                            && let Some(raw) = &pending.plan_scope
                        {
                            let lexical = if std::path::Path::new(raw).is_absolute() {
                                std::path::PathBuf::from(raw)
                            } else {
                                project_root_for_scope.join(raw)
                            };
                            if let Ok(resolved) =
                                crate::safety::safe_join::canonicalize_best_effort(&lexical)
                            {
                                let dir = resolved
                                    .parent()
                                    .map(std::path::Path::to_path_buf)
                                    .unwrap_or(resolved);
                                gate.grant_plan_scope(dir);
                            }
                        }
                        // always_session → 会话级免批桶登记
                        //（同签名命令本会话跨 run 免批；新会话不延续，
                        // 备份恢复不复活——记忆不持久化）
                        if option_id == "always_session" {
                            app.approval_memory.remember(
                                &session_id,
                                &pending.name,
                                &pending.args_json,
                            );
                        }
                        // MCP「始终允许」：写工具级许可表
                        // ——绑定服务器启动指纹 + 原始工具名 + 已确认输入
                        // schema 哈希三元组；新增工具/schema 变化不继承。
                        if option_id == "always_allow"
                            && let Some(identity) = app.mcp.registry().identity(&pending.name)
                        {
                            let _ = app.mcp.licenses.grant(
                                &identity.server_fingerprint,
                                &identity.original_name,
                                &identity.schema_hash,
                                now_ms(),
                            );
                        }
                        // ask 关闭后压缩（触发点②）：落盘面压缩，
                        // 失败不阻断恢复（硬预算义务在恢复后的请求面）
                        maybe_compact_after_ask(
                            &app.store,
                            &events,
                            &session_usage,
                            &app.ledger,
                            &session_id,
                            &run_id,
                            (&summary_model.0, &summary_model.1),
                            &pending,
                            &state,
                            &tools,
                            &policy,
                            &summary_driver,
                            Some(&app.session_log),
                        )
                        .await;
                        match resume_run(&run_json) {
                            Ok(resumed) => {
                                run = resumed;
                                resume_state = Some(state);
                                continue;
                            }
                            Err(e) => {
                                let message = format!("审批恢复失败：{e}");
                                errored = Some(RunFailure {
                                    code: E_INTERNAL.to_owned(),
                                    message: message.clone(),
                                });
                                send_error_ended(&mut sink, message);
                                break RunExit::Cancelled;
                            }
                        }
                    }
                    ApprovalResolution::Denied => {
                        denied_finish(&mut sink, &pending, &state);
                        break RunExit::Cancelled;
                    }
                }
            }
            // 看门狗终止：runEnded(error, E_RUN_STALLED)
            // 已在 drive_run 内发出（结果保留、副作用不重放），这里记
            // 失败终态供 run:error 事件与队列暂停分流。
            Ok(RunExit::Stalled { message }) => {
                errored = Some(RunFailure {
                    code: E_RUN_STALLED.to_owned(),
                    message: message.clone(),
                });
                break RunExit::Stalled { message };
            }
            Ok(other) => break other,
        }
    };

    // suggest 后台小调用（仅自然终态；静默失败、独立于 run 生命周期）
    if matches!(final_exit, RunExit::Done(_)) {
        let final_model = current_model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_default();
        spawn_suggest(
            driver,
            events.clone(),
            session_usage.clone(),
            app.ledger.clone(),
            session_id.clone(),
            run_id.clone(),
            suggest_tail,
            final_model,
        );
    }

    // run 收口（明细面）：补写 endedAt（终态只收口，不搬移已记日期）
    let _ = app.ledger.detail.mark_run_ended(&session_id, &run_id);

    // run 终态事件：GPUI 壳的 Tab 指示与系统通知门共用这三个键；payload
    // 口径照文档。失败终态的 code 从 RunFailure 带出（看门狗终止 =
    // E_RUN_STALLED）。（旧壳的 TauriEventSink 旁路属壳能力，随 ShellOnly
    // 排除清单不进 engine——通知触发由壳监听同键事件实现。）
    let duration_ms = (now_ms() - handle.snapshot().started_at).max(0) as u64;
    // run 收尾行（kind=run：runId/模型/耗时/tokens/终态 reason）。
    // tokens = run 内累计（RunState 的 usage 快照与 usage 帧同源）；
    // 终态 reason = done/error/cancelled（挂起不可能到收尾）。
    let final_usage = handle.snapshot().usage;
    let (reason, error_code) = match (&final_exit, &errored) {
        (RunExit::Done(_), _) => ("done", None),
        (_, Some(failure)) => ("error", Some(failure.code.clone())),
        _ => ("cancelled", None),
    };
    // 失败终态同时记应用通道（run 报错是 app.log 的关键
    // 运行元数据——无会话 id 关联正文，只记 runId 与错误码）
    if reason == "error" {
        app.session_log.app_line(
            crate::providers::types::LogLevel::Warn,
            format!("run 失败：{run_id}"),
            Some(serde_json::json!({ "runId": run_id, "code": error_code })),
        );
    }
    app.session_log.session_line(
        &session_id,
        if reason == "error" {
            crate::providers::types::LogLevel::Warn
        } else {
            crate::providers::types::LogLevel::Info
        },
        crate::session_log::LogKind::Run,
        match reason {
            "done" => "run 完成".to_owned(),
            "error" => "run 失败".to_owned(),
            _ => "run 已取消".to_owned(),
        },
        Some(serde_json::json!({
            "model": model_name,
            "durationMs": duration_ms,
            "tokens": final_usage,
            "reason": reason,
            "code": error_code,
        })),
        Some(&run_id),
        None,
        None,
    );
    match (&final_exit, &errored) {
        (RunExit::Done(_), _) => events.emit(
            keys::RUN_DONE,
            serde_json::json!({ "sessionId": session_id, "runId": run_id, "durationMs": duration_ms }),
        ),
        (_, Some(failure)) => events.emit(
            keys::RUN_ERROR,
            serde_json::json!({ "sessionId": session_id, "runId": run_id, "code": failure.code, "message": failure.message }),
        ),
        _ => events.emit(
            keys::RUN_CANCELLED,
            serde_json::json!({ "sessionId": session_id, "runId": run_id }),
        ),
    }
    runs.remove(&session_id, &run_id);

    // —— followUp 队列收尾分流 ——
    // 次序锚点：投递时机 = run 正常完成且终态处理完（runEnded 帧已发出 +
    // RunManager.remove 已摘除）。steer 未消费残余按终态分流：
    // Done → 转会话 followUp 队列尾（随下次投递发出）；取消/失败 →
    // 随 run:queue.returned 交还前端草稿（未消费的插话未落 canonical、
    // 未上时间线，文本交还即无丢失）。
    let leftover_steer = drain_steer(&handle);
    let followups = &app.followups;
    match followup_disposition(&final_exit, &errored) {
        crate::followup::QueueDisposition::AutoDeliver => {
            for text in leftover_steer {
                let entry = crate::api::run::followup_entry_from_text(&text, &run_config_for_delivery, Vec::new());
                followups.push(&session_id, entry);
            }
            let snapshot = followups.snapshot(&session_id);
            if snapshot.size > 0 && !snapshot.paused {
                crate::api::run::emit_queue_state(&events, &session_id, followups, Vec::new());
                spawn_followup_delivery(app.share_handles(), session_id.clone(), resolve.clone());
            } else {
                crate::api::run::emit_queue_state(&events, &session_id, followups, Vec::new());
            }
        }
        // 失败终态（含看门狗终止 E_RUN_STALLED 与 runEnded(error) 兜底）：
        // 暂停队列 + 持久保留全部待发，steer 残余交还
        crate::followup::QueueDisposition::PauseKeep => {
            followups.set_paused(&session_id, true);
            crate::api::run::emit_queue_state(&events, &session_id, followups, leftover_steer);
        }
        // 取消终态（含审批 deny 收场）：清空队列 + 全部文本交还草稿
        // （返还草稿；暂停态随清空一并复位）
        crate::followup::QueueDisposition::ClearReturn => {
            let mut returned = followups.clear(&session_id);
            returned.extend(leftover_steer);
            crate::api::run::emit_queue_state(&events, &session_id, followups, returned);
        }
    }
}
