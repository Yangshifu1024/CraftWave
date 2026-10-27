//! 工具执行器适配：把 tools 模块的工具注册表接到主循环（RunLoop）要求的
//! [`CancellableExecutor`](super::run_loop::CancellableExecutor) 执行契约上。
//!
//! 名词解释：主循环即 backend/run_loop.rs 的 run 主循环；契约指它规定的
//! 执行器接口（可感知取消、返回统一的执行产物）。工具结果走"双通道"
//! （§8.2）：一条给模型看（回喂文本），一条给界面看（摘要与错误帧）。
//!
//! - 模型通道的回喂文本由 [`crate::tools::compact_text`] 压缩生成；
//! - 帧面的 ok/uiSummary/error 直接取自 `ToolOutcome`（工具执行结果）；
//! - 参数 JSON 解析失败或未知工具 → 返回 `E_INVALID_ARG` 的工具级失败并
//!   回喂模型（不中断 run，让模型看到错误后自行纠正；Malformed 调用——
//!   即模型给出了无法解析的工具调用——也按此"回喂错误促纠正"处理，见
//!   §5.4 对应工单的决议记录）；
//! - command：取消/超时感知的进程执行（`tools::command` 内部用 select!
//!   同时监听执行与取消，结束时回收整个进程组，不留孤儿进程）；
//! - plan：更新共享的计划状态（PlanState）并发 `plan:update` 事件；
//! - ask：注册询问槽 → 发 `ask:opened` 事件 → 等 `resolve_ask` 命令经
//!   oneshot 通道送回答案（§5.4 审批实现），答案回喂为工具结果。
//!
//! 五个文件工具（读/写/改/列/搜）都是亚秒级同步操作，当前阶段由主循环
//! 串行执行（单 run 内天然串行，满足"文件写全局串行队列"的约束；多工具
//! 批次的事务化预演属后续阶段）。

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

/// ask 槽单元：互斥锁包着的"至多一个未决询问"（agent 模块构造、backend
/// 执行器写入、宿主命令 resolve_ask 读取后送回应答）。
pub type AskSlotCell = Mutex<Option<AskSlot>>;

/// 交互类工具的执行环境：ask/plan 工具需要的事件出口与共享状态，以及
/// 工具结果边车文件的数据根目录（边车文件 = 与主存储分开、按需加载的
/// 附属数据文件，见 §7.5）。
#[derive(Clone)]
pub struct ExecutorEnv {
    /// 事件出口（向前端推送 ask:opened / plan:update 等事件）。
    pub events: Arc<dyn EventSink>,
    /// 未决询问槽（ask 工具注册、resolve_ask 消费）。
    pub ask: Arc<AskSlotCell>,
    /// 共享计划状态（plan 工具写、审批门读）。
    pub plan: Arc<Mutex<Option<PlanState>>>,
    /// 所属会话 id（事件载荷用）。
    pub session_id: String,
    /// 所属 run id（事件载荷与询问槽登记用）。
    pub run_id: String,
    /// 数据根目录（§7.1）：工具结果边车落在 `sessions/<会话id>.toolres/`
    /// 目录下（§7.5）。
    pub data_root: std::path::PathBuf,
}

/// 工具注册表全集的执行器：模型可见的 8 个工具（§8.3）都经它执行。
pub struct ToolsetExecutor {
    /// 工具执行上下文（项目根、权限边界等，tools 模块定义）。
    ctx: ToolsContext,
    /// 权限档（§8.5，控制模型可用工具范围的档位）：plan 档已从工具表
    /// 过滤掉写/联网工具，模型仍点名调用时按权限档排除。
    tier: PermissionTier,
    /// 交互类工具的执行环境；None = 未装配环境（无界面的纯文件工具测试用）。
    env: Option<ExecutorEnv>,
}

/// ToolsetExecutor 的构造与各工具执行路径。
impl ToolsetExecutor {
    /// 构造：给定工具上下文与权限档，初始不带交互环境（生产装配需再调
    /// [`Self::with_env`]）。
    pub fn new(ctx: ToolsContext, tier: PermissionTier) -> Self {
        Self {
            ctx,
            tier,
            env: None,
        }
    }

    /// 装配交互类工具的运行环境（ask/plan 的事件出口与共享状态）。
    pub fn with_env(mut self, env: ExecutorEnv) -> Self {
        self.env = Some(env);
        self
    }

    /// 同步工具（五个文件工具）的分发入口。
    fn dispatch(&self, name: &str, args_json: &str) -> ToolOutcome {
        // §8.1 三处同源过滤的主循环侧：plan 档的写/联网工具虽已从广播给
        // 模型的工具表里过滤，但模型仍可能点名调用——此时按权限档排除
        // （E_TOOL_BLOCKED，回喂错误但不中断 run）。
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

    /// plan 工具：更新共享计划状态（PlanState），并向前端发 plan:update 事件。
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

    /// ask 工具：注册询问槽 → 发 ask:opened 事件 → 等待 resolve_ask 应答
    /// 或取消令牌触发，二者先到者胜。
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
            // 同一时刻只允许一个未决询问：槽位已被占用则直接以工具级失败回喂。
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
        // biased：select! 的分支按书写顺序优先判定——先查取消再看应答，
        // 两者同时就绪时以取消为准（取消优先，避免取消后还处理应答）。
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

    /// 清空询问槽并发 ask:closed 事件（§3.5：answered 标记是否真正拿到了
    /// 用户应答，取消关闭时为 false）。
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

    /// 把工具结果全量写入边车文件（§7.5；未装配执行环境则跳过——无界面
    /// 测试没有数据根目录）。
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

/// 权限档的对外展示名（错误提示文案里用）。
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

/// ToolOutcome 到 ToolExecution 的转换（模型通道文本先经 compact_text
/// 压缩快照，再搬运帧面字段）。
fn completed_of(outcome: ToolOutcome) -> ToolExecution {
    let model_text = compact_text(&outcome);
    ToolExecution::Completed {
        ok: outcome.ok,
        ui_summary: outcome.ui_summary,
        error: outcome.error.map(|e| FrameError::new(e.code, e.message)),
        model_text,
    }
}

/// 主循环执行契约的实现：按工具名分流——command/plan/ask 三个交互型工具
/// 各有专门路径，其余同步文件工具走 dispatch。
impl CancellableExecutor for ToolsetExecutor {
    /// 执行一次工具调用：完成后把结果写边车文件并返回统一产物；
    /// command 的取消由工具实现内部收割进程组后以 Cancelled 返回。
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

/// 供测试与未来复用的取消感知骨架：在任意异步操作外包一层 select!，
/// 取消令牌触发即放弃该操作（由实现自行回收资源后返回 Cancelled）。
pub async fn with_cancel<F: Future<Output = ToolExecution>>(
    token: &CancellationToken,
    fut: F,
    executed_message: impl FnOnce() -> String,
) -> ToolExecution {
    tokio::select! {
        // 取消优先判定（biased）：令牌触发时报告"已执行过"的取消语义。
        biased;
        _ = token.cancelled() => ToolExecution::Cancelled {
            executed: true,
            message: executed_message(),
        },
        result = fut => result,
    }
}

/// 执行器的共享句柄别名（主循环参数签名用；Arc = 线程安全的引用计数指针，
/// 多处持有同一执行器）。
pub type SharedExecutor = Arc<dyn CancellableExecutor>;
