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
use crate::providers::types::{ApiFormat, ProviderConfig, ProviderModel};
use crate::safety::tier::PermissionTier;
use crate::storage::{MessageEntry, SessionStore};
use crate::util::errors::{E_INTERNAL, IpcError};
use crate::util::events::EventSink;
use crate::util::events::keys;

use crate::backend::run_loop::E_CANCELLED;

/// autoConfirm 自动放行等待（§5.4：300s 超时自动选 recommended 项）。
pub const AUTO_CONFIRM_TIMEOUT: Duration = Duration::from_secs(300);

/// `start_chat` 参数契约（§3.3 #18；文档命名锚点，命令形参按字段平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartChatArgs {
    pub session_id: String,
    pub messages: Vec<MessageEntry>,
    pub run_config: RunConfig,
}

/// §3.2 RunConfig（Rust 侧镜像）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    /// 会话级覆盖；缺省全局 active。
    #[serde(default)]
    pub model_id: Option<String>,
    pub tier: PermissionTier,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    /// steer 注入随 §5.5 后续票（本票前端恒不传 true）。
    #[serde(default)]
    pub steer: bool,
}

fn default_max_turns() -> u32 {
    24
}

/// `cancel_run` 参数契约（§3.3 #19）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelRunArgs {
    pub session_id: String,
}

/// `resolve_ask` 参数契约（§3.3 #22）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveAskArgs {
    pub ask_id: String,
    pub answer: AskAnswer,
}

/// `approve_plan` 参数契约（§3.3 #26c，14 号票增补：计划门批准快照冻结）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovePlanArgs {
    pub session_id: String,
}

/// `start_chat` 返回契约（§3.3 #18：`{ runId }`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartChatResult {
    pub run_id: String,
}

/// 解析后的运行模型（§11.1 ResolvedModel 的 M1 子集）。
pub(crate) struct ResolvedModel {
    /// 显示名（runStarted.model，= wire id）。
    pub name: String,
    pub driver: rig::driver::Model<Chat>,
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
    let Some(credential_ref) = provider.credential_ref.as_deref() else {
        return Err(IpcError::invalid_arg(format!(
            "供应商 {} 未配置凭据引用",
            provider.id
        )));
    };
    let Some(key) = app.keys.get_secret(credential_ref)? else {
        return Err(IpcError::keyring("钥匙串无此凭据（可能已被删除）"));
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 发送错误终态帧（RunLoop 冒出的 PromptError 兜底，保证前端必有终态）。
fn send_error_ended(sink: &mut dyn FrameSink, message: String) {
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

    enum Wait {
        Answer(Option<AskAnswer>),
        Cancelled,
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
    sink: Box<dyn FrameSink>,
    auto_confirm: bool,
) -> Result<StartChatResult, IpcError> {
    if session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    // ① 模型解析（§11.1）
    let model = resolve_model(app, run_config.model_id.as_deref())?;
    // ② wire 切分（§6.2）：末条 user 为 prompt，其余为 history（参数校验面，
    //    错误在命令内直接 E_INVALID_ARG）
    let (history, prompt) = crate::backend::wire::split_prompt(&messages)
        .map_err(|e| IpcError::invalid_arg(e.to_string()))?;
    // ③ 工具集（§8.1 三处同源过滤）
    let tool_specs = crate::tools::filter_for_tier(crate::tools::build_registry(), run_config.tier);
    let tools = to_rig_tool_definitions(tool_specs)?;

    // ④ 注册 run（同会话互斥 E_SESSION_RUNNING）
    let run_id = uuid::Uuid::new_v4().to_string();
    let handle = runs.start(&session_id, run_id.clone(), now_ms())?;

    // ⑤ spawn RunLoop 任务（帧生产 + 审批挂起恢复 + 取消 + 终态摘除）。
    let spec = RunSpec {
        max_turns: Some(run_config.max_turns as usize),
        ..RunSpec::new()
    };
    let driver = TurnDriver::new(model.driver, spec.clone(), tools);
    let model_name = model.name;
    let cancel = handle.cancel.clone();
    let executor: Arc<dyn crate::backend::run_loop::CancellableExecutor> = Arc::new(
        ToolsetExecutor::new(
            // 临时会话 roots = 全局数据目录（§6.2 注；项目会话随项目票接入）。
            // 防御性建目录：正常由 lib.rs setup 创建，此处兜底首目录缺失
            //（command 的 cwd / spill 落盘都以它为基准）。
            {
                let root = SessionStore::default_root();
                let _ = std::fs::create_dir_all(&root);
                crate::tools::ToolsContext::new(root.clone(), root)
            },
            run_config.tier,
        )
        .with_env(ExecutorEnv {
            events: app.events.clone(),
            ask: handle.ask.clone(),
            plan: handle.plan.clone(),
            session_id: session_id.clone(),
            run_id: run_id.clone(),
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
    tokio::spawn(async move {
        // 帧出口 = 事件层（usage → tokens:update 全量校正）包 Channel/测试 sink
        let mut sink = EventfulSink {
            inner: sink,
            events: events.clone(),
            session_id: session_id.clone(),
            last_run_total: UsageTotal::default(),
            session_usage: session_usage.clone(),
        };
        let mut run = AgentRun::from_spec(&spec, (*prompt).clone(), Some(history));
        let mut resume_state: Option<DriveState> = None;
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
                            match resume_run(&run_json) {
                                Ok(resumed) => {
                                    run = resumed;
                                    resume_state = Some(state);
                                    continue;
                                }
                                Err(e) => {
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
