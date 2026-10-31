//! run 装配的模型解析域（平移自旧壳 `host/commands/chat.rs`）。
//!
//! - [`ResolvedModel`]：解析产物（模型元数据 + 可驱动客户端）；
//! - [`ModelResolver`]：**模型构造接缝**——生产实现按 AppConfig 查找
//!   配置行并构造三方言统一驱动（`providers::wire` 防腐层）；测试实现
//!   返回 rig 剧本假模型（`MockCompletionModel`），端到端测试不依赖真
//!   provider Key。整条装配链（主 run / 压缩摘要 / 标题 / 审计 / 每拍
//!   刷新）统一经本接缝取模型，切接缝即切驱动来源，装配逻辑一行不改；
//! - [`effective_thinking_directive`]：生效 thinking 档的优先级链取值；
//! - [`to_rig_tool_definitions`]：工具表 → rig 广播面；
//! - [`load_image_blobs`]：消息图片 blob 装配（wire 重建的 IO 面）。

use std::sync::{Arc, Mutex};

use rig::completion::request::ToolDefinition;
use rig::driver::Model;
use rig::http_client::DynHttpClient;
use rig::operation::Completion;
use rig::wire::Wire;
use rig::driver::Transport;

use crate::app_state::AppState;
use crate::providers::types::{ApiFormat, ProviderModel, ReasoningEffort};
use crate::providers::wire::DialectWire;
use crate::storage::MessageEntry;
use crate::util::errors::IpcError;

/// 模型驱动的 wire 泛型界（装配链的统一上界）：生产
/// `DialectWire`/`DynHttpClient`、测试 rig 剧本（`MockScript`/`MockRuntime`）。
/// `Send + Sync + 'static`：解析产物要跨 `tokio::spawn` 进 run 任务。
pub trait RunWire: Wire<Op = Completion> + Send + Sync + 'static {}
impl<W: Wire<Op = Completion> + Send + Sync + 'static> RunWire for W {}

/// 模型驱动的传输泛型界（理由同 [`RunWire`]）。
pub trait RunTransport<W: RunWire>: Transport<W> + Send + Sync + 'static {}
impl<W: RunWire, T: Transport<W> + Send + Sync + 'static> RunTransport<W> for T {}

/// 解析后的运行模型（旧壳 chat.rs 同名结构的泛型化）。
pub(crate) struct ResolvedModel<W: RunWire, T: RunTransport<W>> {
    /// 模型配置 id（`ProviderModel.id`；用量统计按实际执行模型
    /// 归属的记账面——显示名可改可删，配置 id 是稳定聚合键）。
    pub id: String,
    /// 显示名（runStarted.model，= wire id）。
    pub name: String,
    /// 可驱动客户端（生产 = 三方言统一 `DialectWire`；测试 = rig 剧本模型）。
    pub driver: Model<W, T>,
    /// 上下文窗口 token 数（`ProviderModel.contextWindow`，缺省 128000；
    /// 压缩预算检查的窗口面）。
    pub context_window: u64,
    /// 单次回复 token 上限（`ProviderModel.maxTokens`，缺省 32768；
    /// 每请求预算的回复预留面，Anthropic 方言的请求级 max_tokens 注入面）。
    pub max_tokens: u64,
    /// 是否支持图片输入（`ProviderModel.vision`，wire 重建
    /// 双形态与 read(kind=image) 前置校验的判定面）。
    pub vision: bool,
    /// 请求方言（thinking 档位按方言走 映射表）。
    pub api_format: ApiFormat,
    /// per-model reasoning 档配置（ProviderModel.reasoningEffort；
    /// 会话未显式设置 thinking 时的回落源，优先级链第二级）。
    pub reasoning_effort: Option<ReasoningEffort>,
    /// API 端点基地址（供应商 baseUrl 或官方缺省，resolve 时快照）——
    /// 会话日志 llm 行的 host 元数据面（诊断用；不涉敏感值）。
    pub base_url: String,
}

/// 模型解析接缝：`(app, 配置 id)` → 解析产物。`model_id` 为 None 时按
/// `AppConfig.activeModelId` 回落（与旧壳 `resolve_model` 同语义）。
pub(crate) type ModelResolver<W, T> =
    Arc<dyn Fn(&AppState, Option<&str>) -> Result<ResolvedModel<W, T>, IpcError> + Send + Sync>;

/// 生产解析器：按 AppConfig 查找 `(provider, model)` 配置行，Key 校验
/// （本地服务占位）后经 `providers::wire::dialect_model` 构造三方言统一
/// 驱动（挂代理工厂当前快照的 HTTP 客户端——模型请求与探测/网络工具共用
/// 代理语义；client 按 run 取用且 run 内复用，代理热生效后下一次
/// start_chat 换新）。
pub(crate) fn dialect_resolver() -> ModelResolver<DialectWire, DynHttpClient> {
    Arc::new(|app: &AppState, model_id: Option<&str>| {
        let config = app.config.load()?;
        let wanted = model_id.or(config.active_model_id.as_deref());
        let Some(wanted) = wanted else {
            return Err(IpcError::invalid_arg(
                "未指定模型：RunConfig.modelId 与 AppConfig.activeModelId 均为空",
            ));
        };
        let mut found: Option<(&crate::providers::types::ProviderConfig, &ProviderModel)> = None;
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
        let http = DynHttpClient::new(rig_reqwest::ReqwestClient::from(
            app.http.client().as_ref().clone(),
        ));
        let driver = crate::providers::wire::dialect_model(
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
    })
}

/// 生效 thinking 档的解析（优先级链）：会话显式档 >
/// per-model reasoningEffort 配置 > 不传。输出 映射指令供请求参数
/// 装配（纯函数在 `providers::reasoning`，本函数只做取值与回落）。
pub(crate) fn effective_thinking_directive<W: RunWire, T: RunTransport<W>>(
    session_thinking: Option<crate::providers::reasoning::ThinkingLevel>,
    model: &ResolvedModel<W, T>,
) -> crate::providers::reasoning::ReasoningDirective {
    let window = model.context_window;
    let max_tokens = model.max_tokens;
    match session_thinking {
        Some(level) => crate::providers::reasoning::thinking_to_request(
            level,
            model.api_format,
            window,
            max_tokens,
        ),
        None => match model.reasoning_effort {
            Some(effort) => crate::providers::reasoning::model_effort_to_request(
                effort,
                model.api_format,
                window,
                max_tokens,
            ),
            None => crate::providers::reasoning::ReasoningDirective::none(),
        },
    }
}

/// ToolSpec → rig ToolDefinition（广播给模型的工具表）。
pub(crate) fn to_rig_tool_definitions(
    specs: Vec<crate::tools::ToolSpec>,
) -> Result<Vec<Arc<ToolDefinition>>, IpcError> {
    specs
        .into_iter()
        .map(|spec| {
            let name = rig::message::ToolName::new(spec.name.to_owned())
                .map_err(|e| IpcError::invalid_arg(format!("非法工具名 {}: {e}", spec.name)))?;
            Ok(Arc::new(ToolDefinition::new(
                name,
                spec.description,
                spec.params_schema,
            )))
        })
        .collect()
}

/// 收集消息里的图片 blob 并装配查找表（wire 重建的 IO 面）：
/// hash 去重后逐个从会话 imgblob 目录读取（字节 → base64 + 魔数嗅探 mime）。
/// 单个 blob 读取失败跳过（vision 模型下该图在 wire 重建时降级为占位文本，
/// 不拒绝请求——blob 可能已被会话删除等流程清走）；日志纪律：只记哈希。
pub(crate) fn load_image_blobs(
    root: &std::path::Path,
    session_id: &str,
    messages: &[MessageEntry],
) -> crate::backend::wire::ImageBlobMap {
    let mut hashes: Vec<&str> = Vec::new();
    for entry in messages {
        for part in &entry.parts {
            if let crate::storage::WirePart::Image { blob_hash, .. } = part
                && !hashes.contains(&blob_hash.as_str())
            {
                hashes.push(blob_hash);
            }
        }
    }
    let mut map = crate::backend::wire::ImageBlobMap::new();
    for hash in hashes {
        match crate::storage::get_blob(root, session_id, hash) {
            Ok((bytes, mime)) => {
                use base64::Engine;
                map.insert(
                    hash.to_owned(),
                    crate::backend::wire::ImageBlobData {
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

/// 测试期的模型槽共享句柄类型别名（`(配置 id, 显示名)` 槽的现行类型；
/// 提取别名让 sinks/tasks/run_task 的签名可读）。
pub(crate) type ModelSlot = Arc<Mutex<Option<(String, String)>>>;
