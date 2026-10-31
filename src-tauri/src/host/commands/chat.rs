//! start_chat / cancel_run / resolve_ask / approve_plan / inject_run_message /
//! resume_followup_queue / remove_followup 命令。
//!
//! - 命令层只做校验/转调与装配：模型解析（RunConfig.modelId >
//!   AppConfig.activeModelId）→ wire 切分 → 工具集（档位过滤）→
//!   RunManager 注册 → spawn RunLoop 任务（`backend::run_loop::drive_run`）；
//! - **审批挂起恢复循环**：`RunExit::Suspended` → 构造
//!   `AskPayload(kind=approval)` → 注册 AskSlot + `ask:opened` 事件 → 等待
//!   `resolve_ask`/取消/autoConfirm 300s → allow 则 `ApprovalGate::approve`
//!   按签名放行 + `resume_run` 续跑（DriveState 延续：不重发 runStarted、
//!   usage/计数不回退）；deny/取消 → 挂起调用补拒绝结果帧 + runEnded(cancelled)；
//! - **suggest 后台小调用**：run Done 后以同模型一次非流式调用生成
//!   建议提示词 → `run:suggestions` 事件；失败静默、用量计入会话累计；
//! - **steer / followUp 队列**：`inject_run_message` 按
//!   活跃状态与 mode 分叉——运行中 steer 入 RunHandle 插话队列（turn 边界
//!   由 `SteerSource` 宿主实现消费：出队 + canonical 落盘 + run:inject 回执，
//!   注入下一请求 history）；followUp 入会话级队列（`host::followup`，
//!   queue.json 持久化），run 正常完成后自动投递队列头新开 run（复查无活跃
//!   run、撞车回队重试）；run 失败暂停队列等待 `resume_followup_queue`；
//!   取消清空队列并把文本随 `run:queue.returned` 交还前端草稿；
//! - 帧出口：tauri `Channel` 适配为已序列化帧下发端，seq
//!   递增 + 环形缓冲（512 帧）+ 下发收进 agent 层 `FrameHub` 枢纽，外包事件
//!   层（usage 帧 → RunState 同步 + tokens:update 事件全量校正）与 canonical
//!   落盘层；`attach_run`/`session_run_snapshot`/`session_pending_ask`
//!   承载 WebView 重载后的帧重放续流与状态对账；自动投递的
//!   新 run 无前端通道，以 `DetachedSink` 起手（帧只入环不下发，前端经
//!   `run:start` 事件 → attach 观察态续流）；
//! - provider 三方言（openai_chat / openai_responses /
//!   anthropic_messages 按 `ProviderConfig.apiFormat` 装配，差异收敛在
//!   `providers::wire` 防腐层），Key 经 config 读取；
//! - 错误终态兜底：RunLoop 内部失败（PromptError）由 run 任务发
//!   runEnded(error)，code = E_INTERNAL（看门狗终止除外，见 craftwave_engine::followup::RunFailure）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rig::completion::request::ToolDefinition;
use rig::message::ToolName;
use rig::run::AgentRun;
use rig::run::spec::RunSpec;
use serde::{Deserialize, Serialize};
use tauri::State;
use tauri::ipc::Channel;
use tokio_util::sync::CancellationToken;

use craftwave_engine::agent::{FrameHub, HubSink, RunHandle, RunManager};
use craftwave_engine::backend::ask::{
    ApprovalDiff, AskAnswer, AskMode, AskOption, AskPayload, AskSlot, DiffHunk, DiffLine,
    DiffLineKind,
};
use craftwave_engine::backend::exec::{AskSlotCell, ExecutorEnv, ToolsetExecutor};
use craftwave_engine::backend::frames::{
    ChatFrame, EndReason, FrameError, FrameSink, RunSnapshot, SequencedFrame, SequencedSink,
    UsageTotal,
};
use craftwave_engine::backend::gate::ApprovalGate;
use craftwave_engine::backend::run_loop::{
    DriveState, PendingCall, RefreshFailure, RequestRefresher, RunExit, SteerSource, drive_run,
    resume_run,
};
// 外部目录授权核心：恢复循环在用户允许后并入会话 roots。
use craftwave_engine::backend::turn::TurnDriver;
use craftwave_engine::app_state::AppState;
use super::workspace::allow_external_dir_core;
use craftwave_engine::followup::{
    FollowUpEntry, FollowUpManager, QueuedImageAttachment, QueuedRunConfig,
};
use craftwave_engine::run_persistence::{PersistingSink, RunRecorder};
use craftwave_engine::providers::types::{ApiFormat, ProviderConfig, ProviderModel};
use craftwave_engine::providers::wire::DialectWire;
use craftwave_engine::safety::tier::PermissionTier;
use craftwave_engine::storage::{MessageEntry, MessageRole, StoredEntry};
use craftwave_engine::util::errors::{E_INTERNAL, E_RUN_STALLED, IpcError};
use craftwave_engine::util::events::EventSink;
use craftwave_engine::util::events::keys;

use craftwave_engine::backend::run_loop::E_CANCELLED;

/// autoConfirm 自动放行等待（300s 超时自动选 recommended 项）。
pub const AUTO_CONFIRM_TIMEOUT: Duration = Duration::from_secs(300);

/// 收集消息里的图片 blob 并装配查找表（wire 重建的 IO 面）：
/// hash 去重后逐个从会话 imgblob 目录读取（字节 → base64 + 魔数嗅探 mime）。
/// 单个 blob 读取失败跳过（vision 模型下该图在 wire 重建时降级为占位文本，
/// 不拒绝请求——blob 可能已被会话删除等流程清走）；日志纪律：只记哈希。
pub(crate) fn load_image_blobs(
    root: &std::path::Path,
    session_id: &str,
    messages: &[MessageEntry],
) -> craftwave_engine::backend::wire::ImageBlobMap {
    let mut hashes: Vec<&str> = Vec::new();
    for entry in messages {
        for part in &entry.parts {
            if let craftwave_engine::storage::WirePart::Image { blob_hash, .. } = part
                && !hashes.contains(&blob_hash.as_str())
            {
                hashes.push(blob_hash);
            }
        }
    }
    let mut map = craftwave_engine::backend::wire::ImageBlobMap::new();
    for hash in hashes {
        match craftwave_engine::storage::get_blob(root, session_id, hash) {
            Ok((bytes, mime)) => {
                use base64::Engine;
                map.insert(
                    hash.to_owned(),
                    craftwave_engine::backend::wire::ImageBlobData {
                        mime: mime.to_owned(),
                        base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
                    },
                );
            }
            Err(_) => {
                #[cfg(debug_assertions)]
                eprintln!("[craftwave] 图片 blob 装配跳过（缺失，会话 {session_id}）：{hash}");
            }
        }
    }
    map
}

/// `start_chat` 参数契约（文档命名锚点，命令形参按字段平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StartChatArgs {
    pub session_id: String,
    pub messages: Vec<MessageEntry>,
    pub run_config: RunConfig,
}

/// RunConfig（Rust 侧镜像）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RunConfig {
    /// 会话级覆盖；缺省全局 active。
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub model_id: Option<String>,
    /// 会话 thinking 档：本次 run 的起点档位（serde
    /// default 向后兼容——旧前端载荷无此字段照常解析）。运行中以后端
    /// prefs 文件为权威（每拍请求前刷新）；None = 未显式设置，按
    /// per-model reasoningEffort 回落（优先级链）。
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub thinking: Option<craftwave_engine::providers::reasoning::ThinkingLevel>,
    pub tier: PermissionTier,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    /// steer 注入（当前前端恒不传 true，注入走 inject_run_message）。
    #[serde(default)]
    pub steer: bool,
}

/// maxTurns 缺省值：单次 run 最多跑 24 个来回（防止模型无限循环烧钱）。
fn default_max_turns() -> u32 {
    24
}

/// `inject_run_message` 的注入语义：steer = 运行中插话
/// （有活跃 run 时注入当前 run，turn 边界消费）；followUp = 排队（任何时刻
/// 可排，run 正常完成后自动投递）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum InjectMode {
    Steer,
    FollowUp,
}

/// `inject_run_message` 参数契约（改写；增
/// `attachments`）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct InjectRunMessageArgs {
    pub session_id: String,
    pub text: String,
    /// 注入语义；缺省按 steer 处理（无活跃 run 时自动落 followUp）。
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub mode: Option<InjectMode>,
    /// followUp 附着的运行配置快照（投递新 run 的 RunConfig 来源；缺省回退
    /// 默认档位组合）。
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub run_config: Option<RunConfig>,
    /// 图片附件引用：非空时**不入 steer**——按
    /// 带图消息规则转会话级 followUp 队列（引用已在 attach_image 落盘）。
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub attachments: Option<Vec<QueuedImageAttachment>>,
}

/// `inject_run_message` 返回契约：实际入队的通道。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct InjectRunMessageResult {
    pub queued: bool,
    /// 实际接纳通道："steer" = 已被运行中 run 接纳；"followUp" = 已入会话级
    /// 待发队列。queued=false（未接纳）时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub mode: Option<InjectMode>,
    /// followUp 模式下返回队列条目 id（前端队列条的删除/编辑取出回传键）；
    /// steer 模式省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub entry_id: Option<String>,
}

/// `resume_followup_queue` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ResumeFollowupQueueArgs {
    pub session_id: String,
}

/// `resume_followup_queue` 返回契约：delivered=true 表示解除
/// 暂停后立即投递了队列头并新开 run（runId 带值）。
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ResumeFollowupQueueResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub run_id: Option<String>,
    pub delivered: bool,
}

/// `remove_followup` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RemoveFollowupArgs {
    pub session_id: String,
    pub entry_id: String,
}

/// `remove_followup` 返回契约：移除后的队列长度。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RemoveFollowupResult {
    pub size: usize,
}

/// `cancel_run` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CancelRunArgs {
    pub session_id: String,
}

/// `resolve_ask` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ResolveAskArgs {
    pub ask_id: String,
    pub answer: AskAnswer,
}

/// `approve_plan` 参数契约（计划门批准快照冻结）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ApprovePlanArgs {
    pub session_id: String,
}

/// `start_chat` 返回契约（`{ runId }`）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StartChatResult {
    pub run_id: String,
}

/// 解析后的运行模型（ResolvedModel 的当前子集）。
pub(crate) struct ResolvedModel {
    /// 模型配置 id（`ProviderModel.id`；用量统计按实际执行模型
    /// 归属的记账面——显示名可改可删，配置 id 是稳定聚合键）。
    pub id: String,
    /// 显示名（runStarted.model，= wire id）。
    pub name: String,
    /// 三方言统一驱动（`providers::wire::DialectWire` 按
    /// apiFormat 构造——openai_chat / openai_responses /
    /// anthropic_messages 三协议在此收口为单一生产类型）。
    pub driver: rig::driver::Model<DialectWire>,
    /// 上下文窗口 token 数（`ProviderModel.contextWindow`，缺省 128000；
    /// 压缩预算检查的窗口面）。
    pub context_window: u64,
    /// 单次回复 token 上限（`ProviderModel.maxTokens`，缺省 32768；
    /// 每请求预算的回复预留面，Anthropic 方言的请求级
    /// max_tokens 注入面）。
    pub max_tokens: u64,
    /// 是否支持图片输入（`ProviderModel.vision`，wire 重建
    /// 双形态与 read(kind=image) 前置校验的判定面）。
    pub vision: bool,
    /// 请求方言（thinking 档位按方言走 映射表）。
    pub api_format: ApiFormat,
    /// per-model reasoning 档配置（ProviderModel.reasoningEffort；
    /// 会话未显式设置 thinking 时的回落源，优先级链第二级）。
    pub reasoning_effort: Option<craftwave_engine::providers::types::ReasoningEffort>,
    /// API 端点基地址（供应商 baseUrl 或官方缺省，resolve 时快照）——
    /// 会话日志 llm 行的 host 元数据面（诊断用；不涉敏感值）。
    pub base_url: String,
}

/// 模型解析：RunConfig.modelId > AppConfig.activeModelId →
/// (provider, model) → 三方言统一 rig 客户端（baseUrl 可覆写）。
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
    // Key 装配（免 Key）：本地服务（local=true）不校验
    // 凭据，缺 Key 时以占位值过 rig 的必填面（本地端点忽略凭据头）；远端
    // 仍强制要求。
    let key = match provider.api_key.as_deref() {
        Some(key) => key.to_owned(),
        None if provider.local => "local".to_owned(),
        None => {
            return Err(IpcError::invalid_arg(format!(
                "供应商 {} 未配置 API Key（设置页填写后保存）",
                provider.id
            )));
        }
    };
    let base_url = provider
        .base_url
        .clone()
        .unwrap_or_else(|| provider.api_format.official_base_url().to_owned());
    // 代理注入（"代理经 rig-reqwest builder"）：不再用 rig
    // 的共享 reqwest client（`client()`），改取代理工厂当前快照包一层
    // ReqwestClient 经 `connect` 挂上——模型请求与探测/网络工具共用
    // none/system/manual 三模式语义（rig-http 0.45 注入口径，回写）。
    // client 按 run 取用且 run 内复用；代理热生效后下一次 start_chat 换新。
    // 三方言统一构造（providers::wire 防腐层）：按 apiFormat
    // 装配 Chat / Responses / Messages wire，差异收敛在 providers 域。
    let http = rig::http_client::DynHttpClient::new(rig_reqwest::ReqwestClient::from(
        app.http.client().as_ref().clone(),
    ));
    let driver = craftwave_engine::providers::wire::dialect_model(
        provider.api_format,
        &base_url,
        &key,
        &model.model,
        http,
    );
    Ok(ResolvedModel {
        // 配置 id（wanted 已归一：参数 id 或 activeModelId 的命中项）
        id: wanted.to_owned(),
        name: model.model.clone(),
        driver,
        context_window: u64::from(model.context_window),
        max_tokens: u64::from(model.max_tokens),
        // vision 缺省 false（保守：未标记的模型按不支持图片处理——字段可空即未知）
        vision: model.vision.unwrap_or(false),
        api_format: provider.api_format,
        reasoning_effort: model.reasoning_effort,
        base_url,
    })
}

/// 生效 thinking 档的解析（优先级链）：会话显式档 >
/// per-model reasoningEffort 配置 > 不传。输出 映射指令供请求参数
/// 装配（纯函数在 `providers::reasoning`，本函数只做取值与回落）。
pub(crate) fn effective_thinking_directive(
    session_thinking: Option<craftwave_engine::providers::reasoning::ThinkingLevel>,
    model: &ResolvedModel,
) -> craftwave_engine::providers::reasoning::ReasoningDirective {
    let window = model.context_window;
    let max_tokens = model.max_tokens;
    match session_thinking {
        Some(level) => craftwave_engine::providers::reasoning::thinking_to_request(
            level,
            model.api_format,
            window,
            max_tokens,
        ),
        None => match model.reasoning_effort {
            Some(effort) => craftwave_engine::providers::reasoning::model_effort_to_request(
                effort,
                model.api_format,
                window,
                max_tokens,
            ),
            None => craftwave_engine::providers::reasoning::ReasoningDirective::none(),
        },
    }
}

/// ToolSpec → rig ToolDefinition（广播给模型的工具表）。
fn to_rig_tool_definitions(
    specs: Vec<craftwave_engine::tools::ToolSpec>,
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

/// tauri `Channel` → 已序列化帧下发端（帧基座出口）：seq/ts 由
/// `FrameHub` 统一填写，本适配器只负责把帧推入 webview
/// 通道。前端掉线（webview 重载）时发送失败：帧已入环形缓冲，可被
/// `attach_run` 重放（的直接丢弃语义升级为可重放续流）。
struct ChannelSink {
    channel: Channel<SequencedFrame>,
}

impl SequencedSink for ChannelSink {
    fn send_seq(&mut self, frame: SequencedFrame) {
        let _ = self.channel.send(frame);
    }
}

/// 帧出口的事件层：usage 帧增量折算 → 会话累计 →
/// `tokens:update` 全量校正事件（sessionUsage 以事件为准）。
/// usage 帧载荷是 run 内累计值，须减去上一帧的 run 累计得本轮增量。
/// 增补：同一处同步 RunHandle 的运行时状态（RunState）——turn 取
/// stepStarted 帧、工具计数按收到的工具结果帧递增、usage 取 usage 帧的
/// 运行累计载荷；`session_run_snapshot` / attach 摘要帧据此对账。
/// 增补：usage 差分增量同步落账（日/模型聚合 + run/会话明细 +
/// stats_cache 写穿）——记账日取当时本地日期；主 run 首个
/// 有计量的帧计当日该模型 runs 一次；模型身份取 `current_model`
/// 槽（HostRequestRefresher 每拍刷新，run 内切模型按拍真实归属）。
struct EventfulSink {
    inner: Box<dyn FrameSink>,
    events: Arc<dyn EventSink>,
    session_id: String,
    /// 本 sink 所属的 run id（明细落账键）。
    run_id: String,
    last_run_total: UsageTotal,
    session_usage: Arc<Mutex<HashMap<String, UsageTotal>>>,
    /// 所属 run 的句柄（运行时状态同步目标；测试直驱路径可为 None）。
    handle: Option<Arc<RunHandle>>,
    /// 用量账本（AppState.ledger 的克隆件——spawn 闭包内无整体）。
    ledger: craftwave_engine::usage::UsageLedger,
    /// 当前生效模型槽 `(配置 id, 显示名)`：run 装配时置初值，
    /// HostRequestRefresher 每拍刷新（模型切换时更新）——Usage 帧落账时
    /// 读到的即「本拍实际执行模型」（真实归属）。
    current_model: Arc<Mutex<Option<(String, String)>>>,
    /// 本 run 是否已计当日该模型的 runs 次数（首个有计量的 usage 帧计一次；
    /// 无计量不伪造）。
    run_counted: bool,
    /// 会话日志器（工具起止行的落点；测试直驱路径 None）。
    session_log: Option<Arc<craftwave_engine::session_log::SessionLogger>>,
    /// 在飞工具起点表（ToolStarted 记名与时刻，ToolResult 收口
    /// 算耗时落 tool 行——帧面无耗时字段，宿主侧配对补齐）。
    tool_starts: HashMap<String, (String, std::time::Instant)>,
}

impl EventfulSink {
    /// 把本轮增量累进共享的会话用量表，并发出全量快照事件
    /// （前端只认事件里的全量值，不自己加总）。`model_id` 随载荷带出
    /// （前端统计区块当日/该模型本地增量修正的依据）。
    fn send_tokens_update(&mut self, delta: &UsageTotal, model_id: &str) {
        let mut map = self
            .session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let total = map.entry(self.session_id.clone()).or_default();
        total.accumulate(delta);
        let snapshot = *total;
        let mut payload = serde_json::json!({ "sessionId": self.session_id, "usage": snapshot });
        if !model_id.is_empty() {
            payload["modelId"] = serde_json::json!(model_id);
        }
        self.events.emit(keys::TOKENS_UPDATE, payload);
    }

    /// usage 差分增量落账（明细/聚合/缓存三面）。`counts_run` = 本 run
    /// 尚未计次且本次有真实计量（主 run 计一次，无计量不伪造）。
    fn record_stats(&mut self, delta: &UsageTotal) {
        if *delta == UsageTotal::default() {
            return; // 零计量不落账（未知用量不伪造）
        }
        let (model_id, model_name) = self
            .current_model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_default();
        let counts_run = !self.run_counted;
        self.run_counted = true;
        self.ledger.record(
            &self.session_id,
            &self.run_id,
            craftwave_engine::usage::run_kind::MAIN,
            &model_id,
            &model_name,
            *delta,
            counts_run,
        );
    }

    /// 把帧面的运行时进度同步进 RunHandle 的 RunState（对账数据源）。
    /// tool_count 口径 = 已收到的工具结果帧数（含硬拦/取消的补发结果帧，
    /// 与 RunLoop 的 DriveState 计数同面；审批拒绝的补发结果帧也计数——
    /// 语义为"该调用已被处理"）。
    fn sync_run_state(&self, frame: &ChatFrame, run_total: Option<UsageTotal>) {
        let Some(handle) = &self.handle else {
            return;
        };
        let mut state = handle
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match frame {
            ChatFrame::StepStarted { turn } => state.turn = *turn,
            ChatFrame::ToolResult { .. } => state.tool_count = state.tool_count.saturating_add(1),
            ChatFrame::Usage { .. } => {
                if let Some(total) = run_total {
                    state.usage = total;
                }
            }
            _ => {}
        }
    }
}

impl FrameSink for EventfulSink {
    fn send(&mut self, frame: ChatFrame) {
        if let ChatFrame::Usage { usage, .. } = &frame {
            let delta = sub_usage(usage, &self.last_run_total);
            self.last_run_total = *usage;
            // usage 帧载荷即运行累计值：先同步状态再发事件（快照与帧面一致）
            self.sync_run_state(&frame, Some(*usage));
            // 统计落账在事件之前（同帧同源：事件 modelId 与落账模型一致）
            self.record_stats(&delta);
            let model_id = self
                .current_model
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .map(|(id, _)| id)
                .unwrap_or_default();
            self.send_tokens_update(&delta, &model_id);
        } else {
            self.sync_run_state(&frame, None);
        }
        // 工具起止行：ToolStarted 记起点，ToolResult 收口算
        // 耗时（错误码随 error 面）；恢复重放批可能无 started（挂起前
        // 已发过）——耗时省略，行照记。
        match &frame {
            ChatFrame::ToolStarted { call_id, name } => {
                self.tool_starts
                    .insert(call_id.clone(), (name.clone(), std::time::Instant::now()));
            }
            ChatFrame::ToolResult {
                call_id,
                ok,
                error,
                ui_summary,
            } => {
                if let Some(logger) = &self.session_log {
                    let (name, elapsed_ms) = match self.tool_starts.remove(call_id) {
                        Some((name, started)) => (name, started.elapsed().as_millis() as u64),
                        // 无起点（审批挂起恢复重放 / 子代理帧等）：耗时未知
                        None => (String::new(), 0),
                    };
                    let code = error.as_ref().map(|e| e.code.clone());
                    let msg = if *ok {
                        format!("工具执行完成：{ui_summary}")
                    } else {
                        format!(
                            "工具执行失败：{}",
                            error
                                .as_ref()
                                .map(|e| e.message.clone())
                                .unwrap_or_else(|| ui_summary.clone())
                        )
                    };
                    logger.session_line(
                        &self.session_id,
                        if *ok {
                            craftwave_engine::providers::types::LogLevel::Info
                        } else {
                            craftwave_engine::providers::types::LogLevel::Warn
                        },
                        craftwave_engine::session_log::LogKind::Tool,
                        msg,
                        Some(serde_json::json!({
                            "name": name,
                            "ok": ok,
                            "durationMs": elapsed_ms,
                            "code": code,
                        })),
                        Some(&self.run_id),
                        Some(call_id),
                        None,
                    );
                }
            }
            _ => {}
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

/// 问卷类 ask 开关的宿主日志实现（`craftwave_engine::backend::exec::
/// AskLogSink`）：把 backend 的回调面落到会话日志 kind=approval 行
/// （问卷与审批卡共用同一 kind；askId 作 callId 锚）。pub(crate)：子代理
/// 环境的 ExecutorEnv 装配（subagent.rs）复用同一实现。
pub(crate) struct HostAskLogger {
    pub(crate) session_log: Arc<craftwave_engine::session_log::SessionLogger>,
}

impl craftwave_engine::backend::exec::AskLogSink for HostAskLogger {
    fn ask_opened(&self, session_id: &str, ask_id: &str, run_id: &str) {
        self.session_log.session_line(
            session_id,
            craftwave_engine::providers::types::LogLevel::Info,
            craftwave_engine::session_log::LogKind::Approval,
            "等待用户回答问卷".to_owned(),
            None,
            Some(run_id),
            Some(ask_id),
            None,
        );
    }

    fn ask_closed(&self, session_id: &str, ask_id: &str, run_id: &str, answered: bool) {
        self.session_log.session_line(
            session_id,
            craftwave_engine::providers::types::LogLevel::Info,
            craftwave_engine::session_log::LogKind::Approval,
            if answered {
                "问卷已回答".to_owned()
            } else {
                "问卷未回答（取消）".to_owned()
            },
            Some(serde_json::json!({ "answered": answered })),
            Some(run_id),
            Some(ask_id),
            None,
        );
    }
}

/// 模型调用日志的宿主实现（[`TurnDriver` 所挂的
/// `backend::turn::LlmLogSink`]）：
/// - Info 级：每次收口记一行元数据（host/model/耗时/usage，不涉内容）；
/// - Debug 级（verbose）：追加请求/响应原文两行——**落盘前必须过脱敏
///   闸**（图片 base64 段摘要化为路径/哈希/格式/大小 + 密钥名单与
///   `Bearer`/`sk-` 兜底替换，红线），序列化本身也只在 verbose
///   才发生（大载荷成本不进常规路径）。
///
/// 图片路径口径：请求里的图片本体段替换为摘要后，宿主把
/// 会话 imgblob 目录传入摘要器——附件/粘贴图以内容寻址落在该目录，
/// 哈希命中即补真实 blob 路径；read 读图的工作区源路径在工具结果文本
/// 内（base64 段外的普通文本，不被替换、原样保留）。无来源的图片
/// 不虚构路径。
struct HostLlmLogger {
    session_log: Arc<craftwave_engine::session_log::SessionLogger>,
    session_id: String,
    run_id: String,
    /// API 端点 host（resolve 时快照的 baseUrl 取 host 部分，诊断面）。
    api_host: String,
    /// 会话图片 blob 目录（摘要补路径用；内容寻址、仅实际存在才补）。
    blob_dir: std::path::PathBuf,
    /// 当前生效模型槽 `(配置 id, 显示名)`（与 refresher/sink 共享的同一
    /// 句柄——运行中切模型后日志读到的是本拍实际执行模型，同源）。
    current_model: Arc<Mutex<Option<(String, String)>>>,
}

impl HostLlmLogger {
    /// 从 baseUrl 提取 host 部分（`scheme://host[:port]/…` → `host[:port]`；
    /// 解析不出原样返回，诊断面不做强校验）。
    fn host_of(base_url: &str) -> String {
        let after_scheme = base_url.split("://").nth(1).unwrap_or(base_url);
        after_scheme
            .split(['/', '?', '#'])
            .next()
            .unwrap_or(base_url)
            .to_owned()
    }

    /// 当前模型显示名（槽未置位时回退配置 id 的空串占位）。
    fn model_label(&self) -> String {
        self.current_model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .map(|(_, name)| name)
            .unwrap_or_default()
    }

    /// usage 四口径折一行 JSON（None = 无计量，如取消路径）。
    fn usage_json(usage: Option<&rig::completion::Usage>) -> serde_json::Value {
        match usage {
            None => serde_json::Value::Null,
            Some(u) => serde_json::json!({
                "input": u.input_tokens.unwrap_or(0),
                "output": u.output_tokens.unwrap_or(0),
                "cacheRead": u.cached_input_tokens.unwrap_or(0),
                "cacheWrite": u.cache_creation_input_tokens.unwrap_or(0),
            }),
        }
    }
}

impl craftwave_engine::backend::turn::LlmLogSink for HostLlmLogger {
    fn on_request(&self, turn: u32, request: &rig::completion::request::CompletionRequest) {
        // verbose 才序列化（级别检查在 verbose()，防大载荷进常规路径）
        if !self.session_log.verbose() {
            return;
        }
        let Ok(mut raw) = serde_json::to_value(request) else {
            return;
        };
        // 脱敏闸（红线）：先图片本体摘要化（补真实 blob 路径），
        // 再密钥名单与 Bearer/sk- 兜底
        craftwave_engine::util::redact::summarize_images(&mut raw, Some(&self.blob_dir));
        craftwave_engine::util::redact::redact_json(&mut raw);
        self.session_log.session_line(
            &self.session_id,
            craftwave_engine::providers::types::LogLevel::Debug,
            craftwave_engine::session_log::LogKind::Llm,
            if turn == 0 {
                "模型请求原文（辅助调用，已脱敏，图片仅路径/哈希/格式/大小）".to_owned()
            } else {
                "模型请求原文（已脱敏，图片仅路径/哈希/格式/大小）".to_owned()
            },
            Some(raw),
            Some(&self.run_id),
            None,
            None,
        );
    }

    fn on_response(
        &self,
        turn: u32,
        duration_ms: u64,
        usage: Option<&rig::completion::Usage>,
        response: Option<&rig::completion::request::CompletionResponse>,
        error: Option<&str>,
    ) {
        let meta = || {
            serde_json::json!({
                "host": self.api_host,
                "model": self.model_label(),
                "durationMs": duration_ms,
                "usage": Self::usage_json(usage),
            })
        };
        match error {
            // 取消：Info 行（流被丢弃，无 usage 无响应体——不算失败）
            Some("cancelled") => {
                self.session_log.session_line(
                    &self.session_id,
                    craftwave_engine::providers::types::LogLevel::Info,
                    craftwave_engine::session_log::LogKind::Llm,
                    "模型请求被取消".to_owned(),
                    Some(meta()),
                    Some(&self.run_id),
                    None,
                    None,
                );
            }
            // 失败：Warn 行，失败说明过字符串兜底防密钥经错误消息漏出；
            // 失败无响应体，Debug 级不追加
            Some(e) => {
                let safe = craftwave_engine::util::redact::redact_string(e).unwrap_or_else(|| (*e).to_owned());
                self.session_log.session_line(
                    &self.session_id,
                    craftwave_engine::providers::types::LogLevel::Warn,
                    craftwave_engine::session_log::LogKind::Llm,
                    format!("模型请求失败：{safe}"),
                    Some(meta()),
                    Some(&self.run_id),
                    None,
                    None,
                );
            }
            // 正常收口：Info 行
            None => {
                let msg = if turn == 0 {
                    "模型辅助调用完成".to_owned()
                } else {
                    "模型请求完成".to_owned()
                };
                self.session_log.session_line(
                    &self.session_id,
                    craftwave_engine::providers::types::LogLevel::Info,
                    craftwave_engine::session_log::LogKind::Llm,
                    msg,
                    Some(meta()),
                    Some(&self.run_id),
                    None,
                    None,
                );
            }
        }
        // Debug 级追加响应原文（脱敏闸同请求面；取消/失败路径 response=None）
        if let (Some(response), true) = (response, self.session_log.verbose()) {
            let Ok(mut raw) = serde_json::to_value(response) else {
                return;
            };
            craftwave_engine::util::redact::summarize_images(&mut raw, Some(&self.blob_dir));
            craftwave_engine::util::redact::redact_json(&mut raw);
            self.session_log.session_line(
                &self.session_id,
                craftwave_engine::providers::types::LogLevel::Debug,
                craftwave_engine::session_log::LogKind::Llm,
                "模型响应原文（已脱敏）".to_owned(),
                Some(raw),
                Some(&self.run_id),
                None,
                None,
            );
        }
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

/// steer 消费的宿主实现：出队 → canonical 落盘
/// （id=`{runId}:steer:{n}`，父=消费时刻链尾）→
/// `run:inject` 回执（acceptedAtTurn + entryId/text，前端据以本地插入插话
/// 节点）。副作用收在本实现内，RunLoop（backend 层）只拿到注入文本。
struct HostSteerSource {
    /// 该 run 的插话队列（RunHandle.steer 的共享句柄）。
    queue: Arc<Mutex<std::collections::VecDeque<String>>>,
    store: Arc<craftwave_engine::storage::SessionStore>,
    events: Arc<dyn EventSink>,
    session_id: String,
    run_id: String,
    /// 条目序号（id 的 `{n}` 段，run 内从 1 递增）。
    seq: std::sync::atomic::AtomicU32,
}

impl SteerSource for HostSteerSource {
    fn take(&self, turn: u32) -> Option<String> {
        let text = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()?;
        let n = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let entry_id = format!("{}:steer:{n}", self.run_id);
        // 父 = 消费时刻 canonical 链尾（steer 条目排在最新已落盘条目之后——
        // turn 收口的 assistant 条目已先于此落盘，时机次序天然成立）
        let parent_id = self
            .store
            .load_session_tree(&self.session_id)
            .ok()
            .and_then(|tree| tree.active_leaf_id);
        let entry = MessageEntry {
            id: entry_id.clone(),
            parent_id,
            role: MessageRole::User,
            parts: vec![craftwave_engine::storage::WirePart::Text { text: text.clone() }],
            created_at: now_ms(),
            run_id: Some(self.run_id.clone()),
            status: None,
        };
        // 落盘失败不阻断注入（帧面可用性优先，取舍同 RunRecorder——
        // 缺行在崩溃恢复扫描中补 interrupted 标记，不会产生半行）
        if let Err(e) = self
            .store
            .append_canonical(&self.session_id, &[StoredEntry::Message(entry)])
        {
            #[cfg(debug_assertions)]
            eprintln!(
                "[craftwave] steer 条目落盘失败（{} {}）：{e:?}",
                self.session_id, entry_id
            );
            #[cfg(not(debug_assertions))]
            let _ = e;
        }
        self.events.emit(
            keys::RUN_INJECT,
            serde_json::json!({
                "sessionId": self.session_id,
                "runId": self.run_id,
                "acceptedAtTurn": turn,
                "entryId": entry_id,
                "text": text,
            }),
        );
        Some(text)
    }
}

/// 自动投递新 run 的帧出口占位：自动投递时后端没有前端
/// 通道——帧枢纽以本空实现起手（帧只入环形缓冲不下发），前端收到
/// `run:start` 事件后经 attach_run（观察态）以新通道重放续流。
struct DetachedSink;

impl SequencedSink for DetachedSink {
    fn send_seq(&mut self, _frame: SequencedFrame) {}
}

/// 每拍请求刷新的当前生效快照（对比面：判断本拍是否需要换模型/换参数）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct EffectiveRuntime {
    /// 生效的模型配置 id（ProviderModel.id）。
    model_id: Option<String>,
    /// 生效的会话 thinking 档（None = 未显式设置，按 per-model 回落）。
    thinking: Option<craftwave_engine::providers::reasoning::ThinkingLevel>,
}

/// 每拍模型请求前的偏好刷新源：
/// 重读会话 prefs 文件（`<sid>.prefs.json`），模型或 thinking 变了就热更新
/// TurnDriver（换客户端 / 换请求参数 / 换图片降级开关），切到窗口更小的
/// 模型时先做预算重查（放不下即 E_CONTEXT_OVERFLOW 终止，暂停保留
/// 输入）。副作用全部收在本实现——backend 层的 `RequestRefresher` 契约
/// 只见判定结果。
///
/// 生效值 = prefs 文件值 > run 起点快照（RunConfig，合并了 start_chat 时的
/// prefs 初值）> 不设。正在进行的请求不被打断（驱动只在拍间读本实现）。
struct HostRequestRefresher {
    /// 共享句柄副本（config/store 读取面；spawn 闭包不能借用 AppState，
    /// 与 followUp 投递同款 [`AppState::share_handles`]）。
    app: AppState,
    session_id: String,
    /// 目标驱动的共享句柄（换模型/换参数的写入口）。
    driver: Arc<TurnDriver<DialectWire, rig::http_client::DynHttpClient>>,
    /// run 起点快照（RunConfig + start_chat 时的 prefs 初值合并）。
    initial: EffectiveRuntime,
    /// 当前生效快照（与上一次刷新后的驱动状态一致）。
    current: Mutex<EffectiveRuntime>,
    /// 当前生效模型槽 `(配置 id, 显示名)`（与 EventfulSink 共享——
    /// 每拍刷新时更新，usage 帧落账读到的即本拍实际执行模型）。
    current_model: Arc<Mutex<Option<(String, String)>>>,
}

impl RequestRefresher for HostRequestRefresher {
    fn before_request(&self, state: &DriveState) -> Result<(), RefreshFailure> {
        let prefs = craftwave_engine::storage::prefs::load_prefs(self.app.store.root(), &self.session_id);
        let wanted = EffectiveRuntime {
            model_id: prefs.model_id.or_else(|| self.initial.model_id.clone()),
            thinking: prefs.thinking.or(self.initial.thinking),
        };
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *current == wanted {
            return Ok(());
        }
        // —— 模型变化：重新解析 + 预算重查 + 换客户端 ——
        if current.model_id != wanted.model_id {
            let resolved = resolve_model(&self.app, wanted.model_id.as_deref())
                .map_err(|e| RefreshFailure::Failed(e.message))?;
            // 预算接缝：切换后的模型窗口须装得下最近请求的真实
            // 输入 + 新模型的回复预留；last_input=0（尚无 usage，首拍前）
            // 也不跳过——按 0 + 预留做下限检查（run 开始前已有完整编排，
            // 该场景仅剩能力面切换）。
            let estimated = state.last_input.saturating_add(resolved.max_tokens);
            if craftwave_engine::backend::estimator::is_over_wire_budget(estimated, resolved.context_window) {
                return Err(RefreshFailure::ContextOverflow {
                    estimated,
                    window: resolved.context_window,
                });
            }
            self.driver.swap_model(resolved.driver);
            // 运行中切非 vision：历史图片在请求面降级为占位，
            // 允许纯文字继续；切回 vision 模型自动恢复原图。
            self.driver.set_downgrade_images(!resolved.vision);
            // 模型槽同步（下一拍 usage 帧落账归属新模型）
            *self
                .current_model
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some((resolved.id.clone(), resolved.name.clone()));
        }
        // —— thinking 生效值重算（模型切换与档位变化都刷新；回落链以
        // 当前生效模型配置为准）——
        let model_id = wanted.model_id.clone().or(current.model_id.clone());
        // 指令与请求级 max_tokens 同源重算：Anthropic 方言恒带
        // 请求级 max_tokens（协议必填；有 thinking 预算时按 上调
        // 规则），OpenAI 系维持 None（请求不带 max_tokens，不回归）
        let (options, max_tokens) = match model_id.as_deref() {
            Some(id) => {
                let resolved = resolve_model(&self.app, Some(id))
                    .map_err(|e| RefreshFailure::Failed(e.message))?;
                let directive = effective_thinking_directive(wanted.thinking, &resolved);
                (
                    craftwave_engine::backend::turn::reasoning_directive_to_options(&directive),
                    craftwave_engine::backend::turn::directive_max_tokens(
                        &directive,
                        resolved.max_tokens,
                        resolved.api_format,
                    ),
                )
            }
            // 无模型可解析（防御面）：不传参数
            None => {
                let directive = craftwave_engine::providers::reasoning::ReasoningDirective::none();
                (
                    craftwave_engine::backend::turn::reasoning_directive_to_options(&directive),
                    None,
                )
            }
        };
        self.driver.update_spec(|spec| {
            spec.options = options;
            spec.max_tokens = max_tokens;
        });
        *current = wanted;
        Ok(())
    }
}

/// 取走并清空某会话活跃 run 的 steer 未消费残余（run 收尾分流用：
/// Done → 转会话 followUp 队列尾；取消/失败 → 随 run:queue.returned 交还）。
fn drain_steer(handle: &Arc<RunHandle>) -> Vec<String> {
    handle
        .steer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .drain(..)
        .collect()
}

/// `run:queue` 事件发射：载荷 = 快照 + 可选
/// `returned` 交还文本数组（取消清空/失败交还时出现）。
fn emit_queue_state(
    events: &Arc<dyn EventSink>,
    session_id: &str,
    followups: &FollowUpManager,
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

/// 审批挂起 → AskPayload(kind=approval)：write=新文件全 add、edit=替换 hunk、
/// command=命令行 + fenceReason。`session_memory_enabled`
/// （会话内记忆启用）为 true 且待批调用是命令类时，选项增「始终允许
/// （本会话）」（mode=always_session）——选中后同签名命令本会话跨 run 免批
/// （记忆在会话级审批记忆桶，不持久化）。`plan_scope_note`
/// 为已批准计划基线的人话摘要，范围门挂起（pending.plan_scope）时随范围卡
/// 展示对照；其他形态传 None。`audit_note`为灰区命令审计
/// 结论注记（ask/manual），随普通审批卡展示（范围门/外部目录/MCP 等非灰区
/// 卡不带）；allow/block 不产生审批卡，不经本参数承载。
pub(crate) fn build_approval_payload(
    pending: &PendingCall,
    session_memory_enabled: bool,
    plan_scope_note: Option<&str>,
    audit_note: Option<&craftwave_engine::backend::ask::AuditNote>,
) -> AskPayload {
    // 计划范围卡：计划外写路径 + 批准基线对照 +
    // 三选项（允许一次 / 本会话始终允许（父目录入基线）/ 拒绝）。无推荐项
    // ——不参与 autoConfirm 自动批准（计划范围须用户明确同意，与外部目录
    // 授权同精神）；command 场景附命令行原文（用户需知道执行什么）；围栏
    // Confirm 待决时一并附上（门判定层吸收——一次交互覆盖本次调用的全部
    // 审批）。
    if let Some(path) = &pending.plan_scope {
        let mut reason = format!("写入路径「{path}」不在已批准计划的范围内");
        if let Some(note) = plan_scope_note {
            reason.push_str(&format!("（批准范围：{note}）"));
        }
        if let Some(fence) = &pending.fence_reason {
            reason.push_str(&format!("；{fence}"));
        }
        let command = if pending.name == "command" {
            serde_json::from_str::<serde_json::Value>(&pending.args_json)
                .ok()
                .and_then(|v| v.get("command").and_then(|c| c.as_str()).map(str::to_owned))
        } else {
            None
        };
        return AskPayload::Approval {
            title: "计划范围确认".to_owned(),
            diff: None,
            command,
            fence_reason: Some(reason),
            preview_html: None,
            plan_scope: Some(path.clone()),
            audit: None,
            options: vec![
                AskOption {
                    id: "allow_once".to_owned(),
                    label: "允许（仅本次）".to_owned(),
                    recommended: None,
                    mode: Some(AskMode::AllowOnce),
                },
                AskOption {
                    id: "always_session".to_owned(),
                    label: "本会话始终允许（写入所在目录）".to_owned(),
                    recommended: None,
                    mode: Some(AskMode::AlwaysSession),
                },
                AskOption {
                    id: "deny".to_owned(),
                    label: "拒绝".to_owned(),
                    recommended: None,
                    mode: Some(AskMode::Deny),
                },
            ],
        };
    }
    // 外部目录授权卡：路径 + 粒度说明 + 允许/拒绝。授权
    // 仅本会话有效（会话 roots 快照），无 recommended——不参与 autoConfirm。
    if let Some(dir) = &pending.external_dir {
        return AskPayload::ExternalDir {
            path: dir.clone(),
            options: vec![
                AskOption {
                    id: "allow".to_owned(),
                    label: "始终允许（本会话）".to_owned(),
                    recommended: None,
                    mode: None,
                },
                AskOption {
                    id: "deny".to_owned(),
                    label: "拒绝".to_owned(),
                    recommended: None,
                    mode: Some(AskMode::Deny),
                },
            ],
        };
    }
    let args: serde_json::Value = serde_json::from_str(&pending.args_json).unwrap_or_default();
    // MCP 动态工具分支：title 带 server 名与工具名；
    // 选项含「始终允许」（持久许可的授予入口；恢复循环
    // 侧写 trust/mcp-tools.json）。MCP 审批无 recommended——不参与
    // autoConfirm 自动批准。归一名反查失败（极窄窗口：断开已摘除动态段）
    // 落到下方通用审批标题兜底。
    if let Some(identity) = craftwave_engine::mcp::tools::McpToolRegistry::global().identity(&pending.name) {
        return AskPayload::Approval {
            title: format!(
                "MCP 调用审批：服务器 {server} 的工具 {tool}",
                server = identity.server_name,
                tool = identity.original_name
            ),
            diff: None,
            command: Some(format!("{} {}", pending.name, pending.args_json)),
            fence_reason: pending.fence_reason.clone(),
            preview_html: None,
            plan_scope: None,
            audit: None,
            options: vec![
                AskOption {
                    id: "allow_once".to_owned(),
                    label: "仅本次允许".to_owned(),
                    recommended: None,
                    mode: Some(AskMode::AllowOnce),
                },
                AskOption {
                    id: "always_allow".to_owned(),
                    label: "始终允许（当前版本）".to_owned(),
                    recommended: None,
                    mode: None,
                },
                AskOption {
                    id: "deny".to_owned(),
                    label: "拒绝".to_owned(),
                    recommended: None,
                    mode: Some(AskMode::Deny),
                },
            ],
        };
    }
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
        // web 工具审批（Network 行）：标题承载方法 + URL 摘要行
        "web_fetch" => "抓取网页".to_owned(),
        "http_request" => "发送 HTTP 请求".to_owned(),
        other => format!("执行工具 {other}"),
    };
    let mut diff = None;
    let mut command = None;
    // HTML 预览附带：write/edit 目标扩展名为 .html/.htm
    // 且内容 ≤50,000 字符时，把产出全文附进审批载荷——审批卡「查看预览」
    // 以 render_html 同款沙箱渲染。超限/其他扩展名不附带
    // （载荷不膨胀，diff 仍是审批主预览面）。
    let mut preview_html = None;
    let html_target = |path: &str| {
        matches!(
            std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("html" | "htm")
        )
    };
    match pending.name.as_str() {
        "write" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if html_target(path)
                && content.chars().count() <= craftwave_engine::tools::render_html::MAX_HTML_CHARS
            {
                preview_html = Some(content.to_owned());
            }
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
            if html_target(path) && new.chars().count() <= craftwave_engine::tools::render_html::MAX_HTML_CHARS
            {
                // 预览语义 = 编辑后的效果：附替换后全文（newText）。
                preview_html = Some(new.to_owned());
            }
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
        // web 工具：command 字段承载 `GET https://…` 摘要行
        // （审批弹窗复用命令行展示位，AskPanel 零改动）
        "web_fetch" => {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            command = Some(format!("GET {url}"));
        }
        "http_request" => {
            let method = args
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("GET")
                .to_ascii_uppercase();
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            command = Some(format!("{method} {url}"));
        }
        _ => {}
    }
    // 通用审批选项：允许（推荐，autoConfirm 超时选它）+ 拒绝；命令类审批
    // 且 ③ 会话内记忆开启时插「始终允许（本会话）」。
    // 非推荐项——autoConfirm 只自动选 allow_once，会话级免批必须用户显式点。
    let mut options = vec![AskOption {
        id: "allow_once".to_owned(),
        label: "允许".to_owned(),
        recommended: Some(true),
        mode: Some(AskMode::AllowOnce),
    }];
    if session_memory_enabled && pending.name == "command" {
        options.push(AskOption {
            id: "always_session".to_owned(),
            label: "始终允许（本会话）".to_owned(),
            recommended: None,
            mode: Some(AskMode::AlwaysSession),
        });
    }
    options.push(AskOption {
        id: "deny".to_owned(),
        label: "拒绝".to_owned(),
        recommended: None,
        mode: Some(AskMode::Deny),
    });
    AskPayload::Approval {
        title,
        diff,
        command,
        fence_reason: pending.fence_reason.clone(),
        preview_html,
        plan_scope: None,
        audit: audit_note.cloned(),
        options,
    }
}

/// 挂起等待的裁决（option_id 供 MCP「始终允许」的持久许可授予面，
/// always_session 供会话级免批登记面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApprovalResolution {
    /// 批准（option_id：allow_once / always_session / always_allow /
    /// autoConfirm 的 recommended 项）。
    Allowed { option_id: String },
    /// 拒绝/取消。
    Denied,
}

/// 挂起等待审批：注册 AskSlot + ask:opened → 等 resolve_ask / 取消 /
/// autoConfirm 超时。等待结束统一清槽 + ask:closed。
/// 主 run 与子代理共用：取消令牌与询问槽由调用方注入
/// （主 run = RunHandle.cancel/ask；子代理 = SubHandle.cancel/ask）。
/// `session_memory_enabled`：true 时命令类审批载荷增
/// 「始终允许（本会话）」选项。`plan_scope_note`：范围门
/// 挂起时随载荷展示的批准基线对照摘要（宿主从共享计划状态拼装）。
/// `audit_note`：灰区命令审计结论注记（ask/manual），
/// 随普通审批卡展示；None = 未经审计或审计已放行/拒绝（不产生审批卡）。
/// `session_log`：审批开关两行的落点（kind=approval：opened/
/// closed + answered）；None = 测试直驱路径不记。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn wait_approval_with(
    events: &Arc<dyn EventSink>,
    session_id: &str,
    run_id: &str,
    cancel: &CancellationToken,
    ask_slot: &Arc<AskSlotCell>,
    pending: &PendingCall,
    auto_confirm: bool,
    session_memory_enabled: bool,
    plan_scope_note: Option<&str>,
    audit_note: Option<&craftwave_engine::backend::ask::AuditNote>,
    session_log: Option<&Arc<craftwave_engine::session_log::SessionLogger>>,
) -> ApprovalResolution {
    let (tx, rx) = tokio::sync::oneshot::channel::<AskAnswer>();
    let ask_id = uuid::Uuid::new_v4().to_string();
    let payload =
        build_approval_payload(pending, session_memory_enabled, plan_scope_note, audit_note);
    {
        let mut slot = ask_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 同一时刻只允许一个未决询问：槽位已被占用则直接拒绝（防御性）
        if slot.is_some() {
            return ApprovalResolution::Denied;
        }
        *slot = Some(AskSlot {
            ask_id: ask_id.clone(),
            session_id: session_id.to_owned(),
            run_id: run_id.to_owned(),
            kind: "approval",
            // 原始载荷留存：重连后的待审批恢复查询据此展示
            // 原始操作及参数（diff/命令/围栏原因与选项）
            payload: payload.clone(),
            tx,
        });
    }
    events.emit(
        keys::ASK_OPENED,
        serde_json::json!({
            "sessionId": session_id,
            "askId": ask_id,
            "payload": payload,
        }),
    );
    // 审批打开行（kind=approval：askId + 操作名；载荷内容不进
    // 日志——参数里可能含正文，审批详情走帧面与审批卡）
    if let Some(logger) = session_log {
        let msg = format!("等待审批：{}", pending.name);
        logger.session_line(
            session_id,
            craftwave_engine::providers::types::LogLevel::Info,
            craftwave_engine::session_log::LogKind::Approval,
            msg,
            Some(serde_json::json!({ "tool": pending.name })),
            Some(run_id),
            Some(&ask_id),
            None,
        );
    }

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
        _ = cancel.cancelled() => Wait::Cancelled,
        // autoConfirm 300s 只对「有推荐项（allow_once）」的载荷生效——
        // mcpTrust 等无 recommended 的信任询问不参与自动批准
        // （高危外部调用永远过人）。
        _ = tokio::time::sleep(AUTO_CONFIRM_TIMEOUT),
            if auto_confirm && payload.recommended_option_id().is_some() => Wait::AutoConfirm,
        answer = rx => Wait::Answer(answer.ok()),
    };
    let (resolution, answered) = match wait {
        Wait::Answer(Some(AskAnswer::Approval { option_id, .. })) => {
            // allow_once / always_session（本会话免批）/
            // always_allow（MCP）/ allow（外部目录授权）
            // 都放行本次；其余选项按拒绝收场
            let allowed = option_id == "allow_once"
                || option_id == "always_session"
                || option_id == "always_allow"
                || option_id == "allow";
            (
                if allowed {
                    ApprovalResolution::Allowed { option_id }
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
        // （Block 不会挂起），灾难级例外天然满足；无 recommended 的
        // 载荷（mcpTrust/MCP 审批）已由 select 条件排除，不会走到这里
        Wait::AutoConfirm => (
            ApprovalResolution::Allowed {
                option_id: "allow_once".to_owned(),
            },
            true,
        ),
    };
    // 单一出口清槽 + ask:closed（resolve_ask 只投递不清理）
    *ask_slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    events.emit(
        keys::ASK_CLOSED,
        serde_json::json!({ "askId": ask_id, "answered": answered }),
    );
    // 审批收口行（kind=approval：answered + 裁决摘要）
    if let Some(logger) = session_log {
        let outcome = match &resolution {
            ApprovalResolution::Allowed { option_id } => format!("已批准（{option_id}）"),
            ApprovalResolution::Denied if answered => "已拒绝".to_owned(),
            ApprovalResolution::Denied => "未回答（取消）".to_owned(),
        };
        let msg = format!("审批收口：{outcome}");
        logger.session_line(
            session_id,
            craftwave_engine::providers::types::LogLevel::Info,
            craftwave_engine::session_log::LogKind::Approval,
            msg,
            Some(serde_json::json!({ "answered": answered, "outcome": outcome })),
            Some(run_id),
            Some(&ask_id),
            None,
        );
    }
    resolution
}

/// 拒绝/取消的收场：挂起调用补拒绝结果帧 + runEnded(cancelled)（
/// AwaitingApproval → Cancelled；run 已序列化，模型侧不再收下一轮，帧面
/// 补 E_CANCELLED 结果让前端工具卡不悬空）。主 run 与子代理
/// 共用（DriveState 自带帧序映射与计数，无 run 句柄依赖）。
pub(crate) fn denied_finish(sink: &mut dyn FrameSink, pending: &PendingCall, state: &DriveState) {
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

/// 审计总等待上限：含证据收集与模型请求，固定
/// 15 秒，不可配置。
pub(crate) const AUDIT_TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
/// 审计回复上限：一句简短中文依据足够，防模型失控长输出烧钱。
pub(crate) const AUDIT_MAX_TOKENS: u64 = 256;

/// 一次审计尝试的产物（用量记账面）：已获得的用量与执行审计的模型。
/// `usage` = None 表示模型请求未返回（超时/请求失败），不伪造未知用量。
struct AuditAttempt {
    usage: Option<UsageTotal>,
    model: (String, String),
}

/// 审计流程的退出形态。
enum AuditFlow {
    /// 模型给出合法三态结论。
    Verdict(craftwave_engine::backend::audit::AuditVerdict, AuditAttempt),
    /// 审计失败/超时/输出无效——转人工（reason = 失败原因，审批卡展示）。
    Failed(String, AuditAttempt),
    /// 用户取消（run 令牌触发），审计终止。
    Cancelled,
}

/// 灰区命令审计编排（字段级）：模型解析 → 证据收集（会话
/// roots 内只读）→ 提示词构建 → 单次无工具调用（与 15 秒总超时、run 取消
/// 令牌 select! 竞争，超时丢弃在飞请求不留后台消耗）。调用前提：宿主已用
/// `ApprovalGate::audit_class` 判定挂起项属灰区。审计执行（本函数）不判断
/// 结论去向——放行/拒绝/转人工由宿主恢复循环分流。
///
/// `session_model_id` = 会话当前模型槽的配置 id（`commandAudit.modelId`
/// 未配置时跟随）；`plan_baseline` = 已批准计划基线路径（批准范围证据）；
/// `total_timeout` 传入 [`AUDIT_TOTAL_TIMEOUT`]（测试可传短超时）。
#[allow(clippy::too_many_arguments)]
async fn run_command_audit(
    app: &AppState,
    session_id: &str,
    pending: &PendingCall,
    session_model_id: Option<&str>,
    user_goal: &str,
    plan_baseline: &[String],
    cancel: &CancellationToken,
    total_timeout: Duration,
) -> AuditFlow {
    // ① 模型解析：专用审计模型优先；未配置跟随会话当前模型。
    //    指定模型解析失败按审计失败转人工并显示原因，不静默换供应商。
    let audit_model_id = app.config.load().ok().and_then(|c| {
        c.agent
            .as_ref()
            .and_then(|a| a.command_audit.as_ref())
            .and_then(|cmd| cmd.model_id.clone())
    });
    let model_id = audit_model_id.or_else(|| session_model_id.map(str::to_owned));
    let resolved = match model_id.as_deref() {
        Some(id) => match resolve_model(app, Some(id)) {
            Ok(m) => m,
            Err(e) => {
                return AuditFlow::Failed(
                    format!("审计模型不可用（未静默换供应商）：{e}"),
                    AuditAttempt {
                        usage: None,
                        model: (String::new(), String::new()),
                    },
                );
            }
        },
        None => {
            return AuditFlow::Failed(
                "审计模型不可用：未配置审计模型且会话无当前模型".to_owned(),
                AuditAttempt {
                    usage: None,
                    model: (String::new(), String::new()),
                },
            );
        }
    };
    let attempt_model = (resolved.id.clone(), resolved.name.clone());

    // ② 证据收集（授权目录内只读——AST 路径目标片段 + 命令定义 +
    //    批准范围；cwd 基准与审批门 judge_base 同款）。
    let args: serde_json::Value = serde_json::from_str(&pending.args_json).unwrap_or_default();
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let roots = craftwave_engine::session_roots::resolve_session_roots(app, session_id);
    // cwd 基准先解析（项目主目录为缺省基准，与审批门 judge_base 同款），
    // 再把根集合收进归一列表（roots 字段被移动前的克隆面）。
    let project_root = roots.project_root.clone();
    let cwd_raw = args.get("cwd").and_then(|v| v.as_str());
    let base = match cwd_raw {
        Some(c) if !c.is_empty() => {
            let p = std::path::Path::new(c);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                project_root.join(p)
            }
        }
        _ => project_root.clone(),
    };
    let root_paths: Vec<std::path::PathBuf> = [project_root, roots.workspace_root]
        .into_iter()
        .chain(roots.extra_dirs)
        .collect();
    // 判定同域归一（审批门装配同款）：canonicalize 失败的根跳过。
    let canonical_roots: Vec<std::path::PathBuf> = root_paths
        .iter()
        .filter_map(|p| craftwave_engine::safety::safe_join::canonicalize_best_effort(p).ok())
        .collect();
    let collector = craftwave_engine::backend::audit::EvidenceCollector {
        roots: &canonical_roots,
        base: &base,
    };
    let evidence = collector.collect(&command);
    let root_list = canonical_roots
        .iter()
        .map(|r| r.display().to_string())
        .collect::<Vec<_>>()
        .join("、");
    let scope_text = if plan_baseline.is_empty() {
        format!("已授权目录：{root_list}")
    } else {
        format!(
            "已批准计划范围：{}\n已授权目录：{root_list}",
            plan_baseline.join("、")
        )
    };
    let prompt =
        craftwave_engine::backend::audit::build_audit_prompt(&craftwave_engine::backend::audit::AuditPromptInput {
            command: &command,
            cwd: &base.display().to_string(),
            evidence: &evidence,
            user_goal: Some(user_goal),
            scope: Some(&scope_text),
        });

    // ③ 单次无工具调用：与总超时、取消令牌竞争（select 丢弃 future 即关闭
    //    在飞 HTTP 请求——超时不留后台审计继续消耗，取消传播终止模型请求）。
    let driver = Arc::new(TurnDriver::new(resolved.driver, RunSpec::new(), Vec::new()));
    let work = driver.complete_once_capped(&prompt, AUDIT_MAX_TOKENS);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => AuditFlow::Cancelled,
        _ = tokio::time::sleep(total_timeout) => AuditFlow::Failed(
            format!(
                "审计超时：{} 秒总预算（含证据收集与模型请求）已到，转人工确认",
                total_timeout.as_secs().max(1)
            ),
            AuditAttempt { usage: None, model: attempt_model },
        ),
        result = work => match result {
            Ok((text, usage)) => {
                let attempt = AuditAttempt {
                    usage: Some(craftwave_engine::backend::turn::usage_total(&usage)),
                    model: attempt_model,
                };
                match craftwave_engine::backend::audit::parse_verdict(&text) {
                    Some(verdict) => AuditFlow::Verdict(verdict, attempt),
                    None => AuditFlow::Failed(
                        craftwave_engine::backend::audit::bad_output_reason(&text),
                        attempt,
                    ),
                }
            }
            Err(e) => AuditFlow::Failed(
                format!("审计模型请求失败：{e}"),
                AuditAttempt { usage: None, model: attempt_model },
            ),
        },
    }
}

/// 辅助小调用的会话记账（suggest 与 审计共用）：已知实际用量
/// 落账（日/模型聚合 + 明细 kind=aux + 缓存写穿；runs 计数不增）+
/// tokens:update 全量校正事件。零计量不落账（未知用量不伪造）。
fn record_aux_usage(
    events: &Arc<dyn EventSink>,
    session_usage: &Arc<Mutex<HashMap<String, UsageTotal>>>,
    ledger: &craftwave_engine::usage::UsageLedger,
    session_id: &str,
    aux_run_id: &str,
    model: &(String, String),
    delta: &UsageTotal,
) {
    if *delta == UsageTotal::default() {
        return;
    }
    ledger.record(
        session_id,
        aux_run_id,
        craftwave_engine::usage::run_kind::AUX,
        &model.0,
        &model.1,
        *delta,
        false,
    );
    let total = {
        let mut map = session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let total = map.entry(session_id.to_owned()).or_default();
        total.accumulate(delta);
        *total
    };
    let mut payload = serde_json::json!({ "sessionId": session_id, "usage": total });
    if !model.0.is_empty() {
        payload["modelId"] = serde_json::json!(model.0);
    }
    events.emit(keys::TOKENS_UPDATE, payload);
}

/// 审计拒绝的收场：E_AUDIT_BLOCKED 工具结果帧（附模型
/// 简短依据）+ runEnded(error)——followUp 队列按失败暂停（宿主侧由调用方
/// 记录 errored 与 RunExit::Failed）。
fn audit_block_finish(
    sink: &mut dyn FrameSink,
    frame_call_id: &str,
    state: &DriveState,
    reason: &str,
) {
    sink.send(ChatFrame::ToolResult {
        call_id: frame_call_id.to_owned(),
        ok: false,
        ui_summary: format!("审计拒绝：{reason}"),
        error: Some(FrameError::new(
            craftwave_engine::util::errors::E_AUDIT_BLOCKED,
            format!("命令审计模型判断高风险，已拒绝执行：{reason}"),
        )),
    });
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Error,
        duration_ms: state
            .started_at
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0),
        tool_count: state.tool_count,
        turn: state.last_turn,
        error: Some(FrameError::new(
            craftwave_engine::util::errors::E_AUDIT_BLOCKED,
            format!("命令审计模型判断高风险，已拒绝执行：{reason}"),
        )),
    });
}

/// 审计期间用户取消的收场（取消向挂起 run 传播）：E_CANCELLED 工具结果
/// 帧 + runEnded(cancelled)（与用户拒绝同收场形态，摘要区分审计场景）。
fn audit_cancelled_finish(sink: &mut dyn FrameSink, frame_call_id: &str, state: &DriveState) {
    sink.send(ChatFrame::ToolResult {
        call_id: frame_call_id.to_owned(),
        ok: false,
        ui_summary: "已取消（审计终止）".to_owned(),
        error: Some(FrameError::new(E_CANCELLED, "用户取消了此操作（审计终止）")),
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

/// suggest 后台小调用：一次非流式补全 → run:suggestions；失败静默、
/// 空文本静默；用量计入会话累计（tokens:update，不计 turnUsage——run 已终态）。
/// 驱动经共享句柄传入（run 任务收尾时把 Arc 交给本任务，suggest
/// 以 run 终态时的最新模型客户端执行）。已知实际用量同步落账
/// （日/模型聚合 + 明细 kind=aux + 缓存写穿；runs 计数不增）——失败
/// /无计量的未知用量不伪造，已返回的用量不因辅助身份被丢弃。
/// `final_model` = run 终态时的模型槽快照 `(配置 id, 显示名)`。
#[allow(clippy::too_many_arguments)]
fn spawn_suggest<W, T>(
    driver: Arc<TurnDriver<W, T>>,
    events: Arc<dyn EventSink>,
    session_usage: Arc<Mutex<HashMap<String, UsageTotal>>>,
    ledger: craftwave_engine::usage::UsageLedger,
    session_id: String,
    run_id: String,
    context_tail: String,
    final_model: (String, String),
) where
    W: rig::wire::Wire<Op = rig::operation::Completion>,
    T: rig::driver::Transport<W>,
{
    tokio::spawn(async move {
        if context_tail.trim().is_empty() {
            return;
        }
        let prompt = format!(
            "根据下面这段对话结尾，生成一句简短的中文后续输入建议（ Composer 预填文案，\
             10–20 字，直接输出建议本身，不要引号和解释）：\n{context_tail}"
        );
        let Ok((text, usage)) = driver.complete_once(&prompt).await else {
            return; // 失败静默放弃（不重试、不提示）
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        // 用量计入会话累计（帧通道已收，走事件通道补计；辅助调用落账
        // 已返回 usage 必记账，runs 计数不增——记账面与审计共用 record_aux_usage）
        let delta = craftwave_engine::backend::turn::usage_total(&usage);
        record_aux_usage(
            &events,
            &session_usage,
            &ledger,
            &session_id,
            &format!("{run_id}:suggest"),
            &final_model,
            &delta,
        );
        events.emit(
            keys::RUN_SUGGESTIONS,
            serde_json::json!({ "sessionId": session_id, "runId": run_id, "text": trimmed }),
        );
    });
}

// ---- 自动起名（后台零工具小调用，suggest 同模式）----

/// 标题生成的调用超时（口径 10s；超时 → 首条消息截断兜底，不重试不提示）。
pub(crate) const TITLE_TIMEOUT: Duration = Duration::from_secs(10);

/// 标题生成调用的回复上限（口径 max_tokens 32——防模型失控长输出）。
pub(crate) const TITLE_MAX_TOKENS: u64 = 32;

/// 标题最大字符数（超时兜底截断 ≤30 字符口径；生成结果同限归一，
/// 模型无视 ≤20 字要求时兜住长度）。
pub(crate) const TITLE_MAX_CHARS: usize = 30;

/// 进入 prompt 的首条消息文本上限（口径：超长截 2000 字符）。
const TITLE_PROMPT_INPUT_CHARS: usize = 2000;

/// 触发判定（三态表）：仅当 index 标题仍是 create_session 默认值且
/// 树内恰好只有这一条 user 消息时自动起名。三个不触发面由此一并覆盖——
/// 用户改名过（title 非默认）、非首条消息（user 计数 > 1，含首条后编辑
/// 重发产生的兄弟分支节点）、重启后继续旧会话（重启不重生成由判定天然
/// 保证，无需额外标记位）。
pub(crate) fn title_should_trigger(meta_title: Option<&str>, user_message_count: usize) -> bool {
    meta_title == Some(crate::host::commands::session::DEFAULT_SESSION_TITLE)
        && user_message_count == 1
}

/// 标题归一（生成结果与兜底共用）：trim → 取第一个非空行 → 超
/// [`TITLE_MAX_CHARS`] 字符截断（按字符取，多字节安全）→ 再 trim。
/// 兜底输入 = 首条消息全文（`lines()` 天然完成"首行截断 + 换行/前后
/// 空白归一"）。
pub(crate) fn normalize_title(raw: &str) -> String {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let truncated: String = line.chars().take(TITLE_MAX_CHARS).collect();
    truncated.trim().to_owned()
}

/// 标题生成 prompt：输入 = 首条 user
/// 消息文本，超长截 [`TITLE_PROMPT_INPUT_CHARS`] 字符；要求输出 ≤20 字
/// 标题、无引号无前缀。
pub(crate) fn title_prompt(first_user_text: &str) -> String {
    let clipped: String = first_user_text
        .chars()
        .take(TITLE_PROMPT_INPUT_CHARS)
        .collect();
    format!(
        "根据用户的这条消息，给这个 AI 助手会话起一个简短标题。要求：不超过 20 个字；\
         直接输出标题本身，不要引号、不要任何前缀或解释。消息内容：{clipped}"
    )
}

/// 首条 user 消息的正文文本（Text parts 顺序拼接；纯图片等无正文场景
/// 返回空串，调用方据此放弃起名）。
pub(crate) fn first_user_text(messages: &[MessageEntry]) -> String {
    messages
        .iter()
        .find(|m| m.role == MessageRole::User)
        .map(|m| {
            m.parts
                .iter()
                .filter_map(|p| match p {
                    craftwave_engine::storage::WirePart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// 后台标题任务（spawn 模式复制 [`spawn_suggest`] 先例
/// ——run 落盘链外独立任务、失败静默、用量计入会话）：
///
/// ① 锁内复判一（并发防护）：进入任务即查 index——标题已非默认
///    （用户抢先改名 / 并发任务已写）直接放弃；
/// ② 生成：[`TurnDriver::complete_once_capped`]（零工具、max_tokens 32）+
///    10s 超时；成功 → 归一后采用；超时/失败/空文本 → 兜底 = 首条消息
///    首行截断。成功调用已返回的 usage 照常记账（
///    已知用量不因后续路径丢弃；无 usage 不伪造，不能承诺失败免费）——
///    kind=aux、runId=`{runId}:title`、runs 计数不增，
///    tokens:update 照发（按当时本地日期落账，由 ledger 承担）；
/// ③ 锁内复判二（先写者为准）：落盘前再查一次 index——已被并发任务写入
///    则放弃（极端窗口下后写者可能覆盖，同值无害）；
/// ④ 落盘双写：meta 行（会话锁，`update_session_title`）→ index mutate
///    （title + updatedAt；锁序 session → index）；
/// ⑤ `session:title` 事件——严格在落盘之后发。
///
/// 全程失败静默：不报错、不提示、不重试；落盘失败不发事件
/// （事件承诺"已落盘"）。重启恢复遵守四口径记录（明细面已落账）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_title_task<W, T>(
    driver: Arc<TurnDriver<W, T>>,
    store: Arc<craftwave_engine::storage::SessionStore>,
    events: Arc<dyn EventSink>,
    session_usage: Arc<Mutex<HashMap<String, UsageTotal>>>,
    ledger: craftwave_engine::usage::UsageLedger,
    session_id: String,
    run_id: String,
    first_user_text: String,
    model: (String, String),
    timeout: Duration,
) where
    W: rig::wire::Wire<Op = rig::operation::Completion>,
    T: rig::driver::Transport<W>,
{
    tokio::spawn(async move {
        let default_title = crate::host::commands::session::DEFAULT_SESSION_TITLE;
        // index 标题仍为默认值的原子复判（SessionIndex 内部互斥保证读一致）
        let still_default = || {
            store
                .index()
                .get(&session_id)
                .map(|m| m.title == default_title)
                .unwrap_or(false)
        };
        // ① 复判一：已非默认（改名 / 并发已写）→ 放弃
        if !still_default() {
            return;
        }
        // ② 生成（超时兜底；usage 记账与标题取舍两段独立——已知用量不丢）
        let prompt = title_prompt(&first_user_text);
        let title = match tokio::time::timeout(
            timeout,
            driver.complete_once_capped(&prompt, TITLE_MAX_TOKENS),
        )
        .await
        {
            Ok(Ok((text, usage))) => {
                let delta = craftwave_engine::backend::turn::usage_total(&usage);
                if delta != UsageTotal::default() {
                    ledger.record(
                        &session_id,
                        &format!("{run_id}:title"),
                        craftwave_engine::usage::run_kind::AUX,
                        &model.0,
                        &model.1,
                        delta,
                        false,
                    );
                    let total = {
                        let mut map = session_usage
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let total = map.entry(session_id.clone()).or_default();
                        total.accumulate(&delta);
                        *total
                    };
                    let mut payload =
                        serde_json::json!({ "sessionId": session_id, "usage": total });
                    if !model.0.is_empty() {
                        payload["modelId"] = serde_json::json!(model.0);
                    }
                    events.emit(keys::TOKENS_UPDATE, payload);
                }
                let generated = normalize_title(&text);
                if generated.is_empty() {
                    normalize_title(&first_user_text) // 空回复视为失败走兜底
                } else {
                    generated
                }
            }
            // 超时 / 调用失败：无 usage 不伪造，标题走静默兜底
            _ => normalize_title(&first_user_text),
        };
        if title.is_empty() {
            return; // 无标题可落（首条消息全空白等极端面），静默放弃
        }
        // ③ 复判二：先写者为准
        if !still_default() {
            return;
        }
        // ④ 落盘双写（meta 行先行——事件承诺"已落盘"；index 失败仅记日志，
        //    下次 canonical 落盘 upsert 时以 prior 补齐，尽力而为面）
        if store.update_session_title(&session_id, &title).is_err() {
            return;
        }
        if let Err(e) = store.index().mutate(&session_id, |m| {
            m.title = title.clone();
            m.updated_at = now_ms();
        }) {
            #[cfg(debug_assertions)]
            eprintln!("[craftwave] 标题 index 更新失败（{session_id}）：{e:?}");
            #[cfg(not(debug_assertions))]
            let _ = e;
        }
        // ⑤ 事件（严格在落盘之后）
        events.emit(
            keys::SESSION_TITLE,
            serde_json::json!({ "sessionId": session_id, "title": title }),
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
                craftwave_engine::storage::WirePart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>()
    };
    messages
        .iter()
        .rev()
        .find(|m| m.role == craftwave_engine::storage::MessageRole::Assistant)
        .map(text_of)
        .filter(|t| !t.trim().is_empty())
        .or_else(|| messages.last().map(text_of))
        .unwrap_or_default()
}

/// 压缩运行时策略快照（配置 + 模型窗口 + 回复预留）。
#[derive(Debug, Clone)]
pub(crate) struct CompactionRuntimePolicy {
    /// 自动压缩阈值（窗口占比，compaction.threshold）。
    pub threshold: f64,
    /// 压缩总超时（分块共用）。
    pub timeout: Duration,
    /// 当前模型上下文窗口（contextWindow）。
    pub context_window: u64,
    /// 回复预留（maxTokens，预算组成）。
    pub reply_reserve: u64,
    /// 摘要模型的上下文窗口（分块装箱面；专用小模型未配时 = 当前模型窗口）。
    pub summary_window: u64,
}

/// 压缩结果（提交面）：新压缩边界（tokensBefore/tokensAfter 由调用方按
/// 各自触发点的估算基准填入帧与事件载荷）。
#[derive(Debug, Clone)]
pub(crate) struct CompactionOutcome {
    pub boundary: craftwave_engine::backend::wire::CompactionBoundary,
}

/// 从 AppConfig 读压缩策略（模型窗口取当前 run 模型；摘要专用模型未配时
/// 摘要窗口 = 当前模型窗口）。
pub(crate) fn compaction_policy(app: &AppState, model: &ResolvedModel) -> CompactionRuntimePolicy {
    let config = app
        .config
        .load()
        .map(|c| c.compaction.clone())
        .unwrap_or_default();
    // 摘要专用小模型未配置时跟随当前模型，窗口同取当前模型
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

/// 把压缩摘要调用的实际用量计入会话累计（tokens:update 照发、
/// 不计 turnUsage——压缩不属主 run 轮次）。共享表 + 事件汇为细粒度参数
/// （run 任务的 spawn 闭包里没有 AppState 整体，只有这些克隆件）。
/// 已知实际用量同步落账（kind=aux，`{runId}:compact`；runs 计数
/// 不增，未知用量不伪造）。
#[allow(clippy::too_many_arguments)]
fn accumulate_compaction_usage(
    session_usage: &Mutex<HashMap<String, UsageTotal>>,
    events: &Arc<dyn EventSink>,
    ledger: &craftwave_engine::usage::UsageLedger,
    session_id: &str,
    run_id: &str,
    summary_model: (&str, &str),
    usages: &[rig::completion::Usage],
) {
    let mut delta = UsageTotal::default();
    for usage in usages {
        delta.accumulate(&craftwave_engine::backend::turn::usage_total(usage));
    }
    if delta == UsageTotal::default() {
        return; // 无计量（mock/未知用量不伪造）
    }
    ledger.record(
        session_id,
        &format!("{run_id}:compact"),
        craftwave_engine::usage::run_kind::AUX,
        summary_model.0,
        summary_model.1,
        delta,
        false,
    );
    let total = {
        let mut map = session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let total = map.entry(session_id.to_owned()).or_default();
        total.accumulate(&delta);
        *total
    };
    let mut payload = serde_json::json!({ "sessionId": session_id, "usage": total });
    if !summary_model.0.is_empty() {
        payload["modelId"] = serde_json::json!(summary_model.0);
    }
    events.emit(keys::TOKENS_UPDATE, payload);
}

/// 压缩后的上下文占用估算（tokensAfter 口径）：裁剪后条目 + 摘要
/// 前缀 + 工具表 + 回复预留。
pub(crate) fn estimate_after_compaction(
    remaining_entries: &[MessageEntry],
    summary: &str,
    tools: &[(String, String, String)],
    reserve: u64,
) -> u64 {
    craftwave_engine::backend::estimator::estimate_request_tokens(
        &[summary],
        remaining_entries,
        "",
        tools,
        reserve,
    )
}

/// 对会话执行一次压缩（核心编排）：
/// ① 摘要调用（增量输入 = 旧摘要 + 新增内容；分块共用总超时）；
/// ② `append_compaction` 原子落盘（含首次格式升级备份守卫——失败
///    暂停写入并报错，原数据不动）；③ 摘要用量计入会话累计。
/// 中途失败不提交条目、不推进边界（原子提交面），错误原样上抛由调用
/// 方发 run:compact_failed 与兜底判定。
/// 压缩生命周期日志（kind=compact 三事件：开始/完成/失败，
/// 「事件点复用」——落点即本函数的三个相位）；失败升 Warn。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_compaction<W, T>(
    store: &Arc<craftwave_engine::storage::SessionStore>,
    events: &Arc<dyn EventSink>,
    session_usage: &Mutex<HashMap<String, UsageTotal>>,
    ledger: &craftwave_engine::usage::UsageLedger,
    session_id: &str,
    owner: &str,
    summary_model: (&str, &str),
    scope: &craftwave_engine::backend::compaction::CompactionScope,
    summary_driver: &TurnDriver<W, T>,
    policy: &CompactionRuntimePolicy,
    session_log: Option<&Arc<craftwave_engine::session_log::SessionLogger>>,
) -> Result<CompactionOutcome, craftwave_engine::backend::compaction::CompactionError>
where
    W: rig::wire::Wire<Op = rig::operation::Completion>,
    T: rig::driver::Transport<W>,
{
    if let Some(logger) = session_log {
        logger.session_line(
            session_id,
            craftwave_engine::providers::types::LogLevel::Info,
            craftwave_engine::session_log::LogKind::Compact,
            "上下文压缩开始".to_owned(),
            Some(serde_json::json!({ "entries": scope.entries.len() })),
            Some(owner),
            None,
            None,
        );
    }
    let result = execute_compaction_inner(
        store,
        events,
        session_usage,
        ledger,
        session_id,
        owner,
        summary_model,
        scope,
        summary_driver,
        policy,
    )
    .await;
    if let Some(logger) = session_log {
        match &result {
            Ok(_) => logger.session_line(
                session_id,
                craftwave_engine::providers::types::LogLevel::Info,
                craftwave_engine::session_log::LogKind::Compact,
                "上下文压缩完成".to_owned(),
                Some(serde_json::json!({
                    "cutEntryId": scope.cut_entry_id,
                    "entries": scope.entries.len(),
                })),
                Some(owner),
                None,
                None,
            ),
            Err(e) => logger.session_line(
                session_id,
                craftwave_engine::providers::types::LogLevel::Warn,
                craftwave_engine::session_log::LogKind::Compact,
                "上下文压缩失败".to_owned(),
                Some(serde_json::json!({ "error": e.to_string() })),
                Some(owner),
                None,
                None,
            ),
        }
    }
    result
}

/// 压缩编排本体（原 [`execute_compaction`] 主体；生命周期行在外层包装）。
#[allow(clippy::too_many_arguments)]
async fn execute_compaction_inner<W, T>(
    store: &Arc<craftwave_engine::storage::SessionStore>,
    events: &Arc<dyn EventSink>,
    session_usage: &Mutex<HashMap<String, UsageTotal>>,
    ledger: &craftwave_engine::usage::UsageLedger,
    session_id: &str,
    owner: &str,
    summary_model: (&str, &str),
    scope: &craftwave_engine::backend::compaction::CompactionScope,
    summary_driver: &TurnDriver<W, T>,
    policy: &CompactionRuntimePolicy,
) -> Result<CompactionOutcome, craftwave_engine::backend::compaction::CompactionError>
where
    W: rig::wire::Wire<Op = rig::operation::Completion>,
    T: rig::driver::Transport<W>,
{
    let (summary, usages) = craftwave_engine::backend::compaction::summarize(
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
        .map_err(|e| craftwave_engine::backend::compaction::CompactionError::Failed(e.to_string()))?;
    let entry = craftwave_engine::storage::StoredCompactionEntry {
        id: craftwave_engine::backend::compaction::compaction_entry_id(owner, count + 1),
        cut_entry_id: scope.cut_entry_id.clone(),
        summary: summary.clone(),
        tokens_before: craftwave_engine::backend::estimator::estimate_entries_tokens(&scope.entries),
        created_at: now_ms(),
    };
    store
        .append_compaction(session_id, entry)
        .map_err(|e| craftwave_engine::backend::compaction::CompactionError::Failed(e.to_string()))?;
    accumulate_compaction_usage(
        session_usage,
        events,
        ledger,
        session_id,
        owner,
        summary_model,
        &usages,
    );
    Ok(CompactionOutcome {
        boundary: craftwave_engine::backend::wire::CompactionBoundary {
            cut_entry_id: scope.cut_entry_id.clone(),
            summary,
        },
    })
}

/// run 开始前的自动压缩编排（ask 关闭前触发点）：
/// 1. 估算本次请求（系统前缀 + 历史边界裁剪 + prompt + 工具表 + 回复预留）；
/// 2. 未达阈值 → Ok((既有边界, tokens))，tokens 供 wire 预算复查；
/// 3. 达阈值 → run:compacting → 压缩编排 → compactDone 帧 + run:compacted；
/// 4. 压缩失败：原上下文能放入窗口（< 0.9）→ Ok((既有边界, …)) 继续；放不下 →
///    Err(E_CONTEXT_OVERFLOW)（输入已落盘保留，不静默裁历史，兜底）。
///
/// 压缩成功后的新边界立即生效于本次 run 的全部请求；
/// `summary_driver` 由调用方按配置解析（专用小模型或当前模型）。
/// `summary_model` = 摘要模型身份 `(配置 id, 显示名)`（落账归属）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn maybe_compact_before_run<W, T>(
    app: &AppState,
    session_id: &str,
    run_id: &str,
    messages: &[MessageEntry],
    tools: &[Arc<rig::completion::request::ToolDefinition>],
    policy: &CompactionRuntimePolicy,
    summary_driver: &TurnDriver<W, T>,
    summary_model: (&str, &str),
    sink: &mut dyn FrameSink,
    custom_prompt: Option<&str>,
) -> Result<(Option<craftwave_engine::backend::wire::CompactionBoundary>, u64), IpcError>
where
    W: rig::wire::Wire<Op = rig::operation::Completion>,
    T: rig::driver::Transport<W>,
{
    let overview = app
        .store
        .compaction_overview(session_id)
        .map_err(|e| IpcError::internal(e.to_string()))?;
    let boundary = overview
        .0
        .map(|c| craftwave_engine::backend::wire::CompactionBoundary {
            cut_entry_id: c.cut_entry_id,
            summary: c.summary,
        });
    // 估算面：活动路径按现有边界裁剪（与 wire 重建同一裁剪口径），
    // 再剔除末条 prompt（本次输入不参与压缩，触发点①口径）
    let whole_after_boundary = boundary_ref_slice(messages, boundary.as_ref())?;
    let body = &whole_after_boundary[..whole_after_boundary.len().saturating_sub(1)];
    let prompt_text = messages
        .last()
        .map(|m| {
            m.parts
                .iter()
                .filter_map(|p| match p {
                    craftwave_engine::storage::WirePart::Text { text } => Some(text.as_str()),
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
    let estimated = craftwave_engine::backend::estimator::estimate_request_tokens(
        &prefixes,
        body,
        &prompt_text,
        &tool_inputs,
        policy.reply_reserve,
    );
    if !craftwave_engine::backend::estimator::should_auto_compact(
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
    let scope = craftwave_engine::backend::compaction::compression_scope(
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
                &app.ledger,
                session_id,
                run_id,
                summary_model,
                &scope,
                summary_driver,
                policy,
                Some(&app.session_log),
            )
            .await
        }
        // 无可压内容但已达阈值（如首次请求即超大附件）：按超预算兜底
        None => Err(craftwave_engine::backend::compaction::CompactionError::Failed(
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
            // 失败兜底：原上下文能放入硬预算（窗口 ×0.9）则继续
            if !craftwave_engine::backend::estimator::is_over_wire_budget(estimated, policy.context_window) {
                return Ok((boundary, estimated));
            }
            return Err(context_overflow(estimated, policy));
        }
    };
    // tokensAfter：新边界下的重算（压缩已在史册生效，重读概览取最新边界）
    let new_boundary = craftwave_engine::backend::wire::CompactionBoundary {
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

/// 活动路径的边界裁剪切片（与 [`craftwave_engine::backend::wire::split_prompt_with_boundary`]
/// 同一裁剪口径的条目视图；边界锚点缺失按参数错误拒绝——不静默全量展开）。
fn boundary_ref_slice<'a>(
    entries: &'a [MessageEntry],
    boundary: Option<&craftwave_engine::backend::wire::CompactionBoundary>,
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

/// ask 关闭后的压缩触发点（run 任务内 wait_approval 收口处，
/// 与落盘封口同一安全点）：**落盘面压缩**——摘要与 compaction 条目落盘后
/// 对恢复后的落盘条目与下一个 run 生效；当前 run 的 AgentRun 状态机
/// （rig 0.45）不支持中途替换内部 history，恢复后照常执行，恢复后请求超
/// 硬预算由请求面预算检查兜底终止（输入保留）。
///
/// 估算基准 = 最近请求真实输入（`DriveState.last_input`）+ 本次审批调用
/// 参数 + 工具表 + 回复预留。失败只发 run:compact_failed、不阻断恢复
/// （落盘面优化不承载硬预算义务——硬预算判定在恢复后的请求面）。
/// 存储器/事件汇/用量表为细粒度参数（spawn 闭包内无 AppState 整体）。
/// `ledger` 与 `summary_model`（摘要模型身份）随闭包传入（落账面）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn maybe_compact_after_ask<W, T>(
    store: &Arc<craftwave_engine::storage::SessionStore>,
    events: &Arc<dyn EventSink>,
    session_usage: &Mutex<HashMap<String, UsageTotal>>,
    ledger: &craftwave_engine::usage::UsageLedger,
    session_id: &str,
    run_id: &str,
    summary_model: (&str, &str),
    pending: &PendingCall,
    state: &DriveState,
    tools: &[Arc<rig::completion::request::ToolDefinition>],
    policy: &CompactionRuntimePolicy,
    summary_driver: &TurnDriver<W, T>,
    session_log: Option<&Arc<craftwave_engine::session_log::SessionLogger>>,
) where
    W: rig::wire::Wire<Op = rig::operation::Completion>,
    T: rig::driver::Transport<W>,
{
    let pending_tokens = craftwave_engine::backend::estimator::estimate_text_tokens(&pending.args_json);
    let tool_tokens = craftwave_engine::backend::estimator::estimate_tool_tokens(&tool_estimate_inputs(tools));
    let estimated = state.last_input + pending_tokens + tool_tokens + policy.reply_reserve;
    if !craftwave_engine::backend::estimator::should_auto_compact(
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
            .map_err(|e| craftwave_engine::backend::compaction::CompactionError::Failed(e.to_string()))?;
        let overview = store
            .compaction_overview(session_id)
            .map_err(|e| craftwave_engine::backend::compaction::CompactionError::Failed(e.to_string()))?;
        let boundary = overview
            .0
            .map(|c| craftwave_engine::backend::wire::CompactionBoundary {
                cut_entry_id: c.cut_entry_id,
                summary: c.summary,
            });
        let scope = craftwave_engine::backend::compaction::compression_scope(
            &tree.messages,
            boundary
                .as_ref()
                .map(|b| (b.cut_entry_id.as_str(), b.summary.as_str())),
        );
        let scope = scope.ok_or_else(|| {
            craftwave_engine::backend::compaction::CompactionError::Failed("无可压缩的历史内容".to_owned())
        })?;
        execute_compaction(
            store,
            events,
            session_usage,
            ledger,
            session_id,
            run_id,
            summary_model,
            &scope,
            summary_driver,
            policy,
            session_log,
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
                    let boundary = craftwave_engine::backend::wire::CompactionBoundary {
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

/// `compact_session` 主体（手动压缩）：对活跃 run 外的
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
            "session already has an active run: {session_id}"
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
            craftwave_engine::storage::StorageError::NotFound(_) => IpcError::new(
                craftwave_engine::util::errors::E_NOT_FOUND,
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
        .map(|c| craftwave_engine::backend::wire::CompactionBoundary {
            cut_entry_id: c.cut_entry_id,
            summary: c.summary,
        });
    // 手动压缩估算面：史册 + 摘要前缀 + 回复预留（工具表不计——无 run 配置，
    // before/after 同口径即可比）
    let prefixes: Vec<&str> = [boundary.as_ref().map(|b| b.summary.as_str())]
        .into_iter()
        .flatten()
        .collect();
    let tokens_before = craftwave_engine::backend::estimator::estimate_request_tokens(
        &prefixes,
        &tree.messages,
        "",
        &[],
        policy.reply_reserve,
    );
    let scope = craftwave_engine::backend::compaction::compression_scope(
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
        &app.ledger,
        session_id,
        "manual",
        (&summary_resolved.id, &summary_resolved.name),
        &scope,
        &summary_driver,
        &policy,
        Some(&app.session_log),
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

/// `compact_session` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CompactSessionArgs {
    pub session_id: String,
}

/// `compact_session` 返回契约：cutEntryId 缺省 = 无可压内容。
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

/// `compact_session` 命令。
#[tauri::command]
pub(crate) async fn compact_session(
    app: State<'_, AppState>,
    runs: State<'_, RunManager>,
    session_id: String,
) -> Result<CompactSessionResult, IpcError> {
    compact_session_core(&app, &runs, &session_id).await
}

/// E_CONTEXT_OVERFLOW 错误构造（暂停并保留输入，提示重试
/// 压缩或开启新会话）。
fn context_overflow(estimated: u64, policy: &CompactionRuntimePolicy) -> IpcError {
    IpcError::new(
        craftwave_engine::util::errors::E_CONTEXT_OVERFLOW,
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
/// config 与测试 sink——不经 tauri 运行时）。`auto_confirm` 为
/// AppConfig.approval.autoConfirm 的入口快照（挂起 300s 超时自动
/// 选 recommended 项；单会话入口读一次）。
///
/// 帧出口：入参 sink（生产 = tauri Channel 适配器，测试 =
/// 收集器）作为 [`FrameHub`] 的初始下发端——seq 递增、环形缓冲（512 帧）
/// 与下发收进枢纽；压缩编排帧与 run 帧共用同一出口，重载后 `attach_run`
/// 可重放（含 compactDone）。
#[allow(clippy::too_many_arguments)]
pub async fn start_chat_core(
    app: &AppState,
    runs: &RunManager,
    session_id: String,
    messages: Vec<MessageEntry>,
    run_config: RunConfig,
    sink: Box<dyn SequencedSink>,
    auto_confirm: bool,
) -> Result<StartChatResult, IpcError> {
    if session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    // ① 会话偏好起点快照：prefs 文件优先于
    //    RunConfig（前端两者同步写，文件缺失/字段未设时回落 RunConfig——
    //    旧前端载荷兼容面）。模型解析与 thinking 初值都按此快照生效。
    let prefs_initial = craftwave_engine::storage::prefs::load_prefs(app.store.root(), &session_id);
    let initial_model_id = prefs_initial.model_id.or(run_config.model_id.clone());
    let initial_thinking = prefs_initial.thinking.or(run_config.thinking);
    // ① 模型解析
    let model = resolve_model(app, initial_model_id.as_deref())?;
    // ①a 图片 blob 装配：收集消息里全部 image part 的哈希，
    //    从会话 imgblob 目录读出（mime 按魔数嗅探恢复）。读取失败跳过该表项
    //    （vision 模型下对应图片降级为占位文本，不拒绝请求——历史图片的
    //    blob 可能已被会话删除流程清走）；日志纪律：只记哈希，不记字节。
    let image_blobs = load_image_blobs(app.store.root(), &session_id, &messages);
    // ①b wire 参数校验（末条 user 为 prompt；image part 按 vision 双
    //    形态校验——非 vision 模型的本次新附图在此拒绝）——压缩编排前
    //    先验，让参数错误优先于任何有副作用的流程
    let vision_policy = craftwave_engine::backend::wire::VisionPolicy {
        vision: model.vision,
        blobs: &image_blobs,
    };
    craftwave_engine::backend::wire::split_prompt_visioned(&messages, None, &vision_policy)
        .map_err(|e| IpcError::invalid_arg(e.to_string()))?;
    // ② 系统级前缀与压缩策略（注入次序：customPrompt → 压缩 summary →
    //    当前计划 → 技能清单）。save_config 即时生效于下次 run（config.load
    //    命中缓存热替换后的值）。
    let config_snapshot = app.config.load()?;
    let custom_prompt = config_snapshot
        .agent
        .as_ref()
        .and_then(|a| a.custom_prompt.clone());
    // ai_language 注入层（次序：customPrompt → 项目指令 →
    // ai_language → 压缩 summary → 技能清单）：模型回复语言偏好，非空用
    // 自定义值、空跟随界面语言
    let ai_language_layer = craftwave_engine::backend::wire::ai_language_layer(
        config_snapshot.ui.ai_language.as_deref(),
        config_snapshot.ui.language.as_str(),
    );
    // 技能快照与渐进披露（run 开始固定版本——重扫/启停只
    // 影响下一 run；未信任技能询问在本步同步完成，拒绝者不进清单）。
    let run_skills = crate::host::commands::knowledge_cmds::ensure_skills_snapshot(app, &session_id).await;
    // 项目指令信任核验与注入：AGENTS.md 等
    // 首次加载弹 projectTrust 询问，deny/未信任不注入（本步同步完成，
    // 与技能信任同一装配时机）。
    let instructions =
        craftwave_engine::knowledge::instructions::ensure_instruction_disclosure(app, &session_id).await;
    let disclosure = run_skills.disclosure.clone();
    let policy = compaction_policy(app, &model);
    // 摘要驱动（专用小模型未配则当前 run 模型——无会话覆盖时即
    // active model；与 run 模型同源解析，独立客户端实例）
    let summary_model_id = app.config.load()?.compaction.model_id.clone();
    let summary_resolved = match summary_model_id.as_deref() {
        Some(id) => resolve_model(app, Some(id))?,
        None => resolve_model(app, initial_model_id.as_deref())?,
    };
    let summary_driver = TurnDriver::new(summary_resolved.driver, RunSpec::new(), Vec::new());
    // ③ 工具集（三处同源过滤）——静态段 + MCP 动态段（
    // 动态段快照随 run 装配固化，tools/list_changed 的热更新下一 run 生效）；
    // 预算检查含工具表
    let mut registry_specs = craftwave_engine::tools::build_registry();
    registry_specs.extend(app.mcp.registry().snapshot());
    let tool_specs = craftwave_engine::tools::filter_for_tier(registry_specs, run_config.tier);
    let tools = to_rig_tool_definitions(tool_specs)?;

    // ④ 同会话互斥预检（E_SESSION_RUNNING 快速失败；正式互斥仍由
    //    RunManager::start 兜底）→ 帧枢纽就位 → run 开始前压缩
    //    （触发点①：首个模型调用前完成摘要与落盘，新边界立即生效于本次 run 全部
    //    请求；compactDone 帧经枢纽入环，重载后 attach 可重放）
    if runs.get(&session_id).is_some() {
        return Err(IpcError::session_running(format!(
            "session already has an active run: {session_id}"
        )));
    }
    let run_id = uuid::Uuid::new_v4().to_string();
    // user 条目幂等兜底提前到压缩流程最前：输入先落盘再谈压缩——
    // 压缩失败/E_CONTEXT_OVERFLOW 暂停时「输入已保留」在史册面成立
    persist_user_fallback(app, &session_id, &messages);
    // 当前生效模型槽：run 装配时置初值，HostRequestRefresher
    // 每拍刷新（模型切换时更新），EventfulSink 的 usage 帧落账读取——
    // run 内切模型按拍真实归属。子代理/压缩各有独立身份面。
    let current_model: Arc<Mutex<Option<(String, String)>>> =
        Arc::new(Mutex::new(Some((model.id.clone(), model.name.clone()))));
    // 会话累计恢复：进程内首次触达该会话时从明细文件灌入内存表——
    // 后续 tokens:update 全量校正事件的快照从首个增量起即完整。
    {
        let mut map = app
            .session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !map.contains_key(&session_id) {
            let restored = app.ledger.detail.load(&session_id).session_total;
            map.insert(session_id.clone(), restored);
        }
    }
    let hub = Arc::new(FrameHub::new(sink));
    let (boundary, estimated_after) = maybe_compact_before_run(
        app,
        &session_id,
        &run_id,
        &messages,
        &tools,
        &policy,
        &summary_driver,
        (&summary_resolved.id, &summary_resolved.name),
        &mut HubSink { hub: hub.clone() },
        custom_prompt.as_deref(),
    )
    .await?;
    // wire 硬预算复查（压缩后仍 > 窗口 ×0.9 → 暂停保留输入，不静默裁历史）
    if craftwave_engine::backend::estimator::is_over_wire_budget(estimated_after, policy.context_window) {
        return Err(context_overflow(estimated_after, &policy));
    }
    // ⑤ wire 切分（压缩边界 + image 双形态）：cutEntryId 前跳过/
    //    后展开，摘要交前缀层；image part 按视觉策略映射（vision 多模态块 /
    //    非 vision 占位）
    let (history, prompt, compaction_summary) =
        craftwave_engine::backend::wire::split_prompt_visioned(&messages, boundary.as_ref(), &vision_policy)
            .map_err(|e| IpcError::invalid_arg(e.to_string()))?;
    // 系统前缀注入层（次序：customPrompt → 项目指令 → 压缩 summary →
    // 技能清单）。项目指令插入第二位（与
    // customPrompt 同类——用户/仓库侧给 AI 的常驻指引）。
    let history = craftwave_engine::backend::wire::with_system_prefix_layers(
        history,
        [
            custom_prompt.as_deref(),
            instructions.as_deref(),
            ai_language_layer.as_deref(),
            compaction_summary.as_deref(),
            disclosure.as_deref(),
        ],
    );

    // ⑥ 注册 run（同会话互斥 E_SESSION_RUNNING；帧枢纽随句柄挂载）+ run:start
    //    事件（手动 start_chat 与自动投递都发——前端
    //    以它区分「自己 adapter 发起的 run」与「后端自动投递的新 run」并接入
    //    观察态续流）
    let handle = runs.start(&session_id, run_id.clone(), now_ms(), hub)?;
    app.events.emit(
        keys::RUN_START,
        serde_json::json!({ "sessionId": session_id, "runId": run_id }),
    );

    // ⑤ spawn RunLoop 任务（帧生产 + 审批挂起恢复 + 取消 + 终态摘除）。
    //    thinking 起点参数：按会话显式档 > per-model
    //    reasoningEffort > 不传的优先级链翻译为 rig 请求参数。
    //    Anthropic 方言同源给出请求级 max_tokens（协议必填 + 预算校验的
    //    output cap，按 thinking 预算上调）；OpenAI 系维持不带 max_tokens。
    let initial_directive = effective_thinking_directive(initial_thinking, &model);
    let mut spec = RunSpec {
        max_turns: Some(run_config.max_turns as usize),
        options: craftwave_engine::backend::turn::reasoning_directive_to_options(&initial_directive),
        ..RunSpec::new()
    };
    if let Some(cap) = craftwave_engine::backend::turn::directive_max_tokens(
        &initial_directive,
        model.max_tokens,
        model.api_format,
    ) {
        spec.max_tokens = Some(cap);
    }
    // 驱动共享句柄：每拍请求刷新源（HostRequestRefresher）持有
    // 同一 Arc 热更新模型客户端与请求参数；suggest 后台小调用也经 Arc 复用
    // 终态时的最新模型。
    let driver = Arc::new(TurnDriver::new(model.driver, spec.clone(), tools.clone()));
    // 模型调用日志钩子：Info 级每次收口记 host/model/耗时/
    // usage 元数据；verbose（Debug 级）追加请求/响应原文——宿主实现内
    // 过图片摘要 + 密钥脱敏闸后落会话日志。
    driver.set_llm_logger(Arc::new(HostLlmLogger {
        session_log: app.session_log.clone(),
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        api_host: HostLlmLogger::host_of(&model.base_url),
        blob_dir: craftwave_engine::storage::imgblob::imgblob_dir(app.store.root(), &session_id),
        current_model: current_model.clone(),
    }));
    // 每拍请求刷新源：运行中切换模型/thinking 在同一 run
    // 的下一次模型请求生效。起点快照 = ①步合并值；当前快照同起点。
    let refresher: Arc<dyn RequestRefresher> = Arc::new(HostRequestRefresher {
        app: app.share_handles(),
        session_id: session_id.clone(),
        driver: driver.clone(),
        initial: EffectiveRuntime {
            model_id: initial_model_id.clone(),
            thinking: initial_thinking,
        },
        current: Mutex::new(EffectiveRuntime {
            model_id: initial_model_id.clone(),
            thinking: initial_thinking,
        }),
        current_model: current_model.clone(),
    });
    let model_name = model.name;
    let cancel = handle.cancel.clone();
    // 会话内记忆启用：命令类审批「始终允许（本会话）」选项
    // 的展示开关；恢复循环按应答登记会话免批桶（句柄提前克隆进 run 任务）。
    // 审批策略快照按 run 读取（save_config 后下一次 start_chat 应用）。
    let approval_config = app
        .config
        .load()
        .map(|c| c.approval.clone())
        .unwrap_or_default();
    let session_memory_enabled = approval_config.session_memory_enabled;
    let approval_memory_for_task = app.approval_memory.clone();
    let session_id_for_memory = session_id.clone();
    let executor: Arc<dyn craftwave_engine::backend::run_loop::CancellableExecutor> = Arc::new(
        ToolsetExecutor::new(
            // 会话 roots（含外部授权目录）：项目会话 = 项目主目录 +
            // 数据目录下 `workspace/`
            // 子目录 + 会话已授权外部目录，临时会话两根同为 workspace 子
            // 目录——数据目录本体（config/sessions 等存储内部文件）退出
            // 模型文件工具可见面；收窄单一落点在 resolve_session_roots，
            // 这里只负责两根首次使用即建（canonicalize 前置）。宿主侧边车
            //（spill/logs）仍挂 AppState.store 根，不经本 roots。
            {
                let roots =
                    craftwave_engine::session_roots::resolve_session_roots(app, &session_id);
                let _ = std::fs::create_dir_all(&roots.project_root);
                let _ = std::fs::create_dir_all(&roots.workspace_root);
                craftwave_engine::tools::ToolsContext::new(roots.project_root, roots.workspace_root)
                    .with_extra_roots(roots.extra_dirs)
            },
            run_config.tier,
        )
        // 同批无冲突调用并发上限（tools 组）与 batch_read
        // 单批文件数上限（tools.batchReadLimit）：按 run 装配时
        // 读取配置生效（save_config 后下一次 start_chat 应用）。
        .with_max_parallel(app.config.load()?.tools.max_parallel as usize)
        .with_batch_read_limit(app.config.load()?.tools.batch_read_limit)
        // read(kind="image") 的 vision 前置校验面：按本次
        // run 解析出的模型标记开启——主动读图与新附图同一拒绝口径
        .with_vision(model.vision)
        .with_env(ExecutorEnv {
            events: app.events.clone(),
            ask: handle.ask.clone(),
            plan: handle.plan.clone(),
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            data_root: app.store.root().to_path_buf(),
            // 问卷 ask 开关日志：经接缝落 kind=approval 行
            ask_log: Some(Arc::new(HostAskLogger {
                session_log: app.session_log.clone(),
            })),
            // 产物登记：主代理环境装配存储器——
            // write/edit 成功落盘后写 <sid>.artifacts.json（子代理环境不装配）
            store: Some(app.store.clone()),
            // MCP 动态工具调用面：归一名反查 + 结果未知语义在
            // mcp/pool 收口。
            mcp: Some(Arc::new(crate::host::commands::mcp_cmds::McpCallerAdapter {
                host: app.mcp.clone(),
            })),
            // run 内技能快照：`skill` 工具只读快照——正文
            // 全文与 description 同版本固定，reload/toggle 不影响活跃 run。
            skills: Some(Arc::new(run_skills.snapshot)),
            // 子代理委派面：主代理环境可委派；白名单不限
            // （allowed_tools=None = 主代理全表）。
            subagents: Some(Arc::new(
                crate::host::commands::subagent::SubagentSpawnerImpl {
                    app: app.share_handles(),
                    ctx: crate::host::commands::subagent::SubagentContext {
                        session_id: session_id.clone(),
                        parent_run_id: run_id.clone(),
                        parent_hub: handle.hub.clone(),
                        tier: run_config.tier,
                        model_id: run_config.model_id.clone(),
                        auto_confirm,
                        // 子代理命令类审批同主 run 展示
                        // 「始终允许（本会话）」，免批登记共用会话桶
                        session_memory_enabled,
                    },
                },
            )),
            allowed_tools: None,
            // 网络工具执行环境（web_fetch/http_request）：SSRF 策略
            // （network.allowPrivateNetwork）+ 代理 + 进程级节流器，按当前
            // AppConfig 装配（save_config 后下一次 start_chat 生效）。
            network: Some(Arc::new(craftwave_engine::tools::NetworkEnv::from_config(
                app.config.load()?.as_ref(),
            ))),
        }),
    );
    // 审批门装配 MCP 判定视图（动态许可核验 + 未声明
    // read_only 即便 full_access 也挂起）。围栏 AST 判定的工作区
    // 上下文（四象限）：roots 与项目主目录做 canonicalize
    // 后装配（词法与 canonical 同域比较的前提）；任一根 canonicalize
    // 失败则不装配——围栏退化为「有写目标即 Confirm」的保守面。
    // 装配面：内部状态禁区判定（数据目录根 + 项目主
    // 目录两根）、会话级免批桶（always_session，主 run 与子代理同桶）、
    // 审批策略快照（四开关 ①④ + 白名单条目，按 run 读取生效）。
    let gate = {
        let roots = craftwave_engine::session_roots::resolve_session_roots(app, &session_id);
        let canonical = |p: &std::path::Path| std::fs::canonicalize(p).ok();
        let gate_ws = match (
            canonical(&roots.project_root),
            canonical(&roots.workspace_root),
        ) {
            (Some(project), Some(data)) => Some(craftwave_engine::backend::gate::GateWorkspace {
                roots: {
                    // 全根 canonical（含授权目录——授权判定与围栏判定同域
                    // 比较）
                    let mut all = vec![project.clone(), data];
                    for extra in &roots.extra_dirs {
                        if let Some(c) = canonical(extra) {
                            all.push(c);
                        }
                    }
                    all
                },
                base: project.clone(),
            }),
            _ => None,
        };
        let approval = approval_config.clone();
        let mut gate = ApprovalGate::new(run_config.tier, handle.plan.clone())
            .with_mcp(Arc::new(crate::host::commands::mcp_cmds::McpGateAdapter {
                host: app.mcp.clone(),
            }))
            .with_internal_guard(craftwave_engine::safety::internal_state::InternalStateGuard::new(
                // 内部状态禁区判定：数据目录根 + 项目主目录两根
                // ——注意第二根是 workspace 子目录，禁区
                // 判定仍以数据目录根为界（config/sessions 等恒在禁区）。
                app.store.root(),
                &roots.project_root,
            ))
            // 外部目录授权：初始集合 = 会话已授权目录，
            // 恢复循环在用户允许后热追加；数据目录根做禁授权目标判定。
            .with_external_dirs(roots.extra_dirs.clone(), app.store.root().to_path_buf())
            .with_session_memory(app.approval_memory.bucket(&session_id))
            // 计划基线扩展桶（范围门 always_session）：主 run
            // 装配会话桶——「本会话始终允许」的授权目录跨 run 生效（进程内
            // 不持久化）。
            .with_plan_scope_memory(app.plan_scopes.bucket(&session_id))
            .with_policy(craftwave_engine::backend::gate::ApprovalPolicy {
                enabled: approval.enabled,
                allowlist_enabled: approval.allowlist_enabled,
                command_allowlist: approval.command_allowlist.clone(),
                require_plan: approval.require_plan,
                git_allow_stage_commit: approval.git_allow_stage_commit,
            });
        if let Some(ws) = gate_ws {
            gate = gate.with_workspace(ws);
        }
        Arc::new(gate)
    };
    let events = app.events.clone();
    let session_usage = Arc::new(Mutex::new(
        app.session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    ));
    // 自动起名：user 消息落盘后判定——index 标题仍为
    // 默认 && 树内仅此一条 user 消息（改名/非首条/重启续聊三不触发，重启
    // 不重生成由判定天然保证）。命中 spawn 后台标题任务（零工具小调用，
    // 独立于 run 生命周期；失败静默）。放在 run 副本表创建
    // 之后 spawn——标题与 run/suggest 共用同一份会话累计快照，tokens:update
    // 全量校正在同一累计域内收敛（跨表双源会造成事件快照互相覆盖）。
    {
        let meta_title = app.store.index().get(&session_id).map(|m| m.title);
        let user_count = app
            .store
            .load_session_tree(&session_id)
            .map(|tree| {
                tree.messages
                    .iter()
                    .filter(|m| m.role == MessageRole::User)
                    .count()
            })
            .unwrap_or(0);
        let first_text = first_user_text(&messages);
        if title_should_trigger(meta_title.as_deref(), user_count) && !first_text.trim().is_empty()
        {
            // 模型 = 压缩摘要专用模型配置共用（已落 `compaction.
            // modelId` 则复用）；未配/解析失败回退会话
            // 激活模型（上面 ① 已解析成功的同源，起名不因摘要配置漂移跳过）。
            let title_resolved = app
                .config
                .load()
                .ok()
                .and_then(|c| c.compaction.model_id.clone())
                .and_then(|id| resolve_model(app, Some(&id)).ok())
                .or_else(|| resolve_model(app, initial_model_id.as_deref()).ok());
            if let Some(resolved) = title_resolved {
                spawn_title_task(
                    Arc::new(TurnDriver::new(resolved.driver, RunSpec::new(), Vec::new())),
                    app.store.clone(),
                    app.events.clone(),
                    session_usage.clone(),
                    app.ledger.clone(),
                    session_id.clone(),
                    run_id.clone(),
                    first_text,
                    (resolved.id, resolved.name),
                    TITLE_TIMEOUT,
                );
            }
        }
    }
    let runs_for_task = runs.clone();
    let suggest_tail = context_tail(&messages);
    // 审计的用户目标：本次 run 的首条 user 输入文本（first_user_text），
    // 随 run 任务进闭包——审计提示词只带本 run 输入摘要，不带
    // 整段聊天历史。
    let audit_goal = first_user_text(&messages);
    // 模型停滞看门狗超时：按 run 装配驱动时
    // 读取生效（save_config 后下一次 start_chat 应用）。
    let stall_timeout =
        std::time::Duration::from_secs(u64::from(app.config.load()?.watchdog.stall_timeout_secs));
    let run_id_for_result = run_id.clone();
    // 项目主目录（范围门恢复循环）：always_session 授权时把
    // 计划外写目标的词法路径展开为绝对路径再归一（与范围门判定同款基准）。
    let project_root_for_scope =
        craftwave_engine::session_roots::resolve_session_roots(app, &session_id).project_root;
    // steer 插话源：RunHandle 的插话队列 + 宿主副作用
    // （canonical 落盘 + run:inject 回执）收进 HostSteerSource，RunLoop 于
    // turn 边界经 trait 取文本
    let steer_source: Arc<dyn SteerSource> = Arc::new(HostSteerSource {
        queue: handle.steer.clone(),
        store: app.store.clone(),
        events: events.clone(),
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        seq: std::sync::atomic::AtomicU32::new(0),
    });
    // ask 关闭后压缩触发点（触发点②）的依赖面：摘要驱动与策略随
    // run 任务进闭包（摘要模型独立解析，与 run 的 driver 互不占用）；
    // 存储器同源 Arc（命令层与 run 落盘器同一锁表）
    let summary_driver_for_task = summary_driver;
    let policy_for_task = policy;
    let store_for_task = app.store.clone();
    // run 落盘器（落盘失败记账挂 app 通道）
    let recorder = RunRecorder::new(
        app.store.clone(),
        session_id.clone(),
        run_id.clone(),
        Some(app.session_log.clone()),
    );
    // run 收尾的队列依赖面：全部字段同源的共享句柄副本（Arc
    // 克隆零深拷贝）——收尾后按终态分流 followUp 队列并自动投递（投递要
    // 以完整状态面重新走 start_chat_core，故捕获句柄而非细粒度克隆件）
    let app_for_delivery = app.share_handles();
    let run_config_for_delivery = run_config.clone();
    // MCP 宿主句柄：恢复循环的「始终允许」许可授予面（spawn
    // 任务不能借用 app，提前克隆共享句柄）。
    let mcp_for_task = app.mcp.clone();
    // 用量账本克隆件：帧面落账与压缩辅助落账各自一份（clone 即
    // 同源句柄）；run 收尾的明细收口（endedAt）再用一份；suggest 的终态
    // 模型快照在收尾处从槽读取。
    let ledger_for_sink = app.ledger.clone();
    let ledger_for_compaction = app.ledger.clone();
    let ledger_for_finalize = app.ledger.clone();
    let current_model_for_sink = current_model.clone();
    let model_slot_for_suggest = current_model.clone();
    // 会话日志依赖面：run 开始/收尾行与工具起止行共用的克隆件
    let session_log_for_sink = app.session_log.clone();
    let session_log_for_end = app.session_log.clone();
    let session_log_for_ask = app.session_log.clone();
    let model_name_for_log = model_name.clone();
    // 审计的依赖面：审计模型未配时跟随会话当前模型（槽快照
    // id）+ 用户目标 + 用量账本/会话累计共享句柄。
    let model_slot_for_audit = current_model.clone();
    let audit_goal_for_task = audit_goal;
    tokio::spawn(async move {
        // 帧出口 = 事件层（usage → tokens:update + RunState 同步 + 统计落账）
        // 包 canonical 落盘器包帧枢纽（seq 递增 → 入环 → 下发）
        let mut sink = EventfulSink {
            inner: Box::new(PersistingSink::new(
                recorder,
                Box::new(HubSink {
                    hub: handle.hub.clone(),
                }),
            )),
            events: events.clone(),
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            last_run_total: UsageTotal::default(),
            session_usage: session_usage.clone(),
            handle: Some(handle.clone()),
            ledger: ledger_for_sink,
            current_model: current_model_for_sink,
            run_counted: false,
            // 会话日志器：工具起止行落点
            session_log: Some(session_log_for_sink.clone()),
            tool_starts: HashMap::new(),
        };
        // run 开始行（kind=run：runId/模型）
        session_log_for_sink.session_line(
            &session_id,
            craftwave_engine::providers::types::LogLevel::Info,
            craftwave_engine::session_log::LogKind::Run,
            "run 开始".to_owned(),
            Some(serde_json::json!({ "model": model_name_for_log })),
            Some(&run_id),
            None,
            None,
        );
        let mut run = AgentRun::from_spec(&spec, (*prompt).clone(), Some(history));
        // 计划提醒：根会话 run 启用变更计数与
        // 注入（子代理 run 的 DriveState 缺省关闭——"根会话"口径）；初始
        // 状态经 drive_run 的 resume 链传递，跨审批挂起延续。
        let mut resume_state: Option<DriveState> = Some(DriveState {
            plan_reminder_enabled: true,
            ..DriveState::default()
        });
        // 错误终态信息（带 code：看门狗终止 E_RUN_STALLED，其余
        // E_INTERNAL 兜底）：loop 内记录、收尾随 run:error 事件发出
        let mut errored: Option<craftwave_engine::followup::RunFailure> = None;
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
                Some(steer_source.as_ref()),
                Some(refresher.as_ref()),
                stall_timeout,
            )
            .await;
            match attempt {
                Err(err) => {
                    let message = err.to_string();
                    errored = Some(craftwave_engine::followup::RunFailure {
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
                    errored = Some(craftwave_engine::followup::RunFailure {
                        code: code.clone(),
                        message: message.clone(),
                    });
                    break RunExit::Failed { code, message };
                }
                Ok(RunExit::Suspended(info)) => {
                    let craftwave_engine::backend::run_loop::SuspendedInfo {
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
                    let mut audit_note: Option<craftwave_engine::backend::ask::AuditNote> = None;
                    if pending.name == "command"
                        && gate.audit_class(&pending.name, &pending.args_json)
                    {
                        let frame_call_id = state
                            .model_call_ids
                            .get(&pending.model_call_id)
                            .cloned()
                            .unwrap_or_else(|| pending.model_call_id.clone());
                        sink.send(ChatFrame::ToolAudit {
                            call_id: frame_call_id.clone(),
                            state: craftwave_engine::backend::frames::AuditState::Running,
                            reason: None,
                        });
                        let plan_baseline: Vec<String> = handle
                            .plan
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .as_ref()
                            .map(|p| p.baseline_paths.clone())
                            .unwrap_or_default();
                        let session_model_id = model_slot_for_audit
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .clone()
                            .map(|(id, _)| id);
                        let flow = run_command_audit(
                            &app_for_delivery,
                            &session_id,
                            &pending,
                            session_model_id.as_deref(),
                            &audit_goal_for_task,
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
                                        &ledger_for_compaction,
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
                                    craftwave_engine::backend::audit::AuditDecision::Allow => {
                                        sink.send(ChatFrame::ToolAudit {
                                            call_id: frame_call_id,
                                            state: craftwave_engine::backend::frames::AuditState::Allow,
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
                                                errored = Some(craftwave_engine::followup::RunFailure {
                                                    code: E_INTERNAL.to_owned(),
                                                    message: message.clone(),
                                                });
                                                send_error_ended(&mut sink, message);
                                                break RunExit::Cancelled;
                                            }
                                        }
                                    }
                                    craftwave_engine::backend::audit::AuditDecision::Block => {
                                        sink.send(ChatFrame::ToolAudit {
                                            call_id: frame_call_id.clone(),
                                            state: craftwave_engine::backend::frames::AuditState::Block,
                                            reason: Some(reason.clone()),
                                        });
                                        audit_block_finish(
                                            &mut sink,
                                            &frame_call_id,
                                            &state,
                                            &reason,
                                        );
                                        errored = Some(craftwave_engine::followup::RunFailure {
                                            code: craftwave_engine::util::errors::E_AUDIT_BLOCKED.to_owned(),
                                            message: format!(
                                                "命令审计模型判断高风险，已拒绝执行：{reason}"
                                            ),
                                        });
                                        break RunExit::Failed {
                                            code: craftwave_engine::util::errors::E_AUDIT_BLOCKED.to_owned(),
                                            message: format!(
                                                "命令审计模型判断高风险，已拒绝执行：{reason}"
                                            ),
                                        };
                                    }
                                    craftwave_engine::backend::audit::AuditDecision::Ask => {
                                        sink.send(ChatFrame::ToolAudit {
                                            call_id: frame_call_id,
                                            state: craftwave_engine::backend::frames::AuditState::Ask,
                                            reason: Some(reason.clone()),
                                        });
                                        audit_note = Some(craftwave_engine::backend::ask::AuditNote {
                                            state: craftwave_engine::backend::ask::AuditNoteState::Ask,
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
                                        &ledger_for_compaction,
                                        &session_id,
                                        &format!("{run_id}:audit"),
                                        &attempt.model,
                                        &delta,
                                    );
                                }
                                sink.send(ChatFrame::ToolAudit {
                                    call_id: frame_call_id,
                                    state: craftwave_engine::backend::frames::AuditState::Manual,
                                    reason: Some(reason.clone()),
                                });
                                audit_note = Some(craftwave_engine::backend::ask::AuditNote {
                                    state: craftwave_engine::backend::ask::AuditNoteState::Manual,
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
                        Some(&session_log_for_ask),
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
                                let _ =
                                    allow_external_dir_core(&app_for_delivery, &session_id, dir);
                                // 内存面授权用最长存在前缀归一（safe_join 同款
                                // 实现）——目标目录不存在时持久面被拒绝
                                // （canonicalize 失败），内存面仍生效让本 run
                                // 继续；否则重放判定不命中会再次挂起成死循环。
                                if let Ok(canonical) =
                                    craftwave_engine::safety::safe_join::canonicalize_best_effort(
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
                                    craftwave_engine::safety::safe_join::canonicalize_best_effort(&lexical)
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
                                approval_memory_for_task.remember(
                                    &session_id_for_memory,
                                    &pending.name,
                                    &pending.args_json,
                                );
                            }
                            // MCP「始终允许」：写工具级许可表
                            // ——绑定服务器启动指纹 + 原始工具名 + 已确认输入
                            // schema 哈希三元组；新增工具/schema 变化不继承。
                            if option_id == "always_allow"
                                && let Some(identity) =
                                    mcp_for_task.registry().identity(&pending.name)
                            {
                                let _ = mcp_for_task.licenses.grant(
                                    &identity.server_fingerprint,
                                    &identity.original_name,
                                    &identity.schema_hash,
                                    now_ms(),
                                );
                            }
                            // ask 关闭后压缩（触发点②）：落盘面压缩，
                            // 失败不阻断恢复（硬预算义务在恢复后的请求面）
                            maybe_compact_after_ask(
                                &store_for_task,
                                &events,
                                &session_usage,
                                &ledger_for_compaction,
                                &session_id,
                                &run_id,
                                (&summary_resolved.id, &summary_resolved.name),
                                &pending,
                                &state,
                                &tools,
                                &policy_for_task,
                                &summary_driver_for_task,
                                Some(&session_log_for_ask),
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
                                    errored = Some(craftwave_engine::followup::RunFailure {
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
                    errored = Some(craftwave_engine::followup::RunFailure {
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
            let final_model = model_slot_for_suggest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .unwrap_or_default();
            spawn_suggest(
                driver,
                events.clone(),
                session_usage.clone(),
                ledger_for_compaction.clone(),
                session_id.clone(),
                run_id.clone(),
                suggest_tail,
                final_model,
            );
        }

        // run 收口（明细面）：补写 endedAt（终态只收口，不搬移已记日期）
        let _ = ledger_for_finalize
            .detail
            .mark_run_ended(&session_id, &run_id);

        // run 终态事件：前端 Tab 指示与系统通知门
        // （TauriEventSink 旁路）共用这三个键；payload 口径照文档。失败终态
        // 的 code 从 craftwave_engine::followup::RunFailure 带出（看门狗终止 = E_RUN_STALLED）。
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
            session_log_for_end.app_line(
                craftwave_engine::providers::types::LogLevel::Warn,
                format!("run 失败：{run_id}"),
                Some(serde_json::json!({ "runId": run_id, "code": error_code })),
            );
        }
        session_log_for_end.session_line(
            &session_id,
            if reason == "error" {
                craftwave_engine::providers::types::LogLevel::Warn
            } else {
                craftwave_engine::providers::types::LogLevel::Info
            },
            craftwave_engine::session_log::LogKind::Run,
            match reason {
                "done" => "run 完成".to_owned(),
                "error" => "run 失败".to_owned(),
                _ => "run 已取消".to_owned(),
            },
            Some(serde_json::json!({
                "model": model_name_for_log,
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
        runs_for_task.remove(&session_id, &run_id);

        // —— followUp 队列收尾分流（抽纯函数）——
        // 次序锚点：投递时机 = run 正常完成且终态处理完（runEnded 帧已发出 +
        // RunManager.remove 已摘除）。steer 未消费残余按终态分流：
        // Done → 转会话 followUp 队列尾（随下次投递发出）；取消/失败 →
        // 随 run:queue.returned 交还前端草稿（未消费的插话未落 canonical、
        // 未上时间线，文本交还即无丢失）。
        let leftover_steer = drain_steer(&handle);
        let followups = &app_for_delivery.followups;
        let events_for_task = &events;
        match craftwave_engine::followup::followup_disposition(&final_exit, &errored) {
            craftwave_engine::followup::QueueDisposition::AutoDeliver => {
                for text in leftover_steer {
                    let entry = followup_entry_from_text(&text, &run_config_for_delivery);
                    followups.push(&session_id, entry);
                }
                let snapshot = followups.snapshot(&session_id);
                if snapshot.size > 0 && !snapshot.paused {
                    emit_queue_state(events_for_task, &session_id, followups, Vec::new());
                    spawn_followup_delivery(app_for_delivery, session_id.clone());
                } else {
                    emit_queue_state(events_for_task, &session_id, followups, Vec::new());
                }
            }
            // 失败终态（含看门狗终止 E_RUN_STALLED 与 runEnded(error) 兜底）：
            // 暂停队列 + 持久保留全部待发，steer 残余交还
            craftwave_engine::followup::QueueDisposition::PauseKeep => {
                followups.set_paused(&session_id, true);
                emit_queue_state(events_for_task, &session_id, followups, leftover_steer);
            }
            // 取消终态（含审批 deny 收场）：清空队列 + 全部文本交还草稿
            // （返还草稿；暂停态随清空一并复位）
            craftwave_engine::followup::QueueDisposition::ClearReturn => {
                let mut returned = followups.clear(&session_id);
                returned.extend(leftover_steer);
                emit_queue_state(events_for_task, &session_id, followups, returned);
            }
        }
    });

    Ok(StartChatResult {
        run_id: run_id_for_result,
    })
}

/// start_chat 命令。
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
    let sink = Box::new(ChannelSink { channel });
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

/// cancel_run 主体（置位即返回；公开给集成测试）。
pub async fn cancel_run_core(runs: &RunManager, session_id: &str) -> bool {
    runs.cancel(session_id)
}

/// cancel_run 命令：CancellationToken 置位即返回。
#[tauri::command]
pub(crate) async fn cancel_run(
    runs: State<'_, RunManager>,
    session_id: String,
) -> Result<bool, IpcError> {
    Ok(runs.cancel(&session_id))
}

// ---- steer / followUp 队列 ----

/// 等待会话空闲的重试参数：runEnded 与 remove 之间有并发 start_chat 撞车时
/// 队列头回队延迟重试（「投递循环不丢消息」）；总窗口约 5s，超时保留
/// 队列等下一次触发（下次 run 完成 / resume 命令），不丢消息。
const DELIVERY_RETRY_LIMIT: u32 = 50;
const DELIVERY_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// 从纯文本构造 followUp 待发条目（runConfig 快照随行——投递时按它开新 run；
/// 附件引用缺省为空，带图入队由 inject_run_message_core 补挂）。
fn followup_entry_from_text(text: &str, run_config: &RunConfig) -> FollowUpEntry {
    FollowUpEntry {
        id: format!("fu-{}", uuid::Uuid::new_v4()),
        text: text.to_owned(),
        created_at: now_ms(),
        run_config: QueuedRunConfig {
            model_id: run_config.model_id.clone(),
            thinking: run_config.thinking,
            tier: run_config.tier,
            max_turns: run_config.max_turns,
        },
        attachments: Vec::new(),
    }
}

/// 自动投递队列头：等会话空闲（复查无活跃 run）→ 出队 →
/// 以附着 runConfig 构造 user 消息自动 start_chat 新 run（走同一入口，压缩
/// 编排/wire 切分天然继承；user 条目由 start_chat 侧 persist_user_fallback
/// 兜底落盘，双写协议补落路径）。
///
/// 撞 E_SESSION_RUNNING 回队重试；其他失败（模型未配等命令面错误）回队 +
/// 暂停等待用户明确继续（`resume_followup_queue`）——两种路径都不丢消息。
/// `app` 为全部字段同源的共享句柄副本（[`AppState::share_handles`]）。
async fn deliver_followup_head(
    app: &AppState,
    session_id: &str,
) -> Result<Option<String>, IpcError> {
    // ① 等会话空闲：runEnded 与 remove 之间并发的 start_chat 会占住会话
    for _ in 0..DELIVERY_RETRY_LIMIT {
        if app.runs.get(session_id).is_none() {
            break;
        }
        tokio::time::sleep(DELIVERY_RETRY_INTERVAL).await;
    }
    if app.runs.get(session_id).is_some() {
        // 空闲等待超时：保留队列（不投递），等下次 run 完成 / resume 触发
        return Ok(None);
    }
    // ② 出队前再查暂停态（resume 前的收尾投递竞态防御）
    if app.followups.snapshot(session_id).paused {
        return Ok(None);
    }
    let Some(entry) = app.followups.pop_head(session_id) else {
        return Ok(None);
    };
    app.events.emit(
        keys::RUN_QUEUE,
        serde_json::json!({
            "sessionId": session_id,
            "size": app.followups.snapshot(session_id).size,
            "paused": app.followups.snapshot(session_id).paused,
        }),
    );
    // ③ 构造 user 消息与 RunConfig（附着快照原样生效）。带图待发：
    //    附件引用展开为 image part（blob 已在 attach_image 落盘；
    //    wire 重建按投递时的模型视觉能力决定多模态块/占位文本）。
    let mut parts = vec![craftwave_engine::storage::WirePart::Text {
        text: entry.text.clone(),
    }];
    for attachment in &entry.attachments {
        parts.push(craftwave_engine::storage::WirePart::Image {
            blob_hash: attachment.blob_hash.clone(),
            mime_type: attachment.mime_type.clone(),
        });
    }
    let user_entry = MessageEntry {
        id: format!("auto-{}", entry.id),
        parent_id: None,
        role: MessageRole::User,
        parts,
        created_at: now_ms(),
        run_id: None,
        status: None,
    };
    let run_config = RunConfig {
        model_id: entry.run_config.model_id.clone(),
        thinking: entry.run_config.thinking,
        tier: entry.run_config.tier,
        max_turns: entry.run_config.max_turns,
        steer: false,
    };
    // ④ 自动开新 run：帧出口用 DetachedSink（无前端通道，帧只入环——前端
    //    经 run:start 事件走 attach 观察态续流）
    let auto_confirm = app
        .config
        .load()
        .map(|c| c.approval.auto_confirm)
        .unwrap_or(false);
    match start_chat_core(
        app,
        &app.runs,
        session_id.to_owned(),
        vec![user_entry],
        run_config,
        Box::new(DetachedSink),
        auto_confirm,
    )
    .await
    {
        Ok(result) => Ok(Some(result.run_id)),
        Err(err) => {
            // 投递失败：队列头回队（不丢消息）。E_SESSION_RUNNING = 撞车，
            // 保留未暂停态由重试机制自然续投；其他错误（模型未配等）暂停
            // 队列等待用户明确继续（避免反复失败刷屏）
            app.followups.push_front(session_id, entry);
            if err.code != craftwave_engine::util::errors::E_SESSION_RUNNING {
                app.followups.set_paused(session_id, true);
            }
            emit_queue_state(&app.events, session_id, &app.followups, Vec::new());
            Err(err)
        }
    }
}

/// 自动投递的异步触发（run 正常完成收尾用）：spawn 独立任务执行
/// [`deliver_followup_head`]，失败只记日志（队列态已回队/暂停，事件面可见）。
fn spawn_followup_delivery(app: AppState, session_id: String) {
    tokio::spawn(async move {
        if let Err(e) = deliver_followup_head(&app, &session_id).await {
            #[cfg(debug_assertions)]
            eprintln!(
                "[craftwave] followUp 自动投递失败（{session_id}）：{}",
                e.message
            );
            #[cfg(not(debug_assertions))]
            let _ = (e, session_id);
        }
    });
}

/// `inject_run_message` 主体（含 attachments 处理）：
/// 按 mode 与活跃状态分流——mode=steer（缺省）且会话有活跃 run → 入该 run
/// 的插话队列（turn 边界消费）；否则入会话级 followUp 队列
/// （`mode=followUp` 显式排队时无论活跃与否都入队）。**带图例外**：
/// attachments 非空时不入 steer（插话队列只承载文本），一律转 followUp
/// 队列（持久化 + 自动投递，投递时展开为 image part）。公开给集成测试。
pub fn inject_run_message_core(
    app: &AppState,
    session_id: &str,
    text: &str,
    mode: Option<InjectMode>,
    run_config: Option<&RunConfig>,
    attachments: &[QueuedImageAttachment],
) -> Result<InjectRunMessageResult, IpcError> {
    if session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    if text.trim().is_empty() {
        return Err(IpcError::invalid_arg("text 不能为空"));
    }
    let handle = app.runs.get(session_id);
    // steer：运行中插话（RunHandle 插话队列，RunLoop turn 边界消费）。
    // 带图例外：插话队列只承载文本，attachments 非空时跳过 steer
    // 落入下方 followUp 路径（运行中也会进队列——run 正常完成后自动投递）。
    if attachments.is_empty()
        && mode != Some(InjectMode::FollowUp)
        && let Some(handle) = handle
    {
        handle
            .steer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(text.to_owned());
        return Ok(InjectRunMessageResult {
            queued: true,
            mode: Some(InjectMode::Steer),
            entry_id: None,
        });
    }
    // followUp：会话级待发队列（持久化 + run 正常完成后自动投递）。
    // 附着 runConfig 快照：显式入参优先（前端发送时读会话权限档/模型覆盖），
    // 缺省回退默认档位组合。
    let effective = run_config.cloned().unwrap_or_else(|| RunConfig {
        model_id: None,
        thinking: None,
        tier: PermissionTier::AutoEdit,
        max_turns: default_max_turns(),
        steer: false,
    });
    let mut entry = followup_entry_from_text(text, &effective);
    entry.attachments = attachments.to_vec();
    let entry_id = entry.id.clone();
    app.followups.push(session_id, entry);
    emit_queue_state(&app.events, session_id, &app.followups, Vec::new());
    Ok(InjectRunMessageResult {
        queued: true,
        mode: Some(InjectMode::FollowUp),
        entry_id: Some(entry_id),
    })
}

/// `inject_run_message` 命令：平铺形参（tauri invoke 一层；
/// InjectRunMessageArgs 为文档命名锚点）。
#[tauri::command]
pub(crate) async fn inject_run_message(
    app: State<'_, AppState>,
    session_id: String,
    text: String,
    mode: Option<InjectMode>,
    run_config: Option<RunConfig>,
    attachments: Option<Vec<QueuedImageAttachment>>,
) -> Result<InjectRunMessageResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = InjectRunMessageArgs {
        session_id,
        text,
        mode,
        run_config,
        attachments,
    };
    inject_run_message_core(
        &app,
        &args.session_id,
        &args.text,
        args.mode,
        args.run_config.as_ref(),
        args.attachments.as_deref().unwrap_or(&[]),
    )
}

/// `resume_followup_queue` 主体：解除失败暂停；解除后无活跃
/// run 且队列非空 → 同步投递队列头新开 run（runId 带值）。公开给集成测试。
pub async fn resume_followup_queue_core(
    app: &AppState,
    session_id: &str,
) -> Result<ResumeFollowupQueueResult, IpcError> {
    if session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    app.followups.set_paused(session_id, false);
    emit_queue_state(&app.events, session_id, &app.followups, Vec::new());
    if app.runs.get(session_id).is_some() {
        // 有活跃 run：只解除暂停，该 run 正常完成后自动投递
        return Ok(ResumeFollowupQueueResult {
            run_id: None,
            delivered: false,
        });
    }
    if app.followups.snapshot(session_id).size == 0 {
        return Ok(ResumeFollowupQueueResult {
            run_id: None,
            delivered: false,
        });
    }
    match deliver_followup_head(app, session_id).await {
        Ok(Some(run_id)) => Ok(ResumeFollowupQueueResult {
            run_id: Some(run_id),
            delivered: true,
        }),
        // 投递未发生（竞态暂停/队列空）或失败（已回队/暂停并广播）：不算命令错误
        Ok(None) => Ok(ResumeFollowupQueueResult {
            run_id: None,
            delivered: false,
        }),
        Err(_) => Ok(ResumeFollowupQueueResult {
            run_id: None,
            delivered: false,
        }),
    }
}

/// `resume_followup_queue` 命令。
#[tauri::command]
pub(crate) async fn resume_followup_queue(
    app: State<'_, AppState>,
    session_id: String,
) -> Result<ResumeFollowupQueueResult, IpcError> {
    resume_followup_queue_core(&app, &session_id).await
}

/// `remove_followup` 主体：移除一条待发消息（队列条 × 删除与
/// 前端「编辑取出」共用）。公开给集成测试。
pub fn remove_followup_core(
    app: &AppState,
    session_id: &str,
    entry_id: &str,
) -> Result<RemoveFollowupResult, IpcError> {
    if session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    let removed = app
        .followups
        .remove(session_id, entry_id)
        .ok_or_else(|| IpcError::not_found(format!("待发消息不在队列：{entry_id}")))?;
    let _ = removed;
    emit_queue_state(&app.events, session_id, &app.followups, Vec::new());
    Ok(RemoveFollowupResult {
        size: app.followups.snapshot(session_id).size,
    })
}

/// `remove_followup` 命令：平铺形参（同 start_chat 惯例）。
#[tauri::command]
pub(crate) async fn remove_followup(
    app: State<'_, AppState>,
    session_id: String,
    entry_id: String,
) -> Result<RemoveFollowupResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = RemoveFollowupArgs {
        session_id,
        entry_id,
    };
    remove_followup_core(&app, &args.session_id, &args.entry_id)
}

/// resolve_ask 主体（投递应答并唤醒等待方；槽位清理与
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

/// resolve_ask 命令（run 槽未命中时落应用级询问槽
/// （MCP TOFU / 技能 projectTrust 同槽），槽表 `AppState.app_asks`
/// ——信任询问不属任何 run；中间再查子代理注册表——子代理
/// 审批挂起的 ask 挂 SubHandle.ask）。
#[tauri::command]
pub(crate) async fn resolve_ask(
    app: State<'_, AppState>,
    runs: State<'_, RunManager>,
    ask_id: String,
    answer: AskAnswer,
) -> Result<(), IpcError> {
    match resolve_ask_core(&runs, &ask_id, answer.clone()).await {
        Ok(()) => Ok(()),
        Err(err) if err.code == craftwave_engine::util::errors::E_NOT_FOUND => {
            // 子代理槽：活跃子代理的审批挂起
            if let Some(sub) = app.subs.find_by_ask(&ask_id) {
                let tx = sub
                    .ask
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    .map(|slot| slot.tx);
                if let Some(tx) = tx
                    && tx.send(answer.clone()).is_ok()
                {
                    return Ok(());
                }
            }
            if app.resolve_app_ask(&ask_id, answer) {
                Ok(())
            } else {
                Err(err)
            }
        }
        Err(err) => Err(err),
    }
}

/// 范围卡基线对照摘要：已批准冻结基线的人话清单；未批准
/// 或基线为空返回 None（范围卡不带对照段——基线空时文案由载荷侧说明首写
/// 须确认）。
fn plan_scope_note_of(handle: &RunHandle) -> Option<String> {
    let plan = handle
        .plan
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let state = plan.as_ref()?;
    if !state.is_approved() || state.baseline_paths.is_empty() {
        return None;
    }
    Some(state.baseline_paths.join("、"))
}

/// approve_plan 主体：冻结当前计划为批准快照与写范围基线。
/// 冻结后同键补发一次 `plan:update` 全量计划态——
/// 前端计划卡/Inspector 展示基线标记与待批准范围的数据面。
pub fn approve_plan_core(
    runs: &RunManager,
    session_id: &str,
    events: &Arc<dyn EventSink>,
) -> Result<(), IpcError> {
    let Some(handle) = runs.get(session_id) else {
        return Err(IpcError::not_found(format!("会话无活跃 run：{session_id}")));
    };
    let payload = {
        let mut plan = handle
            .plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = plan.as_mut() else {
            return Err(IpcError::invalid_arg("尚无计划可批准（等模型先建计划）"));
        };
        state.approve(now_ms());
        serde_json::json!({
            "sessionId": session_id,
            "items": state.items,
            "paths": state.paths,
            "approved": state.is_approved(),
            "baselinePaths": state.baseline_paths,
        })
    };
    events.emit(keys::PLAN_UPDATE, payload);
    Ok(())
}

/// approve_plan 命令。
#[tauri::command]
pub(crate) async fn approve_plan(
    app: State<'_, AppState>,
    runs: State<'_, RunManager>,
    session_id: String,
) -> Result<(), IpcError> {
    approve_plan_core(&runs, &session_id, &app.events)
}

// ---- 帧重放与热重连 ----

/// `attach_run` 参数契约（文档命名锚点，channel 由 tauri 平铺形参注入）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AttachRunArgs {
    pub session_id: String,
    pub since_seq: u64,
}

/// `attach_run` 返回契约：runId 省略 = 无活跃 run（空重放）。
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AttachRunResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub run_id: Option<String>,
    pub replayed: u64,
}

/// `session_run_snapshot` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionRunSnapshotArgs {
    pub session_id: String,
}

/// `session_pending_ask` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionPendingAskArgs {
    pub session_id: String,
}

/// 的返回体：仍有效的未决询问/审批（原始载荷随行）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionPendingAsk {
    pub ask_id: String,
    pub run_id: String,
    /// 载荷形态（"question" / "approval"）。
    pub kind: String,
    /// 开启时的原始载荷（approval 含 diff/命令/围栏原因与选项）。
    pub payload: AskPayload,
}

/// attach_run 主体：重放 `(sinceSeq, 当前头]` 区间帧并
/// 把帧通道热切换到新下发端（重放窗口耗尽时补一次 runSnapshot 摘要帧）。
/// 幂等查询语义：无活跃 run / sessionId 未知 → 空重放
/// （runId 省略），不报 E_NOT_FOUND。
pub fn attach_run_core(
    runs: &RunManager,
    session_id: &str,
    since_seq: u64,
    out: Box<dyn SequencedSink>,
) -> AttachRunResult {
    let Some(handle) = runs.get(session_id) else {
        return AttachRunResult::default();
    };
    let outcome = handle.attach(since_seq, out);
    AttachRunResult {
        run_id: Some(handle.run_id.clone()),
        replayed: outcome.replayed,
    }
}

/// session_run_snapshot 主体（对账）：会话有活跃 run →
/// 运行时快照；无活跃 run / sessionId 未知 → null（幂等查询语义）。
pub fn session_run_snapshot_core(runs: &RunManager, session_id: &str) -> Option<RunSnapshot> {
    runs.get(session_id).map(|handle| handle.run_snapshot())
}

/// session_pending_ask 主体（待审批恢复）：返回该会话仍
/// 有效的未决询问/审批（原始载荷随行）；有效性以后端 ask 槽为权威——
/// 槽位收口（已答/已取消/超时）即查不到；无未决 ask / sessionId 未知 →
/// null（幂等查询语义）。
pub fn session_pending_ask_core(runs: &RunManager, session_id: &str) -> Option<SessionPendingAsk> {
    let handle = runs.get(session_id)?;
    let slot = handle
        .ask
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let ask = slot.as_ref()?;
    Some(SessionPendingAsk {
        ask_id: ask.ask_id.clone(),
        run_id: ask.run_id.clone(),
        kind: ask.kind.to_owned(),
        payload: ask.payload.clone(),
    })
}

/// attach_run 命令。
#[tauri::command]
pub(crate) async fn attach_run(
    runs: State<'_, RunManager>,
    session_id: String,
    since_seq: u64,
    channel: Channel<SequencedFrame>,
) -> Result<AttachRunResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构（channel 不可序列化，
    // 不进 args 结构体）
    let args = AttachRunArgs {
        session_id,
        since_seq,
    };
    Ok(attach_run_core(
        &runs,
        &args.session_id,
        args.since_seq,
        Box::new(ChannelSink { channel }),
    ))
}

/// session_run_snapshot 命令。
#[tauri::command]
pub(crate) async fn session_run_snapshot(
    runs: State<'_, RunManager>,
    session_id: String,
) -> Result<Option<RunSnapshot>, IpcError> {
    let args = SessionRunSnapshotArgs { session_id };
    Ok(session_run_snapshot_core(&runs, &args.session_id))
}

/// session_pending_ask 命令。
#[tauri::command]
pub(crate) async fn session_pending_ask(
    runs: State<'_, RunManager>,
    session_id: String,
) -> Result<Option<SessionPendingAsk>, IpcError> {
    let args = SessionPendingAskArgs { session_id };
    Ok(session_pending_ask_core(&runs, &args.session_id))
}
