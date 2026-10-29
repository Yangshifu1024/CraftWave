//! start_chat / cancel_run / resolve_ask / approve_plan 命令
//! （§3.3 #18/#19/#22/#26c，M1 单会话核心链路 + 14 号票审批接线）。
//!
//! - 命令层只做校验/转调与装配（§2.2）：模型解析（§11.1：RunConfig.modelId >
//!   AppConfig.activeModelId）→ wire 切分（§6.2）→ 工具集（§8.1 档位过滤）→
//!   RunManager 注册 → spawn RunLoop 任务（`backend::run_loop::drive_run`）；
//! - **审批挂起恢复循环**（14 号票，§5.2/§5.4）：`RunExit::Suspended` → 构造
//!   `AskPayload(kind=approval)` → 注册 AskSlot + `ask:opened` 事件 → 等待
//!   `resolve_ask`/取消/autoConfirm 300s → allow 则 `ApprovalGate::approve`
//!   按签名放行 + `resume_run` 续跑（DriveState 延续：不重发 runStarted、
//!   usage/计数不回退）；deny/取消 → 挂起调用补拒绝结果帧 + runEnded(cancelled)；
//! - **suggest 后台小调用**（C-9）：run Done 后以同模型一次非流式调用生成
//!   建议提示词 → `run:suggestions` 事件；失败静默、用量计入会话累计；
//! - 帧出口：tauri `Channel` 适配为 `FrameSink`（seq + ts），外包事件层
//!   （usage 帧 → 会话累计 + `tokens:update` 事件全量校正）；
//! - provider 只接 openai_chat 方言（§11.1），Key 经 keyring 读取；
//! - 错误终态兜底：RunLoop 内部失败（PromptError）由 run 任务发
//!   `runEnded(error)`（§11.3 分类器为 provider 适配层后续票，本票 E_INTERNAL）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rig::completion::request::ToolDefinition;
use rig::message::ToolName;
use rig::providers::openai::wire::{Chat, OpenAIConfig};
use rig::run::AgentRun;
use rig::run::spec::RunSpec;
use serde::{Deserialize, Serialize};
use tauri::State;
use tauri::ipc::Channel;

use crate::agent::{RunHandle, RunManager};
use crate::backend::ask::{
    ApprovalDiff, AskAnswer, AskMode, AskOption, AskPayload, AskSlot, DiffHunk, DiffLine,
    DiffLineKind,
};
use crate::backend::exec::{ExecutorEnv, ToolsetExecutor};
use crate::backend::frames::{
    ChatFrame, EndReason, FrameError, FrameSink, SequencedFrame, UsageTotal,
};
use crate::backend::gate::ApprovalGate;
use crate::backend::run_loop::{DriveState, PendingCall, RunExit, drive_run, resume_run};
use crate::backend::turn::TurnDriver;
use crate::host::AppState;
use crate::host::run_persistence::{PersistingSink, RunRecorder};
use crate::providers::types::{ApiFormat, ProviderConfig, ProviderModel};
use crate::safety::tier::PermissionTier;
use crate::storage::{MessageEntry, MessageRole, StoredEntry};
use crate::util::errors::{E_INTERNAL, IpcError};
use crate::util::events::EventSink;
use crate::util::events::keys;

use crate::backend::run_loop::E_CANCELLED;

/// autoConfirm 自动放行等待（§5.4：300s 超时自动选 recommended 项）。
pub const AUTO_CONFIRM_TIMEOUT: Duration = Duration::from_secs(300);

/// `start_chat` 参数契约（§3.3 #18；文档命名锚点，命令形参按字段平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StartChatArgs {
    pub session_id: String,
    pub messages: Vec<MessageEntry>,
    pub run_config: RunConfig,
}

/// §3.2 RunConfig（Rust 侧镜像）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    /// 会话级覆盖；缺省全局 active。
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub model_id: Option<String>,
    pub tier: PermissionTier,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    /// steer 注入随 §5.5 后续票（本票前端恒不传 true）。
    #[serde(default)]
    pub steer: bool,
}

/// maxTurns 缺省值：单次 run 最多跑 24 个来回（防止模型无限循环烧钱）。
fn default_max_turns() -> u32 {
    24
}

/// `cancel_run` 参数契约（§3.3 #19）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CancelRunArgs {
    pub session_id: String,
}

/// `resolve_ask` 参数契约（§3.3 #22）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ResolveAskArgs {
    pub ask_id: String,
    pub answer: AskAnswer,
}

/// `approve_plan` 参数契约（§3.3 #26c，14 号票增补：计划门批准快照冻结）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ApprovePlanArgs {
    pub session_id: String,
}

/// `start_chat` 返回契约（§3.3 #18：`{ runId }`）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StartChatResult {
    pub run_id: String,
}

/// 解析后的运行模型（§11.1 ResolvedModel 的当前子集）。
pub(crate) struct ResolvedModel {
    /// 显示名（runStarted.model，= wire id）。
    pub name: String,
    pub driver: rig::driver::Model<Chat>,
    /// 上下文窗口 token 数（`ProviderModel.contextWindow`，§3.2 缺省 128000；
    /// 压缩预算检查的窗口面，§6.2）。
    pub context_window: u64,
    /// 单次回复 token 上限（`ProviderModel.maxTokens`，§3.2 缺省 32768；
    /// 每请求预算的回复预留面，§6.2 Q18）。
    pub max_tokens: u64,
}

/// §11.1 模型解析：RunConfig.modelId > AppConfig.activeModelId →
/// (provider, model) → rig OpenAI 兼容客户端（baseUrl 可覆写）。
pub(crate) fn resolve_model(
    app: &AppState,
    model_id: Option<&str>,
) -> Result<ResolvedModel, IpcError> {
    let config = app.config.load()?;
    let wanted = model_id.or(config.active_model_id.as_deref());
    let Some(wanted) = wanted else {
        return Err(IpcError::invalid_arg(
            "未指定模型：RunConfig.modelId 与 AppConfig.activeModelId 均为空",
        ));
    };
    let mut found: Option<(&ProviderConfig, &ProviderModel)> = None;
    for provider in &config.providers {
        for model in &provider.models {
            if model.id == wanted {
                found = Some((provider, model));
            }
        }
    }
    let Some((provider, model)) = found else {
        return Err(IpcError::invalid_arg(format!(
            "activeModelId 无对应模型：{wanted}"
        )));
    };
    if provider.api_format != ApiFormat::OpenaiChat {
        return Err(IpcError::invalid_arg(format!(
            "M1 仅支持 openai_chat 方言（当前 {:?}）",
            provider.api_format
        )));
    }
    let Some(key) = provider.api_key.as_deref() else {
        return Err(IpcError::invalid_arg(format!(
            "供应商 {} 未配置 API Key（设置页填写后保存）",
            provider.id
        )));
    };
    let wire_config = OpenAIConfig::new(key).with_base_url(
        provider
            .base_url
            .clone()
            .unwrap_or_else(|| provider.api_format.official_base_url().to_owned()),
    );
    let driver = wire_config.client().chat(model.model.clone());
    Ok(ResolvedModel {
        name: model.model.clone(),
        driver,
        context_window: u64::from(model.context_window),
        max_tokens: u64::from(model.max_tokens),
    })
}

/// ToolSpec → rig ToolDefinition（广播给模型的工具表，§8.1）。
fn to_rig_tool_definitions(
    specs: Vec<crate::tools::ToolSpec>,
) -> Result<Vec<Arc<ToolDefinition>>, IpcError> {
    specs
        .into_iter()
        .map(|spec| {
            let name = ToolName::new(spec.name.to_owned())
                .map_err(|e| IpcError::invalid_arg(format!("非法工具名 {}: {e}", spec.name)))?;
            Ok(Arc::new(ToolDefinition::new(
                name,
                spec.description,
                spec.params_schema,
            )))
        })
        .collect()
}

/// tauri `Channel` → `FrameSink` 适配（§3.4 帧基座出口：seq 递增 + ts = epoch ms）。
struct ChannelSink {
    channel: Channel<SequencedFrame>,
    seq: u64,
}

impl FrameSink for ChannelSink {
    fn send(&mut self, frame: ChatFrame) {
        let seq = self.seq;
        self.seq += 1;
        let sequenced = SequencedFrame {
            seq,
            ts: Some(now_ms()),
            frame,
        };
        // 前端掉线（webview 重载）时 Channel 发送失败：M1 只读快照语义（§5.6），
        // run 继续执行，帧丢弃。
        let _ = self.channel.send(sequenced);
    }
}

/// 帧出口的事件层（14 号票）：usage 帧增量折算 → 会话累计 →
/// `tokens:update` 全量校正事件（§3.5/§10.5：sessionUsage 以事件为准）。
/// usage 帧载荷是 run 内累计值，须减去上一帧的 run 累计得本轮增量。
struct EventfulSink {
    inner: Box<dyn FrameSink>,
    events: Arc<dyn EventSink>,
    session_id: String,
    last_run_total: UsageTotal,
    session_usage: Arc<Mutex<HashMap<String, UsageTotal>>>,
}

impl EventfulSink {
    /// 把本轮增量累进共享的会话用量表，并发出全量快照事件
    /// （前端只认事件里的全量值，不自己加总）。
    fn send_tokens_update(&mut self, delta: &UsageTotal) {
        let mut map = self
            .session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let total = map.entry(self.session_id.clone()).or_default();
        total.accumulate(delta);
        let snapshot = *total;
        self.events.emit(
            keys::TOKENS_UPDATE,
            serde_json::json!({ "sessionId": self.session_id, "usage": snapshot }),
        );
    }
}

impl FrameSink for EventfulSink {
    fn send(&mut self, frame: ChatFrame) {
        if let ChatFrame::Usage { usage, .. } = &frame {
            let delta = sub_usage(usage, &self.last_run_total);
            self.last_run_total = *usage;
            self.send_tokens_update(&delta);
        }
        self.inner.send(frame);
    }
}

/// usage 帧的 run 累计差分（UsageTotal 无 Sub）。
fn sub_usage(a: &UsageTotal, b: &UsageTotal) -> UsageTotal {
    UsageTotal {
        input: a.input.saturating_sub(b.input),
        output: a.output.saturating_sub(b.output),
        cache_read: a.cache_read.saturating_sub(b.cache_read),
        cache_write: a.cache_write.saturating_sub(b.cache_write),
    }
}

/// 当前时间的 Unix 毫秒时间戳（取不到时兜底 0）。
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 发送错误终态帧（RunLoop 冒出的 PromptError 兜底，保证前端必有终态）。
/// dev 构建同时打到进程终端（tauri dev 可见），便于排障（2026-10-28 优化项）。
fn send_error_ended(sink: &mut dyn FrameSink, message: String) {
    #[cfg(debug_assertions)]
    eprintln!("[craftwave] run 错误终态：{message}");
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Error,
        duration_ms: 0,
        tool_count: 0,
        turn: 1,
        error: Some(FrameError::new(E_INTERNAL, message)),
    });
}

/// 审批挂起 → AskPayload(kind=approval)：write=新文件全 add、edit=替换 hunk、
/// command=命令行 + fenceReason（§3.2）。
pub(crate) fn build_approval_payload(pending: &PendingCall) -> AskPayload {
    let args: serde_json::Value = serde_json::from_str(&pending.args_json).unwrap_or_default();
    let title = match pending.name.as_str() {
        "write" => format!(
            "写入新文件 {}",
            args.get("path").and_then(|v| v.as_str()).unwrap_or("?")
        ),
        "edit" => format!(
            "编辑 {}",
            args.get("path").and_then(|v| v.as_str()).unwrap_or("?")
        ),
        "command" => "执行命令".to_owned(),
        other => format!("执行工具 {other}"),
    };
    let mut diff = None;
    let mut command = None;
    match pending.name.as_str() {
        "write" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            diff = Some(ApprovalDiff {
                path: path.to_owned(),
                hunks: vec![DiffHunk {
                    header: "新文件".to_owned(),
                    lines: content
                        .lines()
                        .map(|l| DiffLine {
                            kind: DiffLineKind::Add,
                            text: l.to_owned(),
                        })
                        .collect(),
                }],
            });
        }
        "edit" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let old = args.get("oldText").and_then(|v| v.as_str()).unwrap_or("");
            let new = args.get("newText").and_then(|v| v.as_str()).unwrap_or("");
            let mut lines = Vec::new();
            for l in old.lines() {
                lines.push(DiffLine {
                    kind: DiffLineKind::Del,
                    text: l.to_owned(),
                });
            }
            for l in new.lines() {
                lines.push(DiffLine {
                    kind: DiffLineKind::Add,
                    text: l.to_owned(),
                });
            }
            diff = Some(ApprovalDiff {
                path: path.to_owned(),
                hunks: vec![DiffHunk {
                    header: format!(
                        "@@ -{del} +{add} @@",
                        del = old.lines().count(),
                        add = new.lines().count()
                    ),
                    lines,
                }],
            });
        }
        "command" => {
            command = args
                .get("command")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
        }
        _ => {}
    }
    AskPayload::Approval {
        title,
        diff,
        command,
        fence_reason: pending.fence_reason.clone(),
        options: vec![
            AskOption {
                id: "allow_once".to_owned(),
                label: "允许".to_owned(),
                recommended: Some(true),
                mode: Some(AskMode::AllowOnce),
            },
            AskOption {
                id: "deny".to_owned(),
                label: "拒绝".to_owned(),
                recommended: None,
                mode: Some(AskMode::Deny),
            },
        ],
    }
}

/// 挂起等待的裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalResolution {
    Allowed,
    Denied,
}

/// 挂起等待审批：注册 AskSlot + ask:opened → 等 resolve_ask / 取消 /
/// autoConfirm 超时（§5.4）。等待结束统一清槽 + ask:closed。
#[allow(clippy::too_many_arguments)]
async fn wait_approval(
    events: &Arc<dyn EventSink>,
    handle: &Arc<RunHandle>,
    pending: &PendingCall,
    auto_confirm: bool,
) -> ApprovalResolution {
    let (tx, rx) = tokio::sync::oneshot::channel::<AskAnswer>();
    let ask_id = uuid::Uuid::new_v4().to_string();
    {
        let mut slot = handle
            .ask
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 串行执行下不应有并存 ask；防御性拒绝（工具侧同口径）
        if slot.is_some() {
            return ApprovalResolution::Denied;
        }
        *slot = Some(AskSlot {
            ask_id: ask_id.clone(),
            session_id: handle.session_id.clone(),
            run_id: handle.run_id.clone(),
            kind: "approval",
            tx,
        });
    }
    let payload = build_approval_payload(pending);
    events.emit(
        keys::ASK_OPENED,
        serde_json::json!({
            "sessionId": handle.session_id,
            "askId": ask_id,
            "payload": payload,
        }),
    );

    // 挂起的三种醒来方式（select 多路等待的结果归一化）
    enum Wait {
        /// 前端 resolve_ask 送回了应答（None 表示发送端已丢弃）。
        Answer(Option<AskAnswer>),
        /// 用户点了取消（run 的取消令牌触发）。
        Cancelled,
        /// autoConfirm 开启且等满 300 秒无人回应。
        AutoConfirm,
    }
    let wait = tokio::select! {
        biased;
        _ = handle.cancel.cancelled() => Wait::Cancelled,
        _ = tokio::time::sleep(AUTO_CONFIRM_TIMEOUT), if auto_confirm => Wait::AutoConfirm,
        answer = rx => Wait::Answer(answer.ok()),
    };
    let (resolution, answered) = match wait {
        Wait::Answer(Some(AskAnswer::Approval { option_id, .. })) => {
            let allowed = option_id == "allow_once";
            (
                if allowed {
                    ApprovalResolution::Allowed
                } else {
                    ApprovalResolution::Denied
                },
                true,
            )
        }
        // 非审批形态应答（问卷）按拒绝收场
        Wait::Answer(Some(_)) => (ApprovalResolution::Denied, true),
        // 发送端已 drop：run 侧已收口，无需再发终态（返回 Denied 由循环兜底）
        Wait::Answer(None) => (ApprovalResolution::Denied, true),
        Wait::Cancelled => (ApprovalResolution::Denied, false),
        // autoConfirm：300s 自动选 recommended（allow_once）——挂起必非黑名单
        // （Block 不会挂起），灾难级例外天然满足（§5.4）
        Wait::AutoConfirm => (ApprovalResolution::Allowed, true),
    };
    // 单一出口清槽 + ask:closed（resolve_ask 只投递不清理）
    *handle
        .ask
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    events.emit(
        keys::ASK_CLOSED,
        serde_json::json!({ "askId": ask_id, "answered": answered }),
    );
    resolution
}

/// 拒绝/取消的收场：挂起调用补拒绝结果帧 + runEnded(cancelled)（§5.3
/// AwaitingApproval → Cancelled；run 已序列化，模型侧不再收下一轮，帧面
/// 补 E_CANCELLED 结果让前端工具卡不悬空）。
fn denied_finish(sink: &mut dyn FrameSink, pending: &PendingCall, state: &DriveState) {
    let frame_call_id = state
        .model_call_ids
        .get(&pending.model_call_id)
        .cloned()
        .unwrap_or_else(|| pending.model_call_id.clone());
    sink.send(ChatFrame::ToolResult {
        call_id: frame_call_id,
        ok: false,
        ui_summary: "用户已拒绝".to_owned(),
        error: Some(FrameError::new(E_CANCELLED, "用户拒绝了此操作")),
    });
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Cancelled,
        duration_ms: state
            .started_at
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0),
        tool_count: state.tool_count,
        turn: state.last_turn,
        error: None,
    });
}

/// suggest 后台小调用（C-9）：一次非流式补全 → run:suggestions；失败静默、
/// 空文本静默；用量计入会话累计（tokens:update，不计 turnUsage——run 已终态）。
fn spawn_suggest(
    driver: TurnDriver,
    events: Arc<dyn EventSink>,
    session_usage: Arc<Mutex<HashMap<String, UsageTotal>>>,
    session_id: String,
    run_id: String,
    context_tail: String,
) {
    tokio::spawn(async move {
        if context_tail.trim().is_empty() {
            return;
        }
        let prompt = format!(
            "根据下面这段对话结尾，生成一句简短的中文后续输入建议（ Composer 预填文案，\
             10–20 字，直接输出建议本身，不要引号和解释）：\n{context_tail}"
        );
        let Ok((text, usage)) = driver.complete_once(&prompt).await else {
            return; // 失败静默放弃（C-9：不重试、不提示）
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        // 用量计入会话累计（帧通道已收，走事件通道补计）
        let delta = crate::backend::turn::usage_total(&usage);
        let total = {
            let mut map = session_usage
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let total = map.entry(session_id.clone()).or_default();
            total.accumulate(&delta);
            *total
        };
        events.emit(
            keys::TOKENS_UPDATE,
            serde_json::json!({ "sessionId": session_id, "usage": total }),
        );
        events.emit(
            keys::RUN_SUGGESTIONS,
            serde_json::json!({ "sessionId": session_id, "runId": run_id, "text": trimmed }),
        );
    });
}

/// 会话消息尾文本（suggest 上下文）：最后一条 assistant 的文本（缺省取
/// 最后一条 user）。
pub(crate) fn context_tail(messages: &[MessageEntry]) -> String {
    let text_of = |entry: &MessageEntry| {
        entry
            .parts
            .iter()
            .filter_map(|p| match p {
                crate::storage::WirePart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>()
    };
    messages
        .iter()
        .rev()
        .find(|m| m.role == crate::storage::MessageRole::Assistant)
        .map(text_of)
        .filter(|t| !t.trim().is_empty())
        .or_else(|| messages.last().map(text_of))
        .unwrap_or_default()
}

/// 压缩运行时策略快照（§6.2：配置 + 模型窗口 + 回复预留，04 号票）。
#[derive(Debug, Clone)]
pub(crate) struct CompactionRuntimePolicy {
    /// 自动压缩阈值（窗口占比，§3.2 compaction.threshold）。
    pub threshold: f64,
    /// 压缩总超时（分块共用，§6.2 Q31）。
    pub timeout: Duration,
    /// 当前模型上下文窗口（§3.2 contextWindow）。
    pub context_window: u64,
    /// 回复预留（§3.2 maxTokens，Q18 预算组成）。
    pub reply_reserve: u64,
    /// 摘要模型的上下文窗口（分块装箱面；专用小模型未配时 = 当前模型窗口）。
    pub summary_window: u64,
}

/// 压缩结果（提交面）：新压缩边界（tokensBefore/tokensAfter 由调用方按
/// 各自触发点的估算基准填入帧与事件载荷）。
#[derive(Debug, Clone)]
pub(crate) struct CompactionOutcome {
    pub boundary: crate::backend::wire::CompactionBoundary,
}

/// 从 AppConfig 读压缩策略（模型窗口取当前 run 模型；摘要专用模型未配时
/// 摘要窗口 = 当前模型窗口）。
pub(crate) fn compaction_policy(app: &AppState, model: &ResolvedModel) -> CompactionRuntimePolicy {
    let config = app
        .config
        .load()
        .map(|c| c.compaction.clone())
        .unwrap_or_default();
    // 摘要专用小模型未配置时跟随当前模型（§6.2），窗口同取当前模型
    let summary_window = config
        .model_id
        .as_deref()
        .and_then(|id| {
            app.config.load().ok().and_then(|c| {
                c.providers
                    .iter()
                    .flat_map(|p| p.models.iter())
                    .find(|m| m.id == id)
                    .map(|m| u64::from(m.context_window))
            })
        })
        .filter(|w| *w > 0)
        .unwrap_or(model.context_window);
    CompactionRuntimePolicy {
        threshold: config.threshold,
        timeout: Duration::from_secs(config.timeout_secs),
        context_window: model.context_window,
        reply_reserve: model.max_tokens,
        summary_window,
    }
}

/// 工具表的估算输入面（name + description + schema 文本）。
fn tool_estimate_inputs(
    tools: &[Arc<rig::completion::request::ToolDefinition>],
) -> Vec<(String, String, String)> {
    tools
        .iter()
        .map(|t| {
            (
                t.name.to_string(),
                t.description.clone(),
                t.parameters.to_string(),
            )
        })
        .collect()
}

/// 把压缩摘要调用的实际用量计入会话累计（C-9：tokens:update 照发、
/// 不计 turnUsage——压缩不属主 run 轮次）。共享表 + 事件汇为细粒度参数
/// （run 任务的 spawn 闭包里没有 AppState 整体，只有这些克隆件）。
fn accumulate_compaction_usage(
    session_usage: &Mutex<HashMap<String, UsageTotal>>,
    events: &Arc<dyn EventSink>,
    session_id: &str,
    usages: &[rig::completion::Usage],
) {
    let mut delta = UsageTotal::default();
    for usage in usages {
        delta.accumulate(&crate::backend::turn::usage_total(usage));
    }
    if delta == UsageTotal::default() {
        return; // 无计量（mock/未知用量不伪造，Q39）
    }
    let total = {
        let mut map = session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let total = map.entry(session_id.to_owned()).or_default();
        total.accumulate(&delta);
        *total
    };
    events.emit(
        keys::TOKENS_UPDATE,
        serde_json::json!({ "sessionId": session_id, "usage": total }),
    );
}

/// 压缩后的上下文占用估算（tokensAfter 口径，§6.2）：裁剪后条目 + 摘要
/// 前缀 + 工具表 + 回复预留。
pub(crate) fn estimate_after_compaction(
    remaining_entries: &[MessageEntry],
    summary: &str,
    tools: &[(String, String, String)],
    reserve: u64,
) -> u64 {
    crate::backend::estimator::estimate_request_tokens(
        &[summary],
        remaining_entries,
        "",
        tools,
        reserve,
    )
}

/// 对会话执行一次压缩（FR-03-05 核心编排，04 号票）：
/// ① 摘要调用（增量输入 = 旧摘要 + 新增内容；分块共用总超时，Q31/Q38）；
/// ② `append_compaction` 原子落盘（含 Q34 首次格式升级备份守卫——失败
///    暂停写入并报错，原数据不动）；③ 摘要用量计入会话累计（C-9）。
/// 中途失败不提交条目、不推进边界（Q31 原子提交面），错误原样上抛由调用
/// 方发 run:compact_failed 与兜底判定。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_compaction(
    store: &Arc<crate::storage::SessionStore>,
    events: &Arc<dyn EventSink>,
    session_usage: &Mutex<HashMap<String, UsageTotal>>,
    session_id: &str,
    owner: &str,
    scope: &crate::backend::compaction::CompactionScope,
    summary_driver: &TurnDriver,
    policy: &CompactionRuntimePolicy,
) -> Result<CompactionOutcome, crate::backend::compaction::CompactionError> {
    let (summary, usages) = crate::backend::compaction::summarize(
        summary_driver,
        scope.old_summary.as_deref(),
        &scope.entries,
        policy.summary_window,
        policy.timeout,
    )
    .await?;
    // 摘要已成功，进入提交面：备份守卫 + append 原子落盘（失败即整体失败，
    // 不推进边界——摘要结果被丢弃，下次重试重新摘要）
    let (_, count) = store
        .compaction_overview(session_id)
        .map_err(|e| crate::backend::compaction::CompactionError::Failed(e.to_string()))?;
    let entry = crate::storage::StoredCompactionEntry {
        id: crate::backend::compaction::compaction_entry_id(owner, count + 1),
        cut_entry_id: scope.cut_entry_id.clone(),
        summary: summary.clone(),
        tokens_before: crate::backend::estimator::estimate_entries_tokens(&scope.entries),
        created_at: now_ms(),
    };
    store
        .append_compaction(session_id, entry)
        .map_err(|e| crate::backend::compaction::CompactionError::Failed(e.to_string()))?;
    accumulate_compaction_usage(session_usage, events, session_id, &usages);
    Ok(CompactionOutcome {
        boundary: crate::backend::wire::CompactionBoundary {
            cut_entry_id: scope.cut_entry_id.clone(),
            summary,
        },
    })
}

/// run 开始前的自动压缩编排（§6.2 触发点①，FR-03-05）：
/// 1. 估算本次请求（系统前缀 + 历史边界裁剪 + prompt + 工具表 + 回复预留）；
/// 2. 未达阈值 → Ok((既有边界, tokens))，tokens 供 wire 预算复查；
/// 3. 达阈值 → run:compacting → 压缩编排 → compactDone 帧 + run:compacted；
/// 4. 压缩失败：原上下文能放入窗口（< 0.9）→ Ok((既有边界, …)) 继续；放不下 →
///    Err(E_CONTEXT_OVERFLOW)（输入已落盘保留，不静默裁历史，§6.2 兜底）。
///
/// 压缩成功后的新边界立即生效于本次 run 的全部请求（§6.2 生效面定稿）；
/// `summary_driver` 由调用方按配置解析（专用小模型或当前模型，§6.2）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn maybe_compact_before_run(
    app: &AppState,
    session_id: &str,
    run_id: &str,
    messages: &[MessageEntry],
    tools: &[Arc<rig::completion::request::ToolDefinition>],
    policy: &CompactionRuntimePolicy,
    summary_driver: &TurnDriver,
    sink: &mut dyn FrameSink,
    custom_prompt: Option<&str>,
) -> Result<(Option<crate::backend::wire::CompactionBoundary>, u64), IpcError> {
    let overview = app
        .store
        .compaction_overview(session_id)
        .map_err(|e| IpcError::internal(e.to_string()))?;
    let boundary = overview
        .0
        .map(|c| crate::backend::wire::CompactionBoundary {
            cut_entry_id: c.cut_entry_id,
            summary: c.summary,
        });
    // 估算面：活动路径按现有边界裁剪（与 wire 重建同一裁剪口径），
    // 再剔除末条 prompt（本次输入不参与压缩，§6.2 触发点①口径）
    let whole_after_boundary = boundary_ref_slice(messages, boundary.as_ref())?;
    let body = &whole_after_boundary[..whole_after_boundary.len().saturating_sub(1)];
    let prompt_text = messages
        .last()
        .map(|m| {
            m.parts
                .iter()
                .filter_map(|p| match p {
                    crate::storage::WirePart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .unwrap_or_default();
    let tool_inputs = tool_estimate_inputs(tools);
    let prefixes: Vec<&str> = [custom_prompt, boundary.as_ref().map(|b| b.summary.as_str())]
        .into_iter()
        .flatten()
        .collect();
    let estimated = crate::backend::estimator::estimate_request_tokens(
        &prefixes,
        body,
        &prompt_text,
        &tool_inputs,
        policy.reply_reserve,
    );
    if !crate::backend::estimator::should_auto_compact(
        estimated,
        policy.context_window,
        policy.threshold,
    ) {
        return Ok((boundary, estimated));
    }

    // —— 达阈值：压缩编排（事件 → 摘要 → 落盘 → 帧/事件收口）——
    app.events.emit(
        keys::RUN_COMPACTING,
        serde_json::json!({ "sessionId": session_id }),
    );
    let scope = crate::backend::compaction::compression_scope(
        body,
        boundary
            .as_ref()
            .map(|b| (b.cut_entry_id.as_str(), b.summary.as_str())),
    );
    let executed = match scope {
        Some(scope) => {
            execute_compaction(
                &app.store,
                &app.events,
                &app.session_usage,
                session_id,
                run_id,
                &scope,
                summary_driver,
                policy,
            )
            .await
        }
        // 无可压内容但已达阈值（如首次请求即超大附件）：按超预算兜底
        None => Err(crate::backend::compaction::CompactionError::Failed(
            "已达压缩阈值但无可压缩的历史内容".to_owned(),
        )),
    };
    let outcome = match executed {
        Ok(outcome) => outcome,
        Err(_) => {
            app.events.emit(
                keys::RUN_COMPACT_FAILED,
                serde_json::json!({ "sessionId": session_id }),
            );
            // 失败兜底（§6.2）：原上下文能放入硬预算（窗口 ×0.9）则继续
            if !crate::backend::estimator::is_over_wire_budget(estimated, policy.context_window) {
                return Ok((boundary, estimated));
            }
            return Err(context_overflow(estimated, policy));
        }
    };
    // tokensAfter：新边界下的重算（压缩已在史册生效，重读概览取最新边界）
    let new_boundary = crate::backend::wire::CompactionBoundary {
        cut_entry_id: outcome.boundary.cut_entry_id.clone(),
        summary: outcome.boundary.summary.clone(),
    };
    let new_body = boundary_ref_slice(messages, Some(&new_boundary))?;
    let tokens_after = estimate_after_compaction(
        new_body,
        &new_boundary.summary,
        &tool_inputs,
        policy.reply_reserve,
    );
    sink.send(ChatFrame::CompactDone {
        cut_entry_id: new_boundary.cut_entry_id.clone(),
        tokens_before: estimated,
        tokens_after,
    });
    app.events.emit(
        keys::RUN_COMPACTED,
        serde_json::json!({
            "sessionId": session_id,
            "tokensBefore": estimated,
            "tokensAfter": tokens_after,
        }),
    );
    Ok((Some(new_boundary), tokens_after))
}

/// 活动路径的边界裁剪切片（与 [`crate::backend::wire::split_prompt_with_boundary`]
/// 同一裁剪口径的条目视图；边界锚点缺失按参数错误拒绝——不静默全量展开）。
fn boundary_ref_slice<'a>(
    entries: &'a [MessageEntry],
    boundary: Option<&crate::backend::wire::CompactionBoundary>,
) -> Result<&'a [MessageEntry], IpcError> {
    match boundary {
        None => Ok(entries),
        Some(b) => {
            let idx = entries
                .iter()
                .position(|e| e.id == b.cut_entry_id)
                .ok_or_else(|| {
                    IpcError::invalid_arg(format!(
                        "压缩边界锚点 {} 不在会话活动路径上",
                        b.cut_entry_id
                    ))
                })?;
            Ok(&entries[idx + 1..])
        }
    }
}

/// ask 关闭后的压缩触发点（§6.2 触发点②，run 任务内 wait_approval 收口处，
/// 与 §7.2 封口安全点重合）：**落盘面压缩**——摘要与 compaction 条目落盘后
/// 对恢复后的落盘条目与下一个 run 生效；当前 run 的 AgentRun 状态机
/// （rig 0.45）不支持中途替换内部 history，恢复后照常执行，恢复后请求超
/// 硬预算由 §6.2 兜底终止（输入保留）。
///
/// 估算基准 = 最近请求真实输入（`DriveState.last_input`）+ 本次审批调用
/// 参数 + 工具表 + 回复预留。失败只发 run:compact_failed、不阻断恢复
/// （落盘面优化不承载硬预算义务——硬预算判定在恢复后的请求面）。
/// 存储器/事件汇/用量表为细粒度参数（spawn 闭包内无 AppState 整体）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn maybe_compact_after_ask(
    store: &Arc<crate::storage::SessionStore>,
    events: &Arc<dyn EventSink>,
    session_usage: &Mutex<HashMap<String, UsageTotal>>,
    session_id: &str,
    run_id: &str,
    pending: &PendingCall,
    state: &DriveState,
    tools: &[Arc<rig::completion::request::ToolDefinition>],
    policy: &CompactionRuntimePolicy,
    summary_driver: &TurnDriver,
) {
    let pending_tokens = crate::backend::estimator::estimate_text_tokens(&pending.args_json);
    let tool_tokens = crate::backend::estimator::estimate_tool_tokens(&tool_estimate_inputs(tools));
    let estimated = state.last_input + pending_tokens + tool_tokens + policy.reply_reserve;
    if !crate::backend::estimator::should_auto_compact(
        estimated,
        policy.context_window,
        policy.threshold,
    ) {
        return;
    }
    events.emit(
        keys::RUN_COMPACTING,
        serde_json::json!({ "sessionId": session_id }),
    );
    // 安全点语义：run 内落盘条目均已完成（turn 收口才落盘），边界不拆批次；
    // 压缩候选 = 史册全量（无"本次输入"豁免——ask 后场景没有独立 prompt）
    let compacted = async {
        let tree = store
            .load_session_tree(session_id)
            .map_err(|e| crate::backend::compaction::CompactionError::Failed(e.to_string()))?;
        let overview = store
            .compaction_overview(session_id)
            .map_err(|e| crate::backend::compaction::CompactionError::Failed(e.to_string()))?;
        let boundary = overview
            .0
            .map(|c| crate::backend::wire::CompactionBoundary {
                cut_entry_id: c.cut_entry_id,
                summary: c.summary,
            });
        let scope = crate::backend::compaction::compression_scope(
            &tree.messages,
            boundary
                .as_ref()
                .map(|b| (b.cut_entry_id.as_str(), b.summary.as_str())),
        );
        let scope = scope.ok_or_else(|| {
            crate::backend::compaction::CompactionError::Failed("无可压缩的历史内容".to_owned())
        })?;
        execute_compaction(
            store,
            events,
            session_usage,
            session_id,
            run_id,
            &scope,
            summary_driver,
            policy,
        )
        .await
    }
    .await;
    match compacted {
        Ok(outcome) => {
            let tokens_after = store
                .load_session_tree(session_id)
                .ok()
                .and_then(|tree| {
                    let boundary = crate::backend::wire::CompactionBoundary {
                        cut_entry_id: outcome.boundary.cut_entry_id.clone(),
                        summary: outcome.boundary.summary.clone(),
                    };
                    // 锚点必在史册（cut 取自史册条目）；防御失败按 0 折算
                    boundary_ref_slice(&tree.messages, Some(&boundary))
                        .ok()
                        .map(|rest| {
                            estimate_after_compaction(
                                rest,
                                &boundary.summary,
                                &[],
                                policy.reply_reserve,
                            )
                        })
                })
                .unwrap_or(0);
            events.emit(
                keys::RUN_COMPACTED,
                serde_json::json!({
                    "sessionId": session_id,
                    "tokensBefore": estimated,
                    "tokensAfter": tokens_after,
                }),
            );
        }
        Err(_) => {
            events.emit(
                keys::RUN_COMPACT_FAILED,
                serde_json::json!({ "sessionId": session_id }),
            );
        }
    }
}

/// `compact_session` 主体（§3.3 #59b，FR-03-05 手动压缩）：对活跃 run 外的
/// 会话快照无条件压缩（不走 0.6 阈值判定）；运行中拒绝（E_SESSION_RUNNING）。
/// 无可压内容时原样返回估算值且不带 cutEntryId。
pub async fn compact_session_core(
    app: &AppState,
    runs: &RunManager,
    session_id: &str,
) -> Result<CompactSessionResult, IpcError> {
    if session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    if runs.get(session_id).is_some() {
        return Err(IpcError::session_running(format!(
            "会话已有活跃 run：{session_id}"
        )));
    }
    let model = resolve_model(app, None)?;
    let policy = compaction_policy(app, &model);
    let summary_model_id = app.config.load()?.compaction.model_id.clone();
    let summary_resolved = match summary_model_id.as_deref() {
        Some(id) => resolve_model(app, Some(id))?,
        None => resolve_model(app, None)?,
    };
    let summary_driver = TurnDriver::new(summary_resolved.driver, RunSpec::new(), Vec::new());
    let tree = app
        .store
        .load_session_tree(session_id)
        .map_err(|e| match e {
            crate::storage::StorageError::NotFound(_) => IpcError::new(
                crate::util::errors::E_NOT_FOUND,
                format!("会话不存在：{session_id}"),
            ),
            other => IpcError::internal(other.to_string()),
        })?;
    let overview = app
        .store
        .compaction_overview(session_id)
        .map_err(|e| IpcError::internal(e.to_string()))?;
    let boundary = overview
        .0
        .map(|c| crate::backend::wire::CompactionBoundary {
            cut_entry_id: c.cut_entry_id,
            summary: c.summary,
        });
    // 手动压缩估算面：史册 + 摘要前缀 + 回复预留（工具表不计——无 run 配置，
    // before/after 同口径即可比）
    let prefixes: Vec<&str> = [boundary.as_ref().map(|b| b.summary.as_str())]
        .into_iter()
        .flatten()
        .collect();
    let tokens_before = crate::backend::estimator::estimate_request_tokens(
        &prefixes,
        &tree.messages,
        "",
        &[],
        policy.reply_reserve,
    );
    let scope = crate::backend::compaction::compression_scope(
        &tree.messages,
        boundary
            .as_ref()
            .map(|b| (b.cut_entry_id.as_str(), b.summary.as_str())),
    );
    let Some(scope) = scope else {
        return Ok(CompactSessionResult {
            cut_entry_id: None,
            tokens_before,
            tokens_after: tokens_before,
        });
    };
    let outcome = execute_compaction(
        &app.store,
        &app.events,
        &app.session_usage,
        session_id,
        "manual",
        &scope,
        &summary_driver,
        &policy,
    )
    .await
    .map_err(|e| IpcError::internal(e.to_string()))?;
    let tokens_after = estimate_after_compaction(
        boundary_ref_slice(&tree.messages, Some(&outcome.boundary))?,
        &outcome.boundary.summary,
        &[],
        policy.reply_reserve,
    );
    Ok(CompactSessionResult {
        cut_entry_id: Some(outcome.boundary.cut_entry_id),
        tokens_before,
        tokens_after,
    })
}

/// `compact_session` 参数契约（§3.3 #59b）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CompactSessionArgs {
    pub session_id: String,
}

/// `compact_session` 返回契约（§3.3 #59b）：cutEntryId 缺省 = 无可压内容。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CompactSessionResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub cut_entry_id: Option<String>,
    pub tokens_before: u64,
    pub tokens_after: u64,
}

/// `compact_session` 命令（§3.3 #59b）。
#[tauri::command]
pub(crate) async fn compact_session(
    app: State<'_, AppState>,
    runs: State<'_, RunManager>,
    session_id: String,
) -> Result<CompactSessionResult, IpcError> {
    compact_session_core(&app, &runs, &session_id).await
}

/// E_CONTEXT_OVERFLOW 错误构造（§3.6，04 号票：暂停并保留输入，提示重试
/// 压缩或开启新会话）。
fn context_overflow(estimated: u64, policy: &CompactionRuntimePolicy) -> IpcError {
    IpcError::new(
        crate::util::errors::E_CONTEXT_OVERFLOW,
        format!(
            "上下文占用约 {estimated} token，超过模型窗口（{}）的硬预算且压缩后仍放不下：\
             已暂停本次发送，输入已保留，可重试压缩或开启新会话",
            policy.context_window
        ),
    )
}

/// user 条目幂等兜底：末条 user 不在史册时以史册链尾为父追加（一次 canonical
/// 落盘即 fsync）。正常协议下前端发送前已 save，此处立即跳过。
fn persist_user_fallback(app: &AppState, session_id: &str, messages: &[MessageEntry]) {
    let Some(user) = messages.iter().rev().find(|m| m.role == MessageRole::User) else {
        return;
    };
    let known = app
        .store
        .load_session_tree(session_id)
        .ok()
        .map(|tree| tree.messages.iter().any(|m| m.id == user.id))
        .unwrap_or(false);
    if known {
        return;
    }
    let mut entry = user.clone();
    if entry.parent_id.is_none()
        && let Ok(tree) = app.store.load_session_tree(session_id)
        && let Some(leaf) = tree.active_leaf_id
    {
        entry.parent_id = Some(leaf);
    }
    if let Err(e) = app
        .store
        .append_canonical(session_id, &[StoredEntry::Message(entry)])
    {
        #[cfg(debug_assertions)]
        eprintln!("[craftwave] user 条目兜底落盘失败（{session_id}）：{e:?}");
        #[cfg(not(debug_assertions))]
        let _ = e;
    }
}

/// start_chat 主体（命令薄包装转调；公开给集成测试注入内存 keyring + 临时
/// config 与测试 FrameSink——不经 tauri 运行时）。`auto_confirm` 为
/// AppConfig.approval.autoConfirm 的入口快照（§5.4：挂起 300s 超时自动
/// 选 recommended 项；M1 单会话入口读一次）。
#[allow(clippy::too_many_arguments)]
pub async fn start_chat_core(
    app: &AppState,
    runs: &RunManager,
    session_id: String,
    messages: Vec<MessageEntry>,
    run_config: RunConfig,
    mut sink: Box<dyn FrameSink>,
    auto_confirm: bool,
) -> Result<StartChatResult, IpcError> {
    if session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    // ① 模型解析（§11.1）
    let model = resolve_model(app, run_config.model_id.as_deref())?;
    // ①b wire 参数校验（§6.2：末条 user 为 prompt）——压缩编排前先验，
    //    让参数错误优先于任何有副作用的流程
    crate::backend::wire::split_prompt(&messages)
        .map_err(|e| IpcError::invalid_arg(e.to_string()))?;
    // ② 系统级前缀与压缩策略（§6.2 注入次序：customPrompt → 压缩 summary →
    //    当前计划）。save_config 即时生效于下次 run（config.load 命中缓存热
    //    替换后的值，§7.6）。
    let custom_prompt = app
        .config
        .load()?
        .agent
        .as_ref()
        .and_then(|a| a.custom_prompt.clone());
    let policy = compaction_policy(app, &model);
    // 摘要驱动（§6.2：专用小模型未配则当前 run 模型——无会话覆盖时即
    // active model；与 run 模型同源解析，独立客户端实例）
    let summary_model_id = app.config.load()?.compaction.model_id.clone();
    let summary_resolved = match summary_model_id.as_deref() {
        Some(id) => resolve_model(app, Some(id))?,
        None => resolve_model(app, run_config.model_id.as_deref())?,
    };
    let summary_driver = TurnDriver::new(summary_resolved.driver, RunSpec::new(), Vec::new());
    // ③ 工具集（§8.1 三处同源过滤）——预算检查含工具表（§6.2 Q18）
    let tool_specs = crate::tools::filter_for_tier(crate::tools::build_registry(), run_config.tier);
    let tools = to_rig_tool_definitions(tool_specs)?;

    // ④ 同会话互斥预检（E_SESSION_RUNNING 快速失败；正式互斥仍由
    //    RunManager::start 兜底）→ run 开始前压缩（§6.2 触发点①：首个模型
    //    调用前完成摘要与落盘，新边界立即生效于本次 run 全部请求）
    if runs.get(&session_id).is_some() {
        return Err(IpcError::session_running(format!(
            "会话已有活跃 run：{session_id}"
        )));
    }
    let run_id = uuid::Uuid::new_v4().to_string();
    // user 条目幂等兜底提前到压缩前（§7.3 首行）：输入先落盘再谈压缩——
    // 压缩失败/E_CONTEXT_OVERFLOW 暂停时「输入已保留」在史册面成立
    persist_user_fallback(app, &session_id, &messages);
    let (boundary, estimated_after) = maybe_compact_before_run(
        app,
        &session_id,
        &run_id,
        &messages,
        &tools,
        &policy,
        &summary_driver,
        sink.as_mut(),
        custom_prompt.as_deref(),
    )
    .await?;
    // wire 硬预算复查（§6.2：压缩后仍 > 窗口 ×0.9 → 暂停保留输入，不静默裁历史）
    if crate::backend::estimator::is_over_wire_budget(estimated_after, policy.context_window) {
        return Err(context_overflow(estimated_after, &policy));
    }
    // ⑤ wire 切分（§6.2 压缩边界）：cutEntryId 前跳过/后展开，摘要交前缀层
    let (history, prompt, compaction_summary) =
        crate::backend::wire::split_prompt_with_boundary(&messages, boundary.as_ref())
            .map_err(|e| IpcError::invalid_arg(e.to_string()))?;
    let history = crate::backend::wire::with_system_prefix_layers(
        history,
        [custom_prompt.as_deref(), compaction_summary.as_deref()],
    );

    // ⑥ 注册 run（同会话互斥 E_SESSION_RUNNING）
    let handle = runs.start(&session_id, run_id.clone(), now_ms())?;

    // ⑤ spawn RunLoop 任务（帧生产 + 审批挂起恢复 + 取消 + 终态摘除）。
    let spec = RunSpec {
        max_turns: Some(run_config.max_turns as usize),
        ..RunSpec::new()
    };
    let driver = TurnDriver::new(model.driver, spec.clone(), tools.clone());
    let model_name = model.name;
    let cancel = handle.cancel.clone();
    let executor: Arc<dyn crate::backend::run_loop::CancellableExecutor> = Arc::new(
        ToolsetExecutor::new(
            // 会话 roots（§6.2/§9.1，16 号票；C-8 收窄 21 号票）：项目会话=
            // 项目主目录 + 数据目录下 `workspace/` 子目录，临时会话两根同为
            // workspace 子目录——数据目录本体（config/sessions 等存储内部
            // 文件）退出模型文件工具可见面；收窄单一落点在
            // resolve_session_roots，这里只负责两根首次使用即建（canonicalize
            // 前置）。宿主侧边车（spill/logs）仍挂 AppState.store 根，不经
            // 本 roots。
            {
                let (project_root, data_root) =
                    crate::host::commands::workspace::resolve_session_roots(app, &session_id);
                let _ = std::fs::create_dir_all(&project_root);
                let _ = std::fs::create_dir_all(&data_root);
                crate::tools::ToolsContext::new(project_root, data_root)
            },
            run_config.tier,
        )
        .with_env(ExecutorEnv {
            events: app.events.clone(),
            ask: handle.ask.clone(),
            plan: handle.plan.clone(),
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            data_root: app.store.root().to_path_buf(),
        }),
    );
    let gate = Arc::new(ApprovalGate::new(run_config.tier, handle.plan.clone()));
    let events = app.events.clone();
    let session_usage = Arc::new(Mutex::new(
        app.session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    ));
    let runs_for_task = runs.clone();
    let suggest_tail = context_tail(&messages);
    let run_id_for_result = run_id.clone();
    // ask 关闭后压缩触发点（§6.2 触发点②）的依赖面：摘要驱动与策略随
    // run 任务进闭包（摘要模型独立解析，与 run 的 driver 互不占用）；
    // 存储器同源 Arc（命令层与 run 落盘器同一锁表）
    let summary_driver_for_task = summary_driver;
    let policy_for_task = policy;
    let store_for_task = app.store.clone();
    let recorder = RunRecorder::new(app.store.clone(), session_id.clone(), run_id.clone());
    tokio::spawn(async move {
        // 帧出口 = 事件层（usage → tokens:update）包 canonical 落盘器（§7.3）
        // 包 Channel/测试 sink
        let mut sink = EventfulSink {
            inner: Box::new(PersistingSink::new(recorder, sink)),
            events: events.clone(),
            session_id: session_id.clone(),
            last_run_total: UsageTotal::default(),
            session_usage: session_usage.clone(),
        };
        let mut run = AgentRun::from_spec(&spec, (*prompt).clone(), Some(history));
        let mut resume_state: Option<DriveState> = None;
        // 错误终态信息（§11.3 分类器为后续票，M1 统一 E_INTERNAL）：loop 内
        // 记录、收尾随 run:error 事件发出（17 号票）
        let mut errored: Option<String> = None;
        let final_exit: RunExit = loop {
            let attempt = drive_run(
                &driver,
                run,
                &run_id,
                &model_name,
                cancel.clone(),
                executor.clone(),
                gate.clone(),
                &mut sink,
                resume_state.take(),
            )
            .await;
            match attempt {
                Err(err) => {
                    errored = Some(err.to_string());
                    // 错误终态兜底：任何 rig 失败都以 runEnded(error) 收场（§5.3 终态不变量）
                    send_error_ended(&mut sink, err.to_string());
                    break RunExit::Cancelled;
                }
                Ok(RunExit::Suspended(info)) => {
                    let crate::backend::run_loop::SuspendedInfo {
                        run_json,
                        pending,
                        state,
                    } = *info;
                    match wait_approval(&events, &handle, &pending, auto_confirm).await {
                        ApprovalResolution::Allowed => {
                            gate.approve(&pending.name, &pending.args_json);
                            // ask 关闭后压缩（§6.2 触发点②）：落盘面压缩，
                            // 失败不阻断恢复（硬预算义务在恢复后的请求面）
                            maybe_compact_after_ask(
                                &store_for_task,
                                &events,
                                &session_usage,
                                &session_id,
                                &run_id,
                                &pending,
                                &state,
                                &tools,
                                &policy_for_task,
                                &summary_driver_for_task,
                            )
                            .await;
                            match resume_run(&run_json) {
                                Ok(resumed) => {
                                    run = resumed;
                                    resume_state = Some(state);
                                    continue;
                                }
                                Err(e) => {
                                    errored = Some(format!("审批恢复失败：{e}"));
                                    send_error_ended(&mut sink, format!("审批恢复失败：{e}"));
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
                Ok(other) => break other,
            }
        };

        // suggest 后台小调用（仅自然终态；C-9：静默失败、独立于 run 生命周期）
        if matches!(final_exit, RunExit::Done(_)) {
            spawn_suggest(
                driver,
                events.clone(),
                session_usage.clone(),
                session_id.clone(),
                run_id.clone(),
                suggest_tail,
            );
        }

        // run 终态事件（§3.5，17 号票补实装）：前端 Tab 指示与系统通知门
        // （TauriEventSink 旁路）共用这三个键；payload 口径照文档。
        let duration_ms = (now_ms() - handle.snapshot().started_at).max(0) as u64;
        match (&final_exit, &errored) {
            (RunExit::Done(_), _) => events.emit(
                keys::RUN_DONE,
                serde_json::json!({ "sessionId": session_id, "runId": run_id, "durationMs": duration_ms }),
            ),
            (_, Some(message)) => events.emit(
                keys::RUN_ERROR,
                serde_json::json!({ "sessionId": session_id, "runId": run_id, "code": "E_INTERNAL", "message": message }),
            ),
            _ => events.emit(
                keys::RUN_CANCELLED,
                serde_json::json!({ "sessionId": session_id, "runId": run_id }),
            ),
        }
        runs_for_task.remove(&session_id, &run_id);
    });

    Ok(StartChatResult {
        run_id: run_id_for_result,
    })
}

/// start_chat 命令（§3.3 #18）。
#[tauri::command]
pub(crate) async fn start_chat(
    app: State<'_, AppState>,
    runs: State<'_, RunManager>,
    session_id: String,
    messages: Vec<MessageEntry>,
    run_config: RunConfig,
    channel: Channel<SequencedFrame>,
) -> Result<StartChatResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = StartChatArgs {
        session_id: session_id.clone(),
        messages,
        run_config,
    };
    let sink = Box::new(ChannelSink { channel, seq: 0 });
    let auto_confirm = app
        .config
        .load()
        .map(|c| c.approval.auto_confirm)
        .unwrap_or(false);
    start_chat_core(
        &app,
        &runs,
        args.session_id,
        args.messages,
        args.run_config,
        sink,
        auto_confirm,
    )
    .await
}

/// cancel_run 主体（§3.3 #19：置位即返回；公开给集成测试）。
pub async fn cancel_run_core(runs: &RunManager, session_id: &str) -> bool {
    runs.cancel(session_id)
}

/// cancel_run 命令（§3.3 #19）：CancellationToken 置位即返回。
#[tauri::command]
pub(crate) async fn cancel_run(
    runs: State<'_, RunManager>,
    session_id: String,
) -> Result<bool, IpcError> {
    Ok(runs.cancel(&session_id))
}

/// resolve_ask 主体（§3.3 #22：投递应答并唤醒等待方；槽位清理与
/// ask:closed 事件由等待方单一出口发出）。
pub async fn resolve_ask_core(
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

/// resolve_ask 命令（§3.3 #22）。
#[tauri::command]
pub(crate) async fn resolve_ask(
    runs: State<'_, RunManager>,
    ask_id: String,
    answer: AskAnswer,
) -> Result<(), IpcError> {
    resolve_ask_core(&runs, &ask_id, answer).await
}

/// approve_plan 主体（§3.3 #26c：冻结当前计划为批准快照，§8.6）。
pub fn approve_plan_core(runs: &RunManager, session_id: &str) -> Result<(), IpcError> {
    let Some(handle) = runs.get(session_id) else {
        return Err(IpcError::not_found(format!("会话无活跃 run：{session_id}")));
    };
    let mut plan = handle
        .plan
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(state) = plan.as_mut() else {
        return Err(IpcError::invalid_arg("尚无计划可批准（等模型先建计划）"));
    };
    state.approve(now_ms());
    Ok(())
}

/// approve_plan 命令（§3.3 #26c）。
#[tauri::command]
pub(crate) async fn approve_plan(
    runs: State<'_, RunManager>,
    session_id: String,
) -> Result<(), IpcError> {
    approve_plan_core(&runs, &session_id)
}
