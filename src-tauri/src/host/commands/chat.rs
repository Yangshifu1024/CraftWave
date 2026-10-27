//! start_chat / cancel_run 命令（§3.3 #18/#19，M1 单会话核心链路）。
//!
//! - 命令层只做校验/转调与装配（§2.2）：模型解析（§11.1：RunConfig.modelId >
//!   AppConfig.activeModelId）→ wire 切分（§6.2）→ 工具集（§8.1 档位过滤）→
//!   RunManager 注册 → spawn RunLoop 任务（`backend::run_loop::drive_run`）；
//! - 帧出口：tauri `Channel` 适配为 `FrameSink`，出口统一填 `seq` + `ts`
//!   （§3.4 帧基座）；
//! - provider 只接 openai_chat 方言（§11.1 的 rig 构造面；anthropic/responses
//!   随 provider 适配层后续票），Key 经 keyring 读取（测试用内存 mock 注入，
//!   真实钥匙串不在本票测试路径）；
//! - 错误终态兜底：RunLoop 内部失败（PromptError）由 run 任务发
//!   `runEnded(error)`（§11.3 分类器为 provider 适配层后续票，本票 E_INTERNAL）。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use rig::completion::request::ToolDefinition;
use rig::message::ToolName;
use rig::providers::openai::wire::{Chat, OpenAIConfig};
use rig::run::AgentRun;
use rig::run::spec::RunSpec;
use serde::{Deserialize, Serialize};
use tauri::State;
use tauri::ipc::Channel;

use crate::agent::RunManager;
use crate::backend::exec::ToolsetExecutor;
use crate::backend::frames::{ChatFrame, EndReason, FrameError, FrameSink, SequencedFrame};
use crate::backend::run_loop::{AllowAllGate, CancellableExecutor, drive_run};
use crate::backend::turn::TurnDriver;
use crate::host::AppState;
use crate::providers::types::{ApiFormat, ProviderConfig, ProviderModel};
use crate::safety::tier::PermissionTier;
use crate::storage::{MessageEntry, SessionStore};
use crate::util::errors::{E_INTERNAL, IpcError};

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
            ts: Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
            ),
            frame,
        };
        // 前端掉线（webview 重载）时 Channel 发送失败：M1 只读快照语义（§5.6），
        // run 继续执行，帧丢弃。
        let _ = self.channel.send(sequenced);
    }
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

/// start_chat 主体（命令薄包装转调；公开给集成测试注入内存 keyring + 临时
/// config 与测试 FrameSink——不经 tauri 运行时）。
pub async fn start_chat_core(
    app: &AppState,
    runs: &RunManager,
    session_id: String,
    messages: Vec<MessageEntry>,
    run_config: RunConfig,
    sink: Box<dyn FrameSink>,
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
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let handle = runs.start(&session_id, run_id.clone(), now)?;

    // ⑤ spawn RunLoop 任务（帧生产 + 取消 + 终态摘除）。max_turns 经 RunSpec
    //    传给 AgentRun::from_spec（其 effective_max_turns 以 spec 为准）。
    let spec = RunSpec {
        max_turns: Some(run_config.max_turns as usize),
        ..RunSpec::new()
    };
    let driver = TurnDriver::new(model.driver, spec.clone(), tools);
    let model_name = model.name;
    let cancel = handle.cancel.clone();
    let executor: Arc<dyn CancellableExecutor> = Arc::new(ToolsetExecutor::new(
        // 临时会话 roots = 全局数据目录（§6.2 注；项目会话随项目票接入）
        crate::tools::ToolsContext::new(SessionStore::default_root(), SessionStore::default_root()),
        run_config.tier,
    ));
    let runs_for_task = runs.clone();
    let session_for_task = session_id.clone();
    let run_id_for_task = run_id.clone();
    tokio::spawn(async move {
        let mut sink = sink;
        let run = AgentRun::from_spec(&spec, (*prompt).clone(), Some(history));
        let result = drive_run(
            &driver,
            run,
            &run_id_for_task,
            &model_name,
            cancel,
            executor,
            Arc::new(AllowAllGate),
            &mut *sink,
        )
        .await;
        if let Err(err) = result {
            // 错误终态兜底：任何 rig 失败都以 runEnded(error) 收场（§5.3 终态不变量）
            send_error_ended(&mut *sink, err.to_string());
        }
        runs_for_task.remove(&session_for_task, &run_id_for_task);
    });

    Ok(StartChatResult { run_id })
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
    start_chat_core(
        &app,
        &runs,
        args.session_id,
        args.messages,
        args.run_config,
        sink,
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
