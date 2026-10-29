//! CW schema v2 配置类型（§3.2 共享类型，Rust 侧逐一镜像；D14 camelCase）。
//!
//! `AppConfig` 即 `~/.craftwave/config.json` 的载药结构（§7.1），
//! `normalize` 是配置协调器（§7.6）的"入参校验/归一化"步骤。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::storage::config_store::ConfigPayload;
use crate::util::errors::IpcError;

/// 当前 schema 版本（CW schema v2，§3.2）。
pub const SCHEMA_VERSION: u32 = 2;

/// 默认 max_tokens（§3.2 ProviderModel 注记）。
pub const DEFAULT_MAX_TOKENS: u32 = 32768;
/// 默认 context_window（§3.2 ProviderModel 注记）。
pub const DEFAULT_CONTEXT_WINDOW: u32 = 128000;
/// 单供应商 Key 数量上限（§11.2）。
pub const MAX_KEYS_PER_PROVIDER: usize = 10;

// ---- 供应商（CW schema v2，实现 FR-02-01/06/07）----

/// apiFormat：决定 rig Model 构造方言（§11.1）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiFormat {
    /// OpenAI Chat Completions 方言（baseUrl 可覆写，兼容端点用）。
    OpenaiChat,
    /// Anthropic Messages 方言。
    AnthropicMessages,
    /// OpenAI Responses API 方言。
    OpenaiResponses,
}

// ApiFormat 的辅助方法。
impl ApiFormat {
    /// 官方缺省地址（baseUrl 缺省时按 apiFormat 取，§3.2 注记）。
    pub fn official_base_url(self) -> &'static str {
        match self {
            Self::OpenaiChat => "https://api.openai.com/v1",
            Self::AnthropicMessages => "https://api.anthropic.com",
            Self::OpenaiResponses => "https://api.openai.com/v1",
        }
    }
}

/// reasoning 映射档位（§11.4）：控制模型思考力度的档位值，
/// 序列化为 snake_case 透传给供应商 API。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    /// 供应商缺省档（不显式传该参数）。
    Default,
    /// 低思考。
    Low,
    /// 中思考。
    Medium,
    /// 高思考。
    High,
    /// 最高思考。
    Max,
}

/// 单个供应商的完整配置（对应设置页里的一个供应商卡片）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// 供应商的唯一标识（uuid）。
    pub id: String,
    /// 展示名（非空，归一化时去首尾空白）。
    pub name: String,
    /// API 请求方言，决定构造请求的方式。
    pub api_format: ApiFormat,
    /// 缺省按 apiFormat 用官方缺省地址（§3.2）；自定义 OpenAI 兼容端点在此覆写。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub base_url: Option<String>,
    /// API Key 明文（用户决策废除 keyring，2026-10-28：Key 存 config.json，
    /// 文件权限 0600 收紧；§9.4 已改写）。空串归一为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub api_key: Option<String>,
    /// 该供应商下的模型清单（至少一个，归一化强制）。
    pub models: Vec<ProviderModel>,
    /// 追加请求头（M2 per-provider headers 正式启用；字段 M1 就位）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub headers: Option<BTreeMap<String, String>>,
}

/// 供应商下的单个模型条目（含窗口参数与能力开关）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModel {
    /// uuid，activeModelId 指向它。
    pub id: String,
    /// wire id，即显示名（API 请求里 model 字段的取值）。
    pub model: String,
    /// 单次回复 token 上限；缺省/为 0 时归一化为 32768（DEFAULT_MAX_TOKENS）。
    #[serde(default)]
    pub max_tokens: u32,
    /// 上下文窗口 token 数；缺省/为 0 时归一化为 128000（DEFAULT_CONTEXT_WINDOW）。
    #[serde(default)]
    pub context_window: u32,
    /// reasoning 档位（可空 = 不传给供应商）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub reasoning_effort: Option<ReasoningEffort>,
    /// 是否支持图片输入（可空 = 未知）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub vision: Option<bool>,
}

// ---- 应用配置（实现 FR-14）----

/// 代理模式：不走代理 / 跟随系统代理 / 手动指定代理地址。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    /// 不使用代理（直连）。
    None,
    /// 跟随操作系统代理设置。
    System,
    /// 使用手动配置的代理地址。
    Manual,
}

/// 代理配置（manual 模式下必须给 url）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfig {
    /// 代理模式。
    pub mode: ProxyMode,
    /// 手动代理地址（仅 manual 模式有意义）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub url: Option<String>,
}

/// 审批门配置（控制高危动作是否需要用户确认）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalConfig {
    /// 审批总开关（false 时不再弹出任何审批）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 自动确认（不打扰用户；危险命令围栏仍会硬拦）。
    #[serde(default)]
    pub auto_confirm: bool,
    /// 命令白名单：命中即免审批直接放行。
    #[serde(default)]
    pub command_allowlist: Vec<String>,
}

/// 界面主题。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// 浅色。
    Light,
    /// 深色。
    Dark,
    /// 跟随操作系统（缺省）。
    #[default]
    System,
}

/// 界面语言。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    /// 简体中文（缺省）。
    #[serde(rename = "zh-CN")]
    ZhCn,
    /// 美式英语。
    #[serde(rename = "en-US")]
    EnUs,
}

/// 界面偏好设置。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiConfig {
    /// 界面语言（缺省中文）。
    #[serde(default = "default_language")]
    pub language: Language,
    /// 主题（缺省跟随系统）。
    #[serde(default)]
    pub theme: Theme,
    /// 模型回复语言（可空 = 跟随用户输入）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub ai_language: Option<String>,
    /// 界面基准字号（12–18px；缺省沿用 14px，不缩放布局）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub font_size: Option<u8>,
}

/// 日志级别（serilog 风格，低到高）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// 调试细节。
    Debug,
    /// 常规信息（缺省）。
    Info,
    /// 警告。
    Warn,
    /// 错误。
    Error,
}

/// 日志设置。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogConfig {
    /// 日志级别（缺省 Info）。
    #[serde(default = "default_log_level")]
    pub level: LogLevel,
}

/// 系统通知偏好（FR-13-06 设置项，17 号票）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationConfig {
    /// 通知总开关。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 仅人工介入（ask/审批等待）时通知；false 时 run 终态也通知。
    #[serde(default)]
    pub intervention_only: bool,
}

// NotificationConfig 的缺省值：通知开、所有 run 终态都通知。
impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            intervention_only: false,
        }
    }
}

/// 智能体行为配置（§3.2 agent 组）。M2 03 号票起承载 customPrompt；
/// maxConcurrentSubagents（10 号票）/ commandAudit（36 号票）随对应票补齐。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    /// 自定义提示词（FR-09-06）：非空时注入为每次对话的系统级前缀
    /// （§6.2 注入次序首位：customPrompt → 压缩 summary → 当前计划）。
    /// 空/未配置零注入；save_config 即时生效于下次 run（§7.6 缓存热替换）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub custom_prompt: Option<String>,
}

/// 上下文压缩配置（§3.2 compaction 组，FR-03-05，04 号票实装）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionConfig {
    /// 自动压缩阈值（上下文占用/窗口占比，(0,1]；缺省 0.6）。
    #[serde(default = "default_compaction_threshold")]
    pub threshold: f64,
    /// 压缩总超时秒数（分块摘要共用同一预算，不能按块重置；缺省 180）。
    #[serde(default = "default_compaction_timeout_secs")]
    pub timeout_secs: u64,
    /// 专用摘要小模型 id（指向某 ProviderModel.id；未配置时用会话 active model）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub model_id: Option<String>,
}

/// serde 缺省值函数：自动压缩阈值缺省 0.6（§6.2）。
fn default_compaction_threshold() -> f64 {
    0.6
}
/// serde 缺省值函数：压缩总超时缺省 180 秒（§6.2）。
fn default_compaction_timeout_secs() -> u64 {
    180
}

// CompactionConfig 的缺省值：0.6 阈值 / 180 秒总超时 / 无专用小模型（随 active model）。
impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            threshold: default_compaction_threshold(),
            timeout_secs: default_compaction_timeout_secs(),
            model_id: None,
        }
    }
}

/// 同批工具并发上限的缺省值（§3.2 tools 组注记，FR-03-11）。
pub const DEFAULT_MAX_PARALLEL: u32 = 4;
/// maxParallel 合法区间上限（§3.2 tools 组注记：1–16）。
pub const MAX_PARALLEL_LIMIT: u32 = 16;

/// 工具批执行配置（§3.2 tools 组，FR-03-11/FR-04-04，07 号票实装）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsConfig {
    /// 同批工具并发上限：批内无冲突调用（ReadOnly/Meta/Network）共享的信号量
    /// 额度，缺省 4，合法区间 1–16；文件写不走此额度（进程级全局互斥队列
    /// 串行，§5.4 ⑤）；按 run 装配执行器时读取生效（save_config 后下一次
    /// start_chat 应用）。
    #[serde(default = "default_max_parallel")]
    pub max_parallel: u32,
}

/// serde 缺省值函数：同批工具并发上限缺省 4（§3.2）。
fn default_max_parallel() -> u32 {
    DEFAULT_MAX_PARALLEL
}

// ToolsConfig 的缺省值：并发上限 4（§3.2）。
impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            max_parallel: DEFAULT_MAX_PARALLEL,
        }
    }
}

/// 应用总配置，即 `~/.craftwave/config.json` 的载入/落盘结构：
/// 一个 JSON 文件承载全部设置（供应商、激活模型、代理、审批、界面、日志、通知）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    /// 配置格式版本号，当前必须为 2（CW schema v2）。
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// 供应商清单。
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// 当前选中的模型 id（指向某个 ProviderModel.id；悬空时归一化为 None）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub active_model_id: Option<String>,
    /// 代理设置（可空 = 未配置）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub proxy: Option<ProxyConfig>,
    /// 审批门设置。
    #[serde(default)]
    pub approval: ApprovalConfig,
    /// 界面偏好。
    #[serde(default)]
    pub ui: UiConfig,
    /// 日志设置。
    #[serde(default)]
    pub log: LogConfig,
    /// 系统通知偏好。
    #[serde(default)]
    pub notifications: NotificationConfig,
    /// 智能体行为配置（可空 = 全部缺省，§3.2 agent 组）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub agent: Option<AgentConfig>,
    /// 上下文压缩（FR-03-05，04 号票；缺省值经 Default：0.6 阈值 / 180s 超时）。
    #[serde(default)]
    pub compaction: CompactionConfig,
    /// 工具批执行（FR-03-11/FR-04-04，07 号票；缺省值经 Default：并发上限 4）。
    #[serde(default)]
    pub tools: ToolsConfig,
}

/// serde 缺省值函数：schemaVersion 字段缺省时取当前版本号 2。
fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}
/// serde 缺省值函数：布尔字段缺省为 true。
fn default_true() -> bool {
    true
}
/// serde 缺省值函数：界面语言缺省为中文。
fn default_language() -> Language {
    Language::ZhCn
}
/// serde 缺省值函数：日志级别缺省为 Info。
fn default_log_level() -> LogLevel {
    LogLevel::Info
}

// ApprovalConfig 的缺省值：审批开、不自动确认、白名单为空（最保守）。
impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_confirm: false,
            command_allowlist: Vec::new(),
        }
    }
}
// UiConfig 的缺省值：中文界面、跟随系统主题、模型语言跟随用户输入。
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            language: Language::ZhCn,
            theme: Theme::System,
            ai_language: None,
            font_size: None,
        }
    }
}
// LogConfig 的缺省值：Info 级别。
impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
        }
    }
}
// AppConfig 的缺省值：空供应商清单 + 各子配置自身缺省值。
impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            providers: Vec::new(),
            active_model_id: None,
            proxy: None,
            approval: ApprovalConfig::default(),
            ui: UiConfig::default(),
            log: LogConfig::default(),
            notifications: NotificationConfig::default(),
            agent: None,
            compaction: CompactionConfig::default(),
            tools: ToolsConfig::default(),
        }
    }
}

// AppConfig 的归一化与校验方法。
impl AppConfig {
    /// 归一化 + 校验（§7.6 协调器第一步）。规则：
    /// - schemaVersion 必须为 2（CW schema v2 之外拒绝）；
    /// - provider/model 的 id、name 非空，baseUrl（如有）必须 http(s)；
    /// - model 的 maxTokens/contextWindow 缺省/为 0 → 默认值（32768/128000）；
    /// - activeModelId 悬空（无对应模型）→ 自愈为 None；
    /// - apiKey 空白归一为 None。
    pub fn normalize(&mut self) -> Result<(), IpcError> {
        if self
            .ui
            .font_size
            .is_some_and(|size| !(12..=18).contains(&size))
        {
            return Err(IpcError::invalid_arg("界面字号必须在 12–18px 之间"));
        }
        // compaction：threshold 限 (0,1]（0 或负数会让每条消息都触发压缩，
        // 大于 1 则永不触发，均视为配置错误拒绝整份提交）；timeoutSecs 下限
        // 30s（低于该值分块摘要几乎没有完成机会）；modelId 空白归一为 None。
        if !(0.0 < self.compaction.threshold && self.compaction.threshold <= 1.0) {
            return Err(IpcError::invalid_arg(format!(
                "compaction.threshold 必须在 (0,1] 内（实际 {}）",
                self.compaction.threshold
            )));
        }
        if self.compaction.timeout_secs < 30 {
            return Err(IpcError::invalid_arg(
                "compaction.timeoutSecs 不能低于 30 秒",
            ));
        }
        // tools.maxParallel：正整数且不超过 16（§3.2 tools 组注记——并发度过高
        // 会放大命令/网络类工具的资源占用，超限视为配置错误拒绝整份提交）。
        if self.tools.max_parallel == 0 || self.tools.max_parallel > MAX_PARALLEL_LIMIT {
            return Err(IpcError::invalid_arg(format!(
                "tools.maxParallel 必须在 1–{MAX_PARALLEL_LIMIT} 内（实际 {}）",
                self.tools.max_parallel
            )));
        }
        self.compaction.model_id = self
            .compaction
            .model_id
            .take()
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty());
        // agent.customPrompt：空白归一为 None（"没填"与"填了又删掉"等价零注入）
        if let Some(agent) = &mut self.agent {
            agent.custom_prompt = agent
                .custom_prompt
                .take()
                .map(|p| p.trim().to_owned())
                .filter(|p| !p.is_empty());
        }
        if self.schema_version != SCHEMA_VERSION {
            return Err(IpcError::invalid_arg(format!(
                "不支持的 schemaVersion：{}（当前仅支持 {SCHEMA_VERSION}）",
                self.schema_version
            )));
        }
        for provider in &mut self.providers {
            provider.name = provider.name.trim().to_string();
            if provider.name.is_empty() {
                return Err(IpcError::invalid_arg(format!(
                    "供应商 {} 的 name 不能为空",
                    provider.id
                )));
            }
            if provider.id.trim().is_empty() {
                return Err(IpcError::invalid_arg("供应商 id 不能为空"));
            }
            if let Some(url) = &provider.base_url {
                ensure_http_url(url)?;
            }
            provider.api_key = provider.api_key.take().filter(|k| !k.trim().is_empty());
            for (name, _) in provider.headers.iter().flatten() {
                if name.trim().is_empty() {
                    return Err(IpcError::invalid_arg(format!(
                        "供应商 {} 存在空 header 名",
                        provider.id
                    )));
                }
            }
            if provider.models.is_empty() {
                return Err(IpcError::invalid_arg(format!(
                    "供应商 {} 至少需要一个模型",
                    provider.id
                )));
            }
            for model in &mut provider.models {
                model.id = model.id.trim().to_string();
                model.model = model.model.trim().to_string();
                if model.id.is_empty() || model.model.is_empty() {
                    return Err(IpcError::invalid_arg(format!(
                        "供应商 {} 存在 id/model 为空的模型",
                        provider.id
                    )));
                }
                if model.max_tokens == 0 {
                    model.max_tokens = DEFAULT_MAX_TOKENS;
                }
                if model.context_window == 0 {
                    model.context_window = DEFAULT_CONTEXT_WINDOW;
                }
            }
        }
        // activeModelId 悬空自愈
        if let Some(active) = &self.active_model_id {
            let exists = self
                .providers
                .iter()
                .any(|p| p.models.iter().any(|m| &m.id == active));
            if !exists {
                self.active_model_id = None;
            }
        }
        Ok(())
    }
}

/// 校验 baseUrl 必须是带主机名的 http/https 地址（防 file://、ftp:// 等
/// 非预期协议进入配置）。
fn ensure_http_url(url: &str) -> Result<(), IpcError> {
    let ok = url.starts_with("http://") || url.starts_with("https://");
    if ok && url.len() > "https://".len() {
        Ok(())
    } else {
        Err(IpcError::invalid_arg(format!(
            "baseUrl 必须为合法 http(s) 地址：{url}"
        )))
    }
}

/// AppConfig 实现协调器载荷 trait（providers → storage 单向依赖，§2.2）。
impl ConfigPayload for AppConfig {
    fn normalize(&mut self) -> Result<(), IpcError> {
        AppConfig::normalize(self)
    }
}
