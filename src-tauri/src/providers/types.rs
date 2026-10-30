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
    /// 本地服务标记（FR-02-04，19 号票）：Ollama / llama.cpp / LM Studio 等
    /// 本机推理端点——免 API Key（`resolve_model` 允许 apiKey 缺省，rig 侧
    /// 以占位值发请求，本地服务不校验凭据），设置页隐藏 Key 输入并标注
    /// "本地 / 离线可用"。仅按本字段显式判定，不做"baseUrl 是 localhost 即
    /// 本地"的隐式推断（远端自建网关同样可能是回环地址）。旧配置无该字段
    /// serde default 回填 false（schemaVersion 维持 2，§3.2）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
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
    /// 审批总开关（false 时不再弹出任何审批；黑名单/内部状态硬拦/计划门/
    /// MCP 审批与信任询问不受影响，plan 档判定不变——23 号票字段级）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 自动确认（不打扰用户；危险命令围栏仍会硬拦）。
    #[serde(default)]
    pub auto_confirm: bool,
    /// ③ 会话内记忆启用（23 号票，FR-14-04）：true 时命令类审批展示
    /// 「始终允许（本会话）」选项（AskPayload option mode=always_session），
    /// 选中后同签名命令本会话跨 run 免批；false 不出该选项。默认开。
    #[serde(default = "default_true")]
    pub session_memory_enabled: bool,
    /// ④ 命令白名单启用（23 号票，FR-14-04）：整体开关，false 时
    /// commandAllowlist 不参与判定（条目保留不删）。默认开。
    #[serde(default = "default_true")]
    pub allowlist_enabled: bool,
    /// 命令白名单：命中即免审批直接放行（23 号票字段级：词序列前缀匹配
    /// ——条目 `git status` 匹配 `git status …` 不匹配 `git statusx`；
    /// 围栏 Confirm 与矩阵 NeedsApproval 免除，黑名单 Block 永不豁免，
    /// plan 档不豁免）。
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

/// 子代理全局并发上限的缺省值（§3.2 agent 组/§5.9 Q13：所有会话共用，缺省 4）。
pub const DEFAULT_MAX_CONCURRENT_SUBAGENTS: u32 = 4;
/// maxConcurrentSubagents 合法区间上限（与工具批并发 tools.maxParallel 同界，
/// 防资源失控；下限 1 由"正整数"语义承担）。
pub const MAX_CONCURRENT_SUBAGENTS_LIMIT: u32 = 16;

/// 智能体行为配置（§3.2 agent 组）。M2 03 号票起承载 customPrompt；
/// M2 10 号票补 maxConcurrentSubagents（子代理全局并发上限）；commandAudit
/// （36 号票）随对应票补齐。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfig {
    /// 自定义提示词（FR-09-06）：非空时注入为每次对话的系统级前缀
    /// （§6.2 注入次序首位：customPrompt → 压缩 summary → 当前计划）。
    /// 空/未配置零注入；save_config 即时生效于下次 run（§7.6 缓存热替换）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub custom_prompt: Option<String>,
    /// 子代理全局并发上限（FR-07-04/§5.9 Q13，10 号票）：应用内所有会话共用，
    /// 满额排队不拒绝；要求正整数，旧配置缺省回填 4，非法值在保存边界拒绝。
    /// 运行中修改：已启动任务继续不取消；降低后暂停补入直到运行数低于新上限，
    /// 提高后立即按新上限启动排队任务（并发闸 [`crate::agent::sub_registry`]）。
    #[serde(default = "default_max_concurrent_subagents")]
    pub max_concurrent_subagents: u32,
}

/// serde 缺省值函数：子代理全局并发上限缺省 4（§5.9 Q13）。
fn default_max_concurrent_subagents() -> u32 {
    DEFAULT_MAX_CONCURRENT_SUBAGENTS
}

// AgentConfig 的缺省值：无自定义提示词 + 并发上限 4。
impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            custom_prompt: None,
            max_concurrent_subagents: DEFAULT_MAX_CONCURRENT_SUBAGENTS,
        }
    }
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

/// batch_read 单批文件数的缺省上限（§3.2 tools.batchReadLimit / §8.3
/// batch_read 行，14 号票）。
pub const DEFAULT_BATCH_READ_LIMIT: u32 = 20;
/// batchReadLimit 合法区间上限（§3.2 tools.batchReadLimit 注记：真正的硬顶
/// 是 batch_read 的 512KB 共享响应预算，本字段只是文件数闸门；上限 100
/// 覆盖大范围清单式调研）。
pub const MAX_BATCH_READ_LIMIT: u32 = 100;

/// 模型流停滞超时的缺省秒数（§3.2 watchdog 组/§5.7：300 秒）。
pub const DEFAULT_STALL_TIMEOUT_SECS: u32 = 300;
/// stallTimeoutSecs 合法区间下限（§5.7：5 秒）。
pub const MIN_STALL_TIMEOUT_SECS: u32 = 5;
/// stallTimeoutSecs 合法区间上限（§5.7：3600 秒）。
pub const MAX_STALL_TIMEOUT_SECS: u32 = 3600;

/// 监督看门狗配置（§3.2 watchdog 组，FR-03-07，37 号票实装）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchdogConfig {
    /// 模型流停滞超时秒数（§5.7 计时边界：仅模型请求阶段计时）：单次模型请求
    /// 自发出起无有效模型增量达到该时长即终止 run（E_RUN_STALLED）；缺省 300，
    /// 合法区间 5–3600；按 run 装配驱动时读取生效（save_config 后下一次
    /// start_chat 应用）。
    #[serde(default = "default_stall_timeout_secs")]
    pub stall_timeout_secs: u32,
}

/// serde 缺省值函数：模型流停滞超时缺省 300 秒（§5.7）。
fn default_stall_timeout_secs() -> u32 {
    DEFAULT_STALL_TIMEOUT_SECS
}

// WatchdogConfig 的缺省值：停滞超时 300 秒（§5.7）。
impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            stall_timeout_secs: DEFAULT_STALL_TIMEOUT_SECS,
        }
    }
}

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
    /// batch_read 工具的单批文件数上限（§8.3 batch_read 行「≤20 文件」的
    /// 可配承载，14 号票回写 §3.2 补齐字段级定义）：缺省 20，合法区间
    /// 1–100，越界拒绝整份提交；save_config 后下一次工具调用生效。
    #[serde(default = "default_batch_read_limit")]
    pub batch_read_limit: u32,
}

/// serde 缺省值函数：同批工具并发上限缺省 4（§3.2）。
fn default_max_parallel() -> u32 {
    DEFAULT_MAX_PARALLEL
}
/// serde 缺省值函数：batch_read 单批文件数上限缺省 20（§3.2/§8.3）。
fn default_batch_read_limit() -> u32 {
    DEFAULT_BATCH_READ_LIMIT
}

// ToolsConfig 的缺省值：并发上限 4 + 单批读取上限 20（§3.2）。
impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            max_parallel: DEFAULT_MAX_PARALLEL,
            batch_read_limit: DEFAULT_BATCH_READ_LIMIT,
        }
    }
}

/// 网络工具 SSRF 防护配置（§3.2 network 组，FR-05-07/§9.2，12 号票实装）。
/// Default（derive）：allow_private_network = false，拒绝私网访问（最保守，
/// §3.2 network 组注记）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkConfig {
    /// 允许 web_fetch/http_request 访问私网/回环/链路本地/CGNAT/ULA 地址
    /// （§9.2 全地址校验的设置例外）：缺省 false（拒绝，最保守）；旧配置
    /// 无该组时 serde default 回填 false；save_config 后下一次工具调用生效。
    #[serde(default)]
    pub allow_private_network: bool,
}

/// web_search 每次搜索的结果条数缺省值（§3.2 search.searxng 组/§8.3
/// web_search 行，13 号票）：工具未显式给 limit 参数时各通道用它截断。
pub const DEFAULT_SEARCH_MAX_RESULTS: u32 = 10;
/// 搜索结果条数的合法区间上限（§3.2 search 组注记：结果列表直接回喂模型
/// 上下文，条数过大挤占窗口且对上游引擎压力大；下限 1 由「正整数」承担）。
pub const MAX_SEARCH_RESULTS_LIMIT: u32 = 30;

/// SearXNG 自托管元搜索配置（§3.2 search.searxng 组，FR-05-05/C-12，
/// 13 号票实装）。用户显式启用才参与 web_search 通道 1（本地优先）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearxngConfig {
    /// 是否启用 SearXNG 通道（C-12 优先级①的开关）：缺省 false——未启用时
    /// web_search 直接从通道 2（内置抓取）开始，不自动探测本地服务。
    #[serde(default)]
    pub enabled: bool,
    /// SearXNG 服务基地址（缺省 `http://127.0.0.1:8888`；归一化剥尾部 `/`）。
    /// 请求走本地回环、不经应用代理（C-12 定案：自托管服务无代理必要，
    /// 走代理反而可能被代理规则拦掉回环地址）；也**不经 SSRF 校验**——
    /// 用户显式启用即信任该地址，而回环地址天然被 §9.2 防线拒绝（§9.2
    /// SearXNG 例外注记）。
    #[serde(default = "default_searxng_base_url")]
    pub base_url: String,
    /// 每次搜索返回的结果条数上限（各通道共用缺省：工具未带 limit 参数时
    /// 按本值截断；合法区间 1–30，越界拒绝整份提交）。
    #[serde(default = "default_search_max_results")]
    pub max_results: u32,
}

/// serde 缺省值函数：SearXNG 服务基地址缺省本机 8888 端口（§3.2/C-12）。
fn default_searxng_base_url() -> String {
    "http://127.0.0.1:8888".to_string()
}
/// serde 缺省值函数：搜索结果条数缺省 10（§8.3 web_search 行口径）。
fn default_search_max_results() -> u32 {
    DEFAULT_SEARCH_MAX_RESULTS
}

// SearxngConfig 的缺省值：不启用 + 本机 8888 + 结果条数 10。
impl Default for SearxngConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: default_searxng_base_url(),
            max_results: DEFAULT_SEARCH_MAX_RESULTS,
        }
    }
}

/// 网络搜索配置（§3.2 search 组，FR-05-05/C-12 本地优先三通道，13 号票
/// 实装）：web_search 工具按「SearXNG（启用且探活成功）→ 内置直接抓取
/// （默认兜底）→ Tavily（Key 已配才参与）」的钉死优先级自动选通道。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchConfig {
    /// SearXNG 自托管元搜索（通道 1，用户显式启用）。
    #[serde(default)]
    pub searxng: SearxngConfig,
    /// Tavily API Key（通道 3，BYOK 高级区）：空白归一为 None（"没填"与
    /// "填了又删掉"等价，未配 Key 该通道不参与优先级链）；明文随 config.json
    /// 0600 落盘（v2.5 存储协议，§9.4——票面"钥匙串协议"按现行协议落
    /// config.json，票内决议记录说明）。旧配置无该组时 serde default 回填。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub tavily_api_key: Option<String>,
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
    /// 监督看门狗（FR-03-07，37 号票；缺省值经 Default：停滞超时 300 秒。
    /// 旧配置无该组时 serde default 回填默认值）。
    #[serde(default)]
    pub watchdog: WatchdogConfig,
    /// 网络工具 SSRF 防护（FR-05-07/§9.2，12 号票；缺省值经 Default：
    /// 拒绝私网访问。旧配置无该组时 serde default 回填）。
    #[serde(default)]
    pub network: NetworkConfig,
    /// 网络搜索（FR-05-05/C-12 本地优先三通道，13 号票；缺省值经 Default：
    /// SearXNG 不启用 + Key 未配 = 零配置默认走内置抓取。旧配置无该组时
    /// serde default 回填）。
    #[serde(default)]
    pub search: SearchConfig,
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

// ApprovalConfig 的缺省值：审批开、不自动确认、会话记忆与白名单启用、
// 白名单为空（最保守；23 号票四开关缺省口径）。
impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_confirm: false,
            session_memory_enabled: true,
            allowlist_enabled: true,
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
            watchdog: WatchdogConfig::default(),
            network: NetworkConfig::default(),
            search: SearchConfig::default(),
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
        // tools.batchReadLimit：合法区间 1–100（§3.2 tools.batchReadLimit 注记
        // ——真正的硬顶是 batch_read 的 512KB 共享响应预算，本字段只是文件数
        // 闸门；越界视为配置错误拒绝整份提交。旧配置缺字段由 serde default
        // 回填 20，14 号票）。
        if self.tools.batch_read_limit == 0 || self.tools.batch_read_limit > MAX_BATCH_READ_LIMIT {
            return Err(IpcError::invalid_arg(format!(
                "tools.batchReadLimit 必须在 1–{MAX_BATCH_READ_LIMIT} 内（实际 {}）",
                self.tools.batch_read_limit
            )));
        }
        // watchdog.stallTimeoutSecs：合法区间 5–3600 秒（§3.2 watchdog 组/§5.7——
        // 过短会对慢模型/长首包误杀，过长失去停滞检测意义；越界视为配置错误
        // 拒绝整份提交）。
        if !(MIN_STALL_TIMEOUT_SECS..=MAX_STALL_TIMEOUT_SECS)
            .contains(&self.watchdog.stall_timeout_secs)
        {
            return Err(IpcError::invalid_arg(format!(
                "watchdog.stallTimeoutSecs 必须在 {MIN_STALL_TIMEOUT_SECS}–{MAX_STALL_TIMEOUT_SECS} 秒内（实际 {}）",
                self.watchdog.stall_timeout_secs
            )));
        }
        self.compaction.model_id = self
            .compaction
            .model_id
            .take()
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty());
        // agent.maxConcurrentSubagents：正整数且不超过 16（§5.9 Q13——所有会话
        // 共用的子代理并发上限，越界视为配置错误拒绝整份提交；旧配置缺字段由
        // serde default 回填 4）。
        if let Some(agent) = &self.agent
            && (agent.max_concurrent_subagents == 0
                || agent.max_concurrent_subagents > MAX_CONCURRENT_SUBAGENTS_LIMIT)
        {
            return Err(IpcError::invalid_arg(format!(
                "agent.maxConcurrentSubagents 必须在 1–{MAX_CONCURRENT_SUBAGENTS_LIMIT} 内（实际 {}）",
                agent.max_concurrent_subagents
            )));
        }
        // agent.customPrompt：空白归一为 None（"没填"与"填了又删掉"等价零注入）
        if let Some(agent) = &mut self.agent {
            agent.custom_prompt = agent
                .custom_prompt
                .take()
                .map(|p| p.trim().to_owned())
                .filter(|p| !p.is_empty());
        }
        // search.searxng.baseUrl：必须带主机名的 http(s) 地址（防 file:// 等
        // 非预期协议进入配置）；剥尾部 `/`（Url::join 拼路径时不产生双斜杠）；
        // search.searxng.maxResults：正整数且不超过 30（§3.2 search 组注记——
        // 结果列表直接回喂模型上下文，条数过大挤占窗口；越界拒绝整份提交）；
        // search.tavilyApiKey：空白归一为 None（未配 Key 通道 3 不参与优先级链）。
        self.search.searxng.base_url = self
            .search
            .searxng
            .base_url
            .trim()
            .trim_end_matches('/')
            .to_string();
        ensure_http_url(&self.search.searxng.base_url)
            .map_err(|e| IpcError::invalid_arg(format!("search.searxng.{}（{e}）", "baseUrl")))?;
        if self.search.searxng.max_results == 0
            || self.search.searxng.max_results > MAX_SEARCH_RESULTS_LIMIT
        {
            return Err(IpcError::invalid_arg(format!(
                "search.searxng.maxResults 必须在 1–{MAX_SEARCH_RESULTS_LIMIT} 内（实际 {}）",
                self.search.searxng.max_results
            )));
        }
        self.search.tavily_api_key = self
            .search
            .tavily_api_key
            .take()
            .map(|k| k.trim().to_owned())
            .filter(|k| !k.is_empty());
        // proxy（FR-02-10，19 号票）：manual 模式必须有地址且 scheme 在
        // 白名单（http/https/socks5/socks5h——socks5h = 域名由代理远端解析）；
        // 校验口径与 providers::http::validate_proxy_url 同源（保存前拦截，
        // 防 http.rs 运行时装配路径才报错）。none/system 模式的 url 字段
        // 无意义，不校验（宽容：切换模式时残留旧值不阻塞保存）。
        if let Some(proxy) = &self.proxy
            && proxy.mode == ProxyMode::Manual
        {
            // 缺 url 传空串进校验（报"缺少协议前缀"），与有 url 但非法同一出口
            if let Err(message) =
                crate::providers::http::validate_proxy_url(proxy.url.as_deref().unwrap_or(""))
            {
                return Err(IpcError::invalid_arg(format!("proxy.url（{message}）")));
            }
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
