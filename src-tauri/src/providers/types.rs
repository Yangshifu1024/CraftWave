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

/// reasoning 映射档位（§11.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Default,
    Low,
    Medium,
    High,
    Max,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub api_format: ApiFormat,
    /// 缺省按 apiFormat 用官方缺省地址（§3.2）；自定义 OpenAI 兼容端点在此覆写。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// keyring 账户引用（§9.4）；config 永不存明文 Key。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
    /// 展示用 Key 数量（由 keyring 提交协议回填）。
    pub credential_count: u32,
    pub models: Vec<ProviderModel>,
    /// 追加请求头（M2 per-provider headers 正式启用；字段 M1 就位）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModel {
    /// uuid，activeModelId 指向它。
    pub id: String,
    /// wire id，即显示名。
    pub model: String,
    /// default 32768。
    #[serde(default)]
    pub max_tokens: u32,
    /// default 128000。
    #[serde(default)]
    pub context_window: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
}

// ---- 应用配置（实现 FR-14）----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    None,
    System,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfig {
    pub mode: ProxyMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub auto_confirm: bool,
    #[serde(default)]
    pub command_allowlist: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    #[default]
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en-US")]
    EnUs,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiConfig {
    #[serde(default = "default_language")]
    pub language: Language,
    #[serde(default)]
    pub theme: Theme,
    /// 模型回复语言（可空 = 跟随用户输入）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_language: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogConfig {
    #[serde(default = "default_log_level")]
    pub level: LogLevel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyConfig>,
    #[serde(default)]
    pub approval: ApprovalConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub log: LogConfig,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}
fn default_true() -> bool {
    true
}
fn default_language() -> Language {
    Language::ZhCn
}
fn default_log_level() -> LogLevel {
    LogLevel::Info
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_confirm: false,
            command_allowlist: Vec::new(),
        }
    }
}
impl Default for UiConfig {
    fn default() -> Self {
        Self {
            language: Language::ZhCn,
            theme: Theme::System,
            ai_language: None,
        }
    }
}
impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
        }
    }
}
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
        }
    }
}

impl AppConfig {
    /// 归一化 + 校验（§7.6 协调器第一步）。规则：
    /// - schemaVersion 必须为 2（CW schema v2 之外拒绝）；
    /// - provider/model 的 id、name 非空，baseUrl（如有）必须 http(s)；
    /// - model 的 maxTokens/contextWindow 缺省/为 0 → 默认值（32768/128000）；
    /// - activeModelId 悬空（无对应模型）→ 自愈为 None；
    /// - credentialCount ≤ 10（§11.2 Key 上限）。
    pub fn normalize(&mut self) -> Result<(), IpcError> {
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
            if provider.credential_count as usize > MAX_KEYS_PER_PROVIDER {
                return Err(IpcError::invalid_arg(format!(
                    "供应商 {} 的 Key 数量超过上限 {MAX_KEYS_PER_PROVIDER}",
                    provider.id
                )));
            }
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
