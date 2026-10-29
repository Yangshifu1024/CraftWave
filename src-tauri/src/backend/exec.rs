//! 工具执行器适配：把 tools 模块的工具注册表接到主循环（RunLoop）要求的
//! [`CancellableExecutor`](super::run_loop::CancellableExecutor) 执行契约上，
//! 并提供受控并发的批次执行（§5.4 ⑤，07 号票）。
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
//! 批次执行（07 号票，FR-03-11/FR-04-04）：[`ToolsetExecutor::execute_batch`]
//! 把 M1 的批内串行升级为受控并发——
//! - 同批写路径冲突检测（Q32，§8.4 步骤 4）：write/edit 经 safe_join 求
//!   canonical 路径分组；同文件多个 edit 合并为一次原子提交（批次事务），
//!   同文件混入 write 或区间重叠 → 拒绝后续冲突调用（E_INVALID_ARG）；
//! - FileWrite 能力（write/edit）走**进程级全局互斥队列**（跨会话串行，
//!   防两会话同写一文件）；
//! - 其余能力（ReadOnly/Meta/Network）无冲突并发，信号量额度 =
//!   `tools.maxParallel`（缺省 4，§3.2）；
//! - JoinSet 收集后按**源序**返回（FR-03-11：完成顺序不改序）；
//! - 取消收齐（§5.3 并发版）：cancel 置位 → 停止派发未开始任务 → join
//!   全部在飞任务，未派发项补 `Cancelled { executed: false }`。

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::ask::{AskAnswer, AskSlot};
use super::frames::FrameError;
use super::run_loop::{BatchCall, CancellableExecutor, ToolExecution};
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
/// `Clone`：批次并发派发需要把执行器克隆进 tokio 任务（全部字段为
/// Arc/Copy 级共享句柄，克隆零深拷贝）。
pub struct ToolsetExecutor {
    /// 工具执行上下文（项目根、权限边界等，tools 模块定义）。
    ctx: ToolsContext,
    /// 权限档（§8.5，控制模型可用工具范围的档位）：plan 档已从工具表
    /// 过滤掉写/联网工具，模型仍点名调用时按权限档排除。
    tier: PermissionTier,
    /// 交互类工具的执行环境；None = 未装配环境（无界面的纯文件工具测试用）。
    env: Option<ExecutorEnv>,
    /// 同批无冲突调用的并发上限（`tools.maxParallel`，§3.2；缺省 4）。
    /// 按 run 装配执行器时读取生效。
    max_parallel: usize,
}

impl Clone for ToolsetExecutor {
    fn clone(&self) -> Self {
        Self {
            ctx: self.ctx.clone(),
            tier: self.tier,
            env: self.env.clone(),
            max_parallel: self.max_parallel,
        }
    }
}

/// ToolsetExecutor 的构造与各工具执行路径。
impl ToolsetExecutor {
    /// 构造：给定工具上下文与权限档，初始不带交互环境（生产装配需再调
    /// [`Self::with_env`]）。并发上限取缺省 4（可再调 [`Self::with_max_parallel`]）。
    pub fn new(ctx: ToolsContext, tier: PermissionTier) -> Self {
        Self {
            ctx,
            tier,
            env: None,
            max_parallel: crate::providers::types::DEFAULT_MAX_PARALLEL as usize,
        }
    }

    /// 装配交互类工具的运行环境（ask/plan 的事件出口与共享状态）。
    pub fn with_env(mut self, env: ExecutorEnv) -> Self {
        self.env = Some(env);
        self
    }

    /// 设置同批无冲突调用的并发上限（`tools.maxParallel`，§3.2）。
    pub fn with_max_parallel(mut self, max_parallel: usize) -> Self {
        self.max_parallel = max_parallel.max(1);
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
                // 原始载荷留存：重连后的待审批/询问恢复查询（§5.6 #59c）据此展示
                payload: payload.clone(),
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

/// FileWrite 能力的进程级全局互斥（§5.4 ⑤"文件写全局队列"，07 号票）：
/// 跨会话串行——同进程全部会话的 write/edit 批次任务在此排队，防止两个
/// 会话同时写同一文件。OnceLock 惰性单例（进程内唯一）。
pub(crate) fn file_write_gate() -> &'static Arc<tokio::sync::Mutex<()>> {
    static GATE: OnceLock<Arc<tokio::sync::Mutex<()>>> = OnceLock::new();
    GATE.get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
}

/// 任务体的产出类型：（成员源序下标, 结果）对序列（合并组产出多个）。
pub(super) type TaskOutput = Vec<(usize, ToolExecution)>;

/// 批内已规划的执行任务（源序无关；结果按成员下标回填）。
pub(super) struct PlannedTask {
    /// 任务覆盖的成员源序下标（取消免派发与结果回填的定位面）。
    pub member_indices: Vec<usize>,
    /// 是否 FileWrite 能力（持全局写互斥执行，不占信号量额度）。
    pub file_write: bool,
    /// 任务体：产出（成员下标, 结果）对（合并组产出多个）。
    pub run: Pin<Box<dyn Future<Output = TaskOutput> + Send>>,
}

/// 批次并发调度核心（§5.4 ⑤，07 号票；纯调度，不含写冲突检测——那是
/// [`ToolsetExecutor::plan_batch`] 的职责，单测可直接驱动）：
///
/// - 任务并发派发（tokio JoinSet）；非文件写任务先过信号量（额度 =
///   `max_parallel`， ReadOnly/Meta/Network 无冲突并发共享）；
/// - 文件写任务持 [`file_write_gate`] 全局互斥（跨会话串行），不占信号量；
/// - **取消收齐（§5.3 并发版）**：cancel 置位 → 停止派发未开始任务（补
///   `Cancelled { executed: false }`）→ join 全部在飞任务（命令类由工具
///   实现内部收割）→ 全部收齐后才返回；
/// - 输出与批内下标一一对应（源序），完成顺序不改序。
pub(super) async fn run_batch_tasks(
    tasks: Vec<PlannedTask>,
    mut results: Vec<Option<ToolExecution>>,
    max_parallel: usize,
    token: &CancellationToken,
) -> Vec<ToolExecution> {
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_parallel.max(1)));
    let mut join_set = tokio::task::JoinSet::new();
    for task in tasks {
        if token.is_cancelled() {
            // 取消：停止派发未开始任务，成员位置补未执行标记（不留悬空）。
            for idx in task.member_indices {
                results[idx] = Some(ToolExecution::Cancelled {
                    executed: false,
                    message: "取消：工具未执行".to_owned(),
                });
            }
            continue;
        }
        let permit_needed = !task.file_write;
        let gate = task.file_write.then(|| file_write_gate().clone());
        let semaphore = semaphore.clone();
        join_set.spawn(async move {
            // 锁序：信号量与写锁分别由两类任务独占持有（文件写不走信号量），
            // 无交叉等待，不存在死锁环。
            let _permit = if permit_needed {
                Some(semaphore.acquire().await.expect("信号量不会被关闭"))
            } else {
                None
            };
            let _guard = match &gate {
                Some(g) => Some(g.lock().await),
                None => None,
            };
            task.run.await
        });
    }
    while let Some(joined) = join_set.join_next().await {
        if let Ok(pairs) = joined {
            for (idx, exec) in pairs {
                results[idx] = Some(exec);
            }
        }
        // 任务 panic（JoinError）：成员位置留空，下方兜底为内部错误。
    }
    results
        .into_iter()
        .map(|slot| {
            slot.unwrap_or_else(|| ToolExecution::Completed {
                ok: false,
                ui_summary: "工具执行失败：批次任务异常终止".to_owned(),
                error: Some(FrameError::new(E_INVALID_ARG, "批次任务异常终止")),
                model_text: format!("{E_INVALID_ARG} 批次任务异常终止"),
            })
        })
        .collect()
}

/// 同文件写冲突的统一拒绝产物（Q32，§8.4 步骤 4）：后续冲突调用以
/// E_INVALID_ARG 明确回喂，源序首个写调用保留执行资格。
fn conflict_rejected(path: &str, reason: &str) -> ToolExecution {
    ToolExecution::Completed {
        ok: false,
        ui_summary: format!("已拒绝：同批对 {path} 的写操作冲突"),
        error: Some(FrameError::new(
            E_INVALID_ARG,
            format!("同批写路径冲突：{reason}（目标 {path}）"),
        )),
        model_text: format!(
            "{E_INVALID_ARG} 该调用已拒绝执行：{reason}（目标 {path}）。\
             请等待本批完成后，基于最新文件内容重新决定。"
        ),
    }
}

/// 批内一次调用的能力分级查询（未知工具 → None，由执行路径回喂错误）。
fn capability_of(name: &str) -> Option<crate::safety::tier::Capability> {
    crate::tools::build_registry()
        .into_iter()
        .find(|spec| spec.name == name)
        .map(|spec| spec.capability)
}

/// 主循环执行契约的实现：按工具名分流——command/plan/ask 三个交互型工具
/// 各有专门路径，其余同步文件工具走 dispatch；批次执行为受控并发（§5.4 ⑤）。
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

    /// 受控并发批次执行（§5.4 ⑤，07 号票）：同批写冲突检测 → 任务规划 →
    /// [`run_batch_tasks`] 并发调度 → 按源序返回（与 `batch` 一一对应）。
    fn execute_batch<'a>(
        &'a self,
        batch: &'a [BatchCall],
        token: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Vec<ToolExecution>> + Send + 'a>> {
        Box::pin(async move {
            let (tasks, results) = self.plan_batch(batch, token);
            run_batch_tasks(tasks, results, self.max_parallel, token).await
        })
    }
}

/// ToolsetExecutor 的批次规划与合并组执行（§5.4 ②/⑤，07 号票）。
impl ToolsetExecutor {
    /// 同批写路径冲突检测 + 任务规划（Q32，§8.4 步骤 4）：
    ///
    /// - write/edit 先 safe_join 求 canonical 路径，同路径写调用分组；
    /// - 组内全为 edit（≥2 个）→ 合并组（[`Self::run_edit_group`] 统一预演、
    ///   重叠拒绝、合并原子提交）；组内仅 1 个写调用 → 独立执行；
    /// - 同文件混入 write（或多个 write）→ 源序首个保留，其余全部入口拒绝
    ///   （E_INVALID_ARG；write 语义为"仅新建"，与 edit/其他 write 互斥）；
    /// - 路径解析失败的写调用 → 独立执行（由工具自身回喂路径/参数错误）；
    /// - 其余调用（ReadOnly/Meta/Network/Interactive）→ 独立执行。
    ///
    /// 返回（任务列表, 预填结果——入口拒绝项已带结果，其余为 None）。
    fn plan_batch(
        &self,
        batch: &[BatchCall],
        token: &CancellationToken,
    ) -> (Vec<PlannedTask>, Vec<Option<ToolExecution>>) {
        let mut results: Vec<Option<ToolExecution>> = vec![None; batch.len()];
        // 写调用分组：canonical 路径 → (源序下标, 是否 write)。线性容器保持
        // 首次出现顺序（批容量小，线性查找足够）。
        let mut groups: Vec<(String, Vec<(usize, bool)>)> = Vec::new();
        let mut tasks: Vec<PlannedTask> = Vec::new();
        for (idx, call) in batch.iter().enumerate() {
            let capability = capability_of(&call.name);
            if capability == Some(crate::safety::tier::Capability::FileWrite)
                && let Some(canonical) = self.write_target(call)
            {
                let is_write = call.name == "write";
                match groups.iter_mut().find(|(p, _)| *p == canonical) {
                    Some((_, members)) => members.push((idx, is_write)),
                    None => groups.push((canonical, vec![(idx, is_write)])),
                }
                continue;
            }
            tasks.push(self.plan_single(idx, call, token));
        }
        for (path, members) in groups {
            // 组内按源序排（成员按批内出现序已有序，防御性排序）。
            let mut members = members;
            members.sort_by_key(|(idx, _)| *idx);
            let contains_write = members.iter().any(|&(_, is_write)| is_write);
            if !contains_write && members.len() >= 2 {
                // 全为 edit 且 ≥2：合并组（组内预演 + 合并原子提交）。
                let this = self.clone();
                let member_calls: Vec<(usize, BatchCall)> = members
                    .iter()
                    .map(|&(idx, _)| (idx, batch[idx].clone()))
                    .collect();
                tasks.push(PlannedTask {
                    member_indices: members.iter().map(|&(idx, _)| idx).collect(),
                    file_write: true,
                    run: Box::pin(async move { this.run_edit_group(member_calls).await }),
                });
                continue;
            }
            // 首个写调用保留执行资格；其余（write 混批/多 write）拒绝。
            let (first_idx, _) = members[0];
            tasks.push(self.plan_single(first_idx, &batch[first_idx], token));
            for &(idx, _) in members.iter().skip(1) {
                results[idx] = Some(conflict_rejected(
                    &path,
                    "同批对同一文件混入 write（仅新建语义，与 edit/重复 write 互斥）",
                ));
            }
        }
        // 任务按源序排（结果按下标回填，次序仅影响派发顺序）。
        tasks.sort_by_key(|task| task.member_indices[0]);
        (tasks, results)
    }

    /// 独立执行任务（单调用）：FileWrite 能力持全局写互斥，其余走信号量。
    fn plan_single(
        &self,
        index: usize,
        call: &BatchCall,
        token: &CancellationToken,
    ) -> PlannedTask {
        let file_write =
            capability_of(&call.name) == Some(crate::safety::tier::Capability::FileWrite);
        let this = self.clone();
        let call = call.clone();
        let token = token.clone();
        PlannedTask {
            member_indices: vec![index],
            file_write,
            run: Box::pin(async move {
                let exec = this
                    .execute(&call.call_id, &call.name, &call.args_json, &token)
                    .await;
                vec![(index, exec)]
            }),
        }
    }

    /// 写目标（write/edit）的 canonical 路径提取（safe_join；参数不含 path
    /// 或路径越界 → None，保持独立执行回喂错误）。
    fn write_target(&self, call: &BatchCall) -> Option<String> {
        let args: serde_json::Value = serde_json::from_str(&call.args_json).ok()?;
        let raw = args.get("path")?.as_str()?;
        let resolved = self.ctx.guard().ok()?.resolve(raw).ok()?;
        Some(resolved.to_string_lossy().into_owned())
    }

    /// 同文件多个 edit 的合并组执行（§8.4 步骤 4/5，Q32）：统一基于修改前
    /// 内容预演（三级匹配 → 归一化字节区间），区间重叠的后续成员拒绝
    /// （E_INVALID_ARG），接受成员合并为一次原子落盘（批次事务，失败回滚
    /// 整组）；任一成员匹配失败 → 整组降级为源序逐个独立执行。结果按成员
    /// 源序返回并写边车。
    async fn run_edit_group(
        &self,
        members: Vec<(usize, BatchCall)>,
    ) -> Vec<(usize, ToolExecution)> {
        // 严格解析成员参数（plan 阶段只宽松提取了 path）；解析失败按工具级
        // 失败收口，不阻塞其他成员。全体按源序排。
        let mut ordered: Vec<(usize, String, Result<edit::EditArgs, String>)> = members
            .into_iter()
            .map(|(index, call)| {
                let args = serde_json::from_str::<edit::EditArgs>(&call.args_json)
                    .map_err(|e| format!("edit 参数解析失败：{e}"));
                (index, call.call_id, args)
            })
            .collect();
        ordered.sort_by_key(|(index, _, _)| *index);
        let mut mergeable: Vec<(usize, String, edit::EditArgs)> = Vec::new();
        let mut outputs: Vec<(usize, ToolExecution)> = Vec::new();
        for (index, call_id, args) in ordered {
            match args {
                Ok(a) => mergeable.push((index, call_id, a)),
                Err(message) => outputs.push((index, completed_failure(&message))),
            }
        }
        if !mergeable.is_empty() {
            let path = mergeable[0].2.path.clone();
            let args_list: Vec<edit::EditArgs> =
                mergeable.iter().map(|(_, _, a)| a.clone()).collect();
            let outcomes = edit::apply_edit_group(&self.ctx, &path, &args_list, |p, b| {
                write::atomic_write_utf8(p, b)
            });
            for ((index, call_id, _), outcome) in mergeable.into_iter().zip(outcomes) {
                self.persist_outcome(&call_id, &outcome);
                outputs.push((index, completed_of(outcome)));
            }
        }
        outputs.sort_by_key(|(index, _)| *index);
        outputs
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
