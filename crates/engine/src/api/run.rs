//! 运行域 API：start_chat、cancel、steer/followUp 队列、ask 权限处置、
//! 计划批准、压缩与帧重放续流。
//!
//! 平移来源：`host/commands/chat.rs`（帧流编排域）。其中：
//! - **Ready 函数**：逻辑已在本模块（薄编排，全部使用 engine 原语）；
//! - **Planned 函数**（`start_chat` / `compact_session` /
//!   `resume_followup_queue`）：签名与契约已定，函数体待平移旧壳
//!   chat.rs 的 run 装配树（约 3000 行：模型解析、压缩编排、审计、
//!   标题任务、followUp 自动投递、EventfulSink 记账事件层、
//!   HostRequestRefresher 每拍刷新）——**目标切片 = 阶段 2.C 2.11「帧流」**
//!   （spec：engine start_chat → 帧消费 → 64ms 节流；abort 桥 cancel）。
//!   调用这三个函数当前 `todo!()` panic——切片对着签名开发的假数据
//!   装置不受影响（原则 6：切片测试不依赖其他切片完成）。
//!
//! 帧下发端约定：旧壳 `Channel<SequencedFrame>` 换成
//! [`SequencedSink`] 参数（GPUI 壳实现它把帧推入 Entity 触发重渲染）。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agent::RunManager;
use crate::app_state::AppState;
use crate::backend::ask::{AskAnswer, AskPayload};
use crate::backend::frames::{RunSnapshot, SequencedSink};
use crate::followup::{QueuedImageAttachment, QueuedRunConfig};
use crate::safety::tier::PermissionTier;
use crate::storage::MessageEntry;
use crate::util::errors::IpcError;
use crate::util::events::keys;

/// maxTurns 缺省值：单次 run 最多跑 24 个来回（防止模型无限循环烧钱）。
pub const DEFAULT_MAX_TURNS: u32 = 24;

/// RunConfig（旧壳 chat.rs 契约镜像）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    /// 会话级覆盖；缺省全局 active。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub model_id: Option<String>,
    /// 会话 thinking 档：本次 run 的起点档位。运行中以后端
    /// prefs 文件为权威（每拍请求前刷新）；None = 未显式设置，按
    /// per-model reasoningEffort 回落（优先级链）。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub thinking: Option<crate::providers::reasoning::ThinkingLevel>,
    pub tier: PermissionTier,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    /// steer 注入（同进程直调恒不传 true，注入走 `inject_run_message`）。
    #[serde(default)]
    pub steer: bool,
}

/// maxTurns 缺省值函数（serde default 载体）。
fn default_max_turns() -> u32 {
    DEFAULT_MAX_TURNS
}

/// `start_chat` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StartChatArgs {
    pub session_id: String,
    pub messages: Vec<MessageEntry>,
    pub run_config: RunConfig,
}

/// `start_chat` 返回契约（`{ runId }`）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StartChatResult {
    pub run_id: String,
}

/// `inject_run_message` 的注入语义：steer = 运行中插话
/// （有活跃 run 时注入当前 run，turn 边界消费）；followUp = 排队（任何时刻
/// 可排，run 正常完成后自动投递）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum InjectMode {
    Steer,
    FollowUp,
}

/// `inject_run_message` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct InjectRunMessageArgs {
    pub session_id: String,
    pub text: String,
    /// 注入语义；缺省按 steer 处理（无活跃 run 时自动落 followUp）。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub mode: Option<InjectMode>,
    /// followUp 附着的运行配置快照（投递新 run 的 RunConfig 来源；缺省回退
    /// 默认档位组合）。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub run_config: Option<RunConfig>,
    /// 图片附件引用：非空时**不入 steer**——按
    /// 带图消息规则转会话级 followUp 队列（引用已在 attach_image 落盘）。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub attachments: Option<Vec<QueuedImageAttachment>>,
}

/// `inject_run_message` 返回契约：实际入队的通道。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct InjectRunMessageResult {
    pub queued: bool,
    /// 实际接纳通道："steer" = 已被运行中 run 接纳；"followUp" = 已入会话级
    /// 待发队列。queued=false（未接纳）时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub mode: Option<InjectMode>,
    /// followUp 模式下返回队列条目 id（队列条的删除/编辑取出回传键）；
    /// steer 模式省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub entry_id: Option<String>,
}

/// `resume_followup_queue` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ResumeFollowupQueueArgs {
    pub session_id: String,
}

/// `resume_followup_queue` 返回契约：delivered=true 表示解除
/// 暂停后立即投递了队列头并新开 run（runId 带值）。
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ResumeFollowupQueueResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub run_id: Option<String>,
    pub delivered: bool,
}

/// `remove_followup` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RemoveFollowupArgs {
    pub session_id: String,
    pub entry_id: String,
}

/// `remove_followup` 返回契约：移除后的队列长度。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RemoveFollowupResult {
    pub size: usize,
}

/// `resolve_ask` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ResolveAskArgs {
    pub ask_id: String,
    pub answer: AskAnswer,
}

/// `approve_plan` 参数契约（计划门批准快照冻结）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ApprovePlanArgs {
    pub session_id: String,
}

/// `compact_session` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CompactSessionArgs {
    pub session_id: String,
}

/// `compact_session` 返回契约：cutEntryId 缺省 = 无可压内容。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CompactSessionResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub cut_entry_id: Option<String>,
    pub tokens_before: u64,
    pub tokens_after: u64,
}

/// `attach_run` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AttachRunArgs {
    pub session_id: String,
    pub since_seq: u64,
}

/// `attach_run` 返回契约：runId 省略 = 无活跃 run（空重放）。
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AttachRunResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub run_id: Option<String>,
    pub replayed: u64,
}

/// `session_run_snapshot` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionRunSnapshotArgs {
    pub session_id: String,
}

/// `session_pending_ask` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionPendingAskArgs {
    pub session_id: String,
}

/// `session_pending_ask` 的返回体：仍有效的未决询问/审批（原始载荷随行）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionPendingAsk {
    pub ask_id: String,
    pub run_id: String,
    /// 载荷形态（"question" / "approval"）。
    pub kind: String,
    /// 开启时的原始载荷（approval 含 diff/命令/围栏原因与选项）。
    pub payload: AskPayload,
}

/// 发起一次 run（旧命令 `start_chat`）。
///
/// **Planned**（阶段 2.C 2.11 帧流切片平移旧壳 chat.rs `start_chat_core`
/// 及其装配树）：会话偏好起点快照 → 模型解析（RunConfig.modelId >
/// AppConfig.activeModelId，免 Key 本地供应商占位）→ 图片 blob 装配 →
/// wire 校验 → 系统前缀与压缩编排（customPrompt → summary → 计划 →
/// 技能清单）→ 工具集（档位过滤）→ RunManager 注册（重复启动
/// E_SESSION_RUNNING）→ spawn `backend::run_loop::drive_run`；
/// 审批挂起恢复循环（`RunExit::Suspended` → AskSlot + ask:opened →
/// `resolve_ask`/取消/autoConfirm 300s）；suggest 后台小调用；
/// `EventfulSink` 事件层（usage 帧 → 会话累计 + tokens:update + 记账三面）
/// 与每拍 prefs 刷新（`HostRequestRefresher`）。
///
/// `sink`：帧下发端（seq/ts 由 FrameHub 统一填写后经它下发）。
/// `auto_confirm`：审批自动放行开关（读 `AppConfig.approval.autoConfirm`）。
pub async fn start_chat(
    _app: &AppState,
    _runs: &RunManager,
    _args: StartChatArgs,
    _sink: Box<dyn SequencedSink>,
    _auto_confirm: bool,
) -> Result<StartChatResult, IpcError> {
    todo!("阶段 2.C 2.11 帧流切片平移旧壳 chat.rs start_chat_core 装配树（见模块文档 Planned 约定）")
}

/// 取消会话活跃 run（旧命令 `cancel_run`）：取消令牌置位即返回，
/// 真正的收尾由 run 任务自己完成（补发取消终态帧）。
/// 返回是否有活跃 run 被置位（false = 无活跃 run，幂等）。
pub fn cancel_run(app: &AppState, session_id: &str) -> Result<bool, IpcError> {
    Ok(app.runs.cancel(session_id))
}

/// 发送中插话 / 待发队列（旧命令 `inject_run_message`）：
/// 按 `mode` 与活跃状态分叉——运行中 steer 入 RunHandle 插话队列（turn
/// 边界由 SteerSource 消费：出队 + canonical 落盘 + `run:inject` 回执）；
/// followUp 入会话级队列（`sessions/<sid>.queue.json` 持久化），run 正常
/// 完成后自动投递；带图消息恒走 followUp（插话队列只承载文本）。
pub fn inject_run_message(app: &AppState, args: InjectRunMessageArgs) -> Result<InjectRunMessageResult, IpcError> {
    if args.session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    if args.text.trim().is_empty() {
        return Err(IpcError::invalid_arg("text 不能为空"));
    }
    let attachments = args.attachments.unwrap_or_default();
    let handle = app.runs.get(&args.session_id);
    // steer：运行中插话（RunHandle 插话队列，RunLoop turn 边界消费）。
    // 带图例外：插话队列只承载文本，attachments 非空时跳过 steer
    // 落入下方 followUp 路径（运行中也会进队列——run 正常完成后自动投递）。
    if attachments.is_empty()
        && args.mode != Some(InjectMode::FollowUp)
        && let Some(handle) = handle
    {
        handle
            .steer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(args.text.clone());
        return Ok(InjectRunMessageResult {
            queued: true,
            mode: Some(InjectMode::Steer),
            entry_id: None,
        });
    }
    // followUp：会话级待发队列（持久化 + run 正常完成后自动投递）。
    // 附着 runConfig 快照：显式入参优先，缺省回退默认档位组合。
    let effective = args.run_config.clone().unwrap_or(RunConfig {
        model_id: None,
        thinking: None,
        tier: PermissionTier::AutoEdit,
        max_turns: DEFAULT_MAX_TURNS,
        steer: false,
    });
    let entry = followup_entry_from_text(&args.text, &effective, attachments);
    let entry_id = entry.id.clone();
    app.followups.push(&args.session_id, entry);
    emit_queue_state(&app.events, &args.session_id, &app.followups, Vec::new());
    Ok(InjectRunMessageResult {
        queued: true,
        mode: Some(InjectMode::FollowUp),
        entry_id: Some(entry_id),
    })
}

/// 从纯文本构造 followUp 待发条目（runConfig 快照随行——投递时按它开新
/// run；附件引用由调用方挂入）。
fn followup_entry_from_text(
    text: &str,
    run_config: &RunConfig,
    attachments: Vec<QueuedImageAttachment>,
) -> crate::followup::FollowUpEntry {
    crate::followup::FollowUpEntry {
        id: format!("fu-{}", uuid::Uuid::new_v4()),
        text: text.to_owned(),
        created_at: crate::storage::trust::now_ms(),
        run_config: QueuedRunConfig {
            model_id: run_config.model_id.clone(),
            thinking: run_config.thinking,
            tier: run_config.tier,
            max_turns: run_config.max_turns,
        },
        attachments,
    }
}

/// `run:queue` 事件发射：载荷 = 快照 + 可选
/// `returned` 交还文本数组（取消清空/失败交还时出现）。
pub(crate) fn emit_queue_state(
    events: &Arc<dyn crate::util::events::EventSink>,
    session_id: &str,
    followups: &crate::followup::FollowUpManager,
    returned: Vec<String>,
) {
    let snapshot = followups.snapshot(session_id);
    let mut payload = serde_json::json!({
        "sessionId": session_id,
        "size": snapshot.size,
        "paused": snapshot.paused,
    });
    if !returned.is_empty() {
        payload["returned"] = serde_json::json!(returned);
    }
    events.emit(keys::RUN_QUEUE, payload);
}

/// 解除失败暂停并投递队列头（旧命令 `resume_followup_queue`）。
///
/// **Planned**（随 start_chat 同切片平移）：`set_paused(false)` + `run:queue`
/// 事件后，若无活跃 run 且队列非空 → 投递队列头（读条目 → `start_chat`
/// 开新 run，runId 带值）。投递未发生（竞态暂停/队列空）或失败（已回队/
/// 暂停并广播）不算命令错误（delivered=false）。
pub async fn resume_followup_queue(
    app: &AppState,
    args: ResumeFollowupQueueArgs,
) -> Result<ResumeFollowupQueueResult, IpcError> {
    let _ = (app, args);
    todo!("阶段 2.C 2.11 帧流切片随 start_chat 平移（旧壳 chat.rs resume_followup_queue_core + deliver_followup_head）")
}

/// 移除一条待发消息（队列条删除与「编辑取出」共用；旧命令
/// `remove_followup`）：移除后广播 `run:queue`；条目不在队列 → E_NOT_FOUND。
pub fn remove_followup(app: &AppState, args: RemoveFollowupArgs) -> Result<RemoveFollowupResult, IpcError> {
    if args.session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    app.followups
        .remove(&args.session_id, &args.entry_id)
        .ok_or_else(|| IpcError::not_found(format!("待发消息不在队列：{}", args.entry_id)))?;
    emit_queue_state(&app.events, &args.session_id, &app.followups, Vec::new());
    Ok(RemoveFollowupResult {
        size: app.followups.snapshot(&args.session_id).size,
    })
}

/// ask 应答投递（旧命令 `resolve_ask`）：主 run 槽未命中时依次落
/// 子代理槽（活跃子代理审批挂起）与应用级询问槽（MCP TOFU / 技能
/// projectTrust，`AppState.app_asks`）。槽位清理与 ask:closed 事件由
/// 等待方单一出口发出（本函数只投递）。
pub async fn resolve_ask(app: &AppState, args: ResolveAskArgs) -> Result<(), IpcError> {
    match resolve_run_ask(&app.runs, &args.ask_id, args.answer.clone()).await {
        Ok(()) => Ok(()),
        Err(err) if err.code == crate::util::errors::E_NOT_FOUND => {
            // 子代理槽：活跃子代理的审批挂起
            if let Some(sub) = app.subs.find_by_ask(&args.ask_id) {
                let tx = sub
                    .ask
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    .map(|slot| slot.tx);
                if let Some(tx) = tx
                    && tx.send(args.answer.clone()).is_ok()
                {
                    return Ok(());
                }
            }
            if app.resolve_app_ask(&args.ask_id, args.answer) {
                Ok(())
            } else {
                Err(err)
            }
        }
        Err(err) => Err(err),
    }
}

/// 主 run 槽的应答投递（旧壳 `resolve_ask_core` 平移）。
async fn resolve_run_ask(
    runs: &RunManager,
    ask_id: &str,
    answer: AskAnswer,
) -> Result<(), IpcError> {
    let Some(handle) = runs.find_by_ask(ask_id) else {
        return Err(IpcError::not_found(format!("询问不存在或已关闭：{ask_id}")));
    };
    let mut slot = handle
        .ask
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(current) = slot.take() else {
        return Err(IpcError::not_found(format!("询问不存在或已关闭：{ask_id}")));
    };
    if current.tx.send(answer).is_err() {
        // 等待方已退出（run 终止竞态）
        return Err(IpcError::not_found(format!("询问已失效：{ask_id}")));
    }
    Ok(())
}

/// 批准当前计划（旧命令 `approve_plan`）：冻结当前计划为批准快照与写范围
/// 基线；冻结后同键补发一次 `plan:update` 全量计划态——计划卡/Inspector
/// 展示基线标记与待批准范围的数据面。
pub fn approve_plan(app: &AppState, args: ApprovePlanArgs) -> Result<(), IpcError> {
    let Some(handle) = app.runs.get(&args.session_id) else {
        return Err(IpcError::not_found(format!(
            "会话无活跃 run：{}",
            args.session_id
        )));
    };
    let payload = {
        let mut plan = handle
            .plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = plan.as_mut() else {
            return Err(IpcError::invalid_arg("尚无计划可批准（等模型先建计划）"));
        };
        state.approve(crate::storage::trust::now_ms());
        serde_json::json!({
            "sessionId": args.session_id,
            "items": state.items,
            "paths": state.paths,
            "approved": state.is_approved(),
            "baselinePaths": state.baseline_paths,
        })
    };
    app.events.emit(keys::PLAN_UPDATE, payload);
    Ok(())
}

/// 手动上下文压缩（旧命令 `compact_session`）。
///
/// **Planned**（随 start_chat 同切片平移旧壳 chat.rs
/// `compact_session_core`）：模型解析 → 压缩策略（`compaction_policy`）→
/// 历史边界解析 → 压缩范围计算（`backend::compaction::compression_scope`）
/// → 摘要模型执行压缩（`execute_compaction`）→ 前后 token 估算返回。
/// 会话运行中拒绝（E_SESSION_RUNNING）。
pub async fn compact_session(
    app: &AppState,
    args: CompactSessionArgs,
) -> Result<CompactSessionResult, IpcError> {
    let _ = (app, args);
    todo!("阶段 2.C 2.11 帧流切片随 start_chat 平移（旧壳 chat.rs compact_session_core + compaction_policy + execute_compaction）")
}

/// 帧重放与热重连（旧命令 `attach_run`）：重放 `(sinceSeq, 当前头]` 区间帧
/// 并把帧通道热切换到新下发端（重放窗口耗尽时补一次 runSnapshot 摘要帧）。
/// 幂等查询语义：无活跃 run / sessionId 未知 → 空重放（runId 省略），
/// 不报 E_NOT_FOUND。
pub fn attach_run(
    app: &AppState,
    args: AttachRunArgs,
    out: Box<dyn SequencedSink>,
) -> Result<AttachRunResult, IpcError> {
    let Some(handle) = app.runs.get(&args.session_id) else {
        return Ok(AttachRunResult::default());
    };
    let outcome = handle.attach(args.since_seq, out);
    Ok(AttachRunResult {
        run_id: Some(handle.run_id.clone()),
        replayed: outcome.replayed,
    })
}

/// 运行时状态对账（旧命令 `session_run_snapshot`）：会话有活跃 run →
/// 运行时快照；无活跃 run / sessionId 未知 → None（幂等查询语义）。
pub fn session_run_snapshot(
    app: &AppState,
    args: SessionRunSnapshotArgs,
) -> Result<Option<RunSnapshot>, IpcError> {
    Ok(app.runs.get(&args.session_id).map(|h| h.run_snapshot()))
}

/// 待审批恢复查询（旧命令 `session_pending_ask`）：返回该会话仍
/// 有效的未决询问/审批（原始载荷随行）；有效性以后端 ask 槽为权威——
/// 槽位收口（已答/已取消/超时）即查不到；无未决 ask → None（幂等）。
pub fn session_pending_ask(
    app: &AppState,
    args: SessionPendingAskArgs,
) -> Result<Option<SessionPendingAsk>, IpcError> {
    let Some(handle) = app.runs.get(&args.session_id) else {
        return Ok(None);
    };
    let slot = handle
        .ask
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(ask) = slot.as_ref() else {
        return Ok(None);
    };
    Ok(Some(SessionPendingAsk {
        ask_id: ask.ask_id.clone(),
        run_id: ask.run_id.clone(),
        kind: ask.kind.to_owned(),
        payload: ask.payload.clone(),
    }))
}
