//! 工具执行器适配：tools 注册表 → RunLoop 的
//! [`CancellableExecutor`](super::run_loop::CancellableExecutor) 契约。
//!
//! - 模型通道回喂文本 = [`crate::tools::compact_text`]（§8.2 双通道）；
//! - 帧面 ok/uiSummary/error 直接取 `ToolOutcome`；
//! - argsJson 解析失败 / 未知工具 → `E_INVALID_ARG` 工具级失败回喂
//!   （Malformed 调用的宿主决议走「回喂错误促模型纠正」，§5.4 记录见票 Answer）；
//! - command（14 号票）：取消/超时感知的进程执行（`tools::command` 内部
//!   select! 收割进程组）；
//! - plan（14 号票）：更新共享 PlanState 并发 `plan:update` 事件；
//! - ask（14 号票）：注册 AskSlot → `ask:opened` 事件 → 等 `resolve_ask`
//!   oneshot（§5.4 审批实现），answers 回喂为工具结果。
//!
//! 文件五工具为亚秒级同步操作，M1 RunLoop 串行执行（§5.4 ⑤「文件写全局
//! 串行队列」的满足面——单 run 内天然串行；批次事务预演为 M2）。

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::ask::{AskAnswer, AskSlot};
use super::frames::FrameError;
use super::run_loop::{CancellableExecutor, ToolExecution};
use crate::safety::tier::PermissionTier;
use crate::tools::plan::PlanState;
use crate::tools::{ToolOutcome, ToolsContext, compact_text, edit, grep, list_files, read, write};
use crate::util::errors::{E_INVALID_ARG, ToolError};
use crate::util::events::EventSink;
use crate::util::events::keys;

/// ask 槽单元（RunHandle.ask 的本体；agent 构造、backend 写入、host resolve_ask 读）。
pub type AskSlotCell = Mutex<Option<AskSlot>>;

/// 交互类执行依赖（14 号票：ask/plan 需要的运行时挂点；15 号票：边车数据根）。
#[derive(Clone)]
pub struct ExecutorEnv {
    pub events: Arc<dyn EventSink>,
    pub ask: Arc<AskSlotCell>,
    pub plan: Arc<Mutex<Option<PlanState>>>,
    pub session_id: String,
    pub run_id: String,
    /// 数据根（§7.1）：toolres 边车落 `sessions/<sid>.toolres/`（§7.5，15 号票）。
    pub data_root: std::path::PathBuf,
}

/// 注册表全集执行器（§8.3 模型可见 8 工具）。
pub struct ToolsetExecutor {
    ctx: ToolsContext,
    /// 权限档（§8.5）：plan 档过滤后的工具集之外调用一律 E_TOOL_BLOCKED。
    tier: PermissionTier,
    /// 交互类挂点；None = 无环境装配（headless 文件工具测试用）。
    env: Option<ExecutorEnv>,
}

impl ToolsetExecutor {
    pub fn new(ctx: ToolsContext, tier: PermissionTier) -> Self {
        Self {
            ctx,
            tier,
            env: None,
        }
    }

    /// 装配交互类挂点（ask/plan 事件与状态）。
    pub fn with_env(mut self, env: ExecutorEnv) -> Self {
        self.env = Some(env);
        self
    }

    /// 同步工具（文件五件）分发。
    fn dispatch(&self, name: &str, args_json: &str) -> ToolOutcome {
        // §8.1 三处同源过滤的 RunLoop 侧：plan 档 FileWrite/Network 已从工具表
        // 过滤，模型仍点名调用时按权限档排除（E_TOOL_BLOCKED，不中断 run）。
        let Some(spec) = crate::tools::build_registry()
            .into_iter()
            .find(|s| s.name == name)
        else {
            return ToolOutcome::failure(
                ToolError::new(E_INVALID_ARG, format!("未知工具：{name}")),
                format!("未知工具：{name}"),
            );
        };
        if crate::safety::tier::decide_gate(spec.capability, self.tier)
            == crate::safety::tier::GateDecision::Filtered
        {
            return ToolOutcome::failure(
                ToolError::tool_blocked(format!(
                    "权限档排除：{name}（当前档 {tier}）",
                    tier = tier_label(self.tier)
                )),
                format!("权限档排除：{name}"),
            );
        }
        let args: serde_json::Value = match serde_json::from_str(args_json) {
            Ok(v) => v,
            Err(e) => {
                return ToolOutcome::failure(
                    ToolError::invalid_arg(format!("工具 {name} 参数不是合法 JSON：{e}")),
                    format!("工具 {name} 参数解析失败"),
                );
            }
        };
        match name {
            "read" => run_typed(&self.ctx, &args, read::run),
            "write" => run_typed(&self.ctx, &args, write::run),
            "edit" => run_typed(&self.ctx, &args, edit::run),
            "list_files" => run_typed(&self.ctx, &args, list_files::run),
            "grep" => run_typed(&self.ctx, &args, grep::run),
            // command/plan/ask 是异步交互型，execute 内分发
            other => ToolOutcome::failure(
                ToolError::new(E_INVALID_ARG, format!("未知工具：{other}")),
                format!("未知工具：{other}"),
            ),
        }
    }

    /// plan 工具：更新共享 PlanState + plan:update 事件。
    fn run_plan(&self, args_json: &str) -> ToolOutcome {
        let Some(env) = &self.env else {
            return ToolOutcome::failure(
                ToolError::new(E_INVALID_ARG, "plan 工具需要执行环境（未装配）"),
                "计划更新失败",
            );
        };
        let args: crate::tools::plan::PlanArgs = match serde_json::from_str(args_json) {
            Ok(a) => a,
            Err(e) => {
                return ToolOutcome::failure(
                    ToolError::invalid_arg(format!("plan 参数解析失败：{e}")),
                    "计划参数解析失败",
                );
            }
        };
        let mut plan_state = env
            .plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let plan_state = plan_state.get_or_insert_with(PlanState::default);
        match crate::tools::plan::apply(plan_state, &args) {
            Ok(items) => {
                env.events.emit(
                    keys::PLAN_UPDATE,
                    serde_json::json!({ "sessionId": env.session_id, "items": items }),
                );
                let summary = format!(
                    "计划已更新（{} 项，{} 进行中）",
                    items.len(),
                    items
                        .iter()
                        .filter(|i| i.status == crate::storage::entries::PlanItemStatus::InProgress)
                        .count()
                );
                ToolOutcome::success(crate::tools::plan::outcome_data(&items), summary)
            }
            Err(err) => ToolOutcome::from(err),
        }
    }

    /// ask 工具：注册槽 → ask:opened → 等 resolve_ask / 取消。
    async fn run_ask(&self, args_json: &str, token: &CancellationToken) -> ToolExecution {
        let Some(env) = &self.env else {
            return completed_failure("ask 工具需要执行环境（未装配）");
        };
        let args: crate::tools::ask::AskArgs = match serde_json::from_str(args_json) {
            Ok(a) => a,
            Err(e) => return completed_failure(&format!("ask 参数解析失败：{e}")),
        };
        let payload = match crate::tools::ask::build_payload(&args) {
            Ok(p) => p,
            Err(err) => return completed_failure(&err.message),
        };
        let (tx, rx) = oneshot::channel::<AskAnswer>();
        let ask_id = uuid::Uuid::new_v4().to_string();
        {
            let mut slot = env
                .ask
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if slot.is_some() {
                return completed_failure("已有未决询问（同一时刻只允许一个）");
            }
            *slot = Some(AskSlot {
                ask_id: ask_id.clone(),
                session_id: env.session_id.clone(),
                run_id: env.run_id.clone(),
                kind: "question",
                tx,
            });
        }
        env.events.emit(
            keys::ASK_OPENED,
            serde_json::json!({
                "sessionId": env.session_id,
                "askId": ask_id,
                "payload": payload,
            }),
        );
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                Self::close_ask(env, &ask_id, false);
                ToolExecution::Cancelled { executed: false, message: "询问被取消".to_owned() }
            }
            answer = rx => match answer {
                Ok(AskAnswer::Question { answers }) => {
                    Self::close_ask(env, &ask_id, true);
                    ToolExecution::Completed {
                        ok: true,
                        ui_summary: format!("用户已回答 {} 个问题", answers.len()),
                        error: None,
                        model_text: compact_text(&ToolOutcome::success(
                            serde_json::json!({ "answers": answers }),
                            "用户已回答",
                        )),
                    }
                }
                Ok(other) => {
                    Self::close_ask(env, &ask_id, true);
                    completed_failure(&format!("询问收到非问卷应答：{other:?}"))
                }
                Err(_) => {
                    // 发送端 drop（run 任务终止）：按取消收口
                    Self::close_ask(env, &ask_id, false);
                    ToolExecution::Cancelled { executed: false, message: "询问中断".to_owned() }
                }
            }
        }
    }

    /// 清空槽位 + ask:closed 事件（§3.5：answered = 是否拿到应答）。
    fn close_ask(env: &ExecutorEnv, ask_id: &str, answered: bool) {
        let mut slot = env
            .ask
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = None;
        env.events.emit(
            keys::ASK_CLOSED,
            serde_json::json!({ "askId": ask_id, "answered": answered }),
        );
    }

    /// 工具结果全量写 toolres 边车（§7.5，15 号票；无环境装配则跳过）。
    fn persist_outcome(&self, call_id: &str, outcome: &ToolOutcome) {
        if let Some(env) = &self.env {
            crate::storage::write_outcome(&env.data_root, &env.session_id, call_id, outcome);
        }
    }
}

/// 参数解析 + 调用 + 失败兜底的模板（解析失败 → E_INVALID_ARG 结果回喂）。
fn run_typed<A: serde::de::DeserializeOwned>(
    ctx: &ToolsContext,
    args: &serde_json::Value,
    f: fn(&ToolsContext, &A) -> ToolOutcome,
) -> ToolOutcome {
    match serde_json::from_value::<A>(args.clone()) {
        Ok(parsed) => f(ctx, &parsed),
        Err(e) => ToolOutcome::failure(
            ToolError::invalid_arg(format!("工具参数校验失败：{e}")),
            "工具参数校验失败",
        ),
    }
}

fn tier_label(tier: PermissionTier) -> &'static str {
    match tier {
        PermissionTier::Plan => "plan",
        PermissionTier::ConfirmEach => "confirm_each",
        PermissionTier::AutoEdit => "auto_edit",
        PermissionTier::FullAccess => "full_access",
    }
}

/// 工具级失败 → Completed（失败也是收齐的结果，回喂模型促其纠正）。
fn completed_failure(message: &str) -> ToolExecution {
    let outcome = ToolOutcome::failure(ToolError::invalid_arg(message.to_owned()), message);
    completed_of(outcome)
}

/// ToolOutcome → ToolExecution（模型通道先快照，再搬帧面字段）。
fn completed_of(outcome: ToolOutcome) -> ToolExecution {
    let model_text = compact_text(&outcome);
    ToolExecution::Completed {
        ok: outcome.ok,
        ui_summary: outcome.ui_summary,
        error: outcome.error.map(|e| FrameError::new(e.code, e.message)),
        model_text,
    }
}

impl CancellableExecutor for ToolsetExecutor {
    fn execute<'a>(
        &'a self,
        call_id: &'a str,
        name: &'a str,
        args_json: &'a str,
        token: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async move {
            match name {
                "command" => {
                    let outcome =
                        match serde_json::from_str::<crate::tools::command::CommandArgs>(args_json)
                        {
                            Ok(args) => {
                                crate::tools::command::execute(&self.ctx, call_id, &args, token)
                                    .await
                            }
                            Err(e) => crate::tools::command::CommandExit::Done(Box::new(
                                ToolOutcome::failure(
                                    ToolError::invalid_arg(format!("command 参数解析失败：{e}")),
                                    "命令参数解析失败",
                                ),
                            )),
                        };
                    match outcome {
                        crate::tools::command::CommandExit::Done(outcome) => {
                            self.persist_outcome(call_id, &outcome);
                            completed_of(*outcome)
                        }
                        crate::tools::command::CommandExit::Cancelled => ToolExecution::Cancelled {
                            executed: true,
                            message: "命令进程组已收割".to_owned(),
                        },
                    }
                }
                "plan" => {
                    let outcome = self.run_plan(args_json);
                    self.persist_outcome(call_id, &outcome);
                    completed_of(outcome)
                }
                "ask" => {
                    let execution = self.run_ask(args_json, token).await;
                    if let ToolExecution::Completed { ok, ui_summary, .. } = &execution {
                        // ask 应答即结果，无全量 detail：轻量边车供卡片恢复
                        let outcome = if *ok {
                            ToolOutcome::success(
                                serde_json::json!({ "answered": true }),
                                ui_summary.clone(),
                            )
                        } else {
                            ToolOutcome::failure(
                                ToolError::invalid_arg(ui_summary.clone()),
                                ui_summary.clone(),
                            )
                        };
                        self.persist_outcome(call_id, &outcome);
                    }
                    execution
                }
                other => {
                    let outcome = self.dispatch(other, args_json);
                    self.persist_outcome(call_id, &outcome);
                    completed_of(outcome)
                }
            }
        })
    }
}

/// 供测试/未来复用的取消感知骨架：在闭包外包一层 select!，
/// token 触发即放弃闭包（由实现自行收割资源后返回 Cancelled）。
pub async fn with_cancel<F: Future<Output = ToolExecution>>(
    token: &CancellationToken,
    fut: F,
    executed_message: impl FnOnce() -> String,
) -> ToolExecution {
    tokio::select! {
        biased;
        _ = token.cancelled() => ToolExecution::Cancelled {
            executed: true,
            message: executed_message(),
        },
        result = fut => result,
    }
}

/// Arc 别名（RunLoop 参数签名用）。
pub type SharedExecutor = Arc<dyn CancellableExecutor>;
