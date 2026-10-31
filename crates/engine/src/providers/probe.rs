//! 供应商连接探测（建面；扩为三方言）：
//! `test_provider` 命令的领域实现。
//!
//! 不依赖 Tauri（入参全是值类型），单测直接打本模块；`host/commands/
//! provider.rs` 只是参数契约薄壳。探测请求走代理工厂提供的 client
//! （providers/http.rs），与模型请求同一条代理语义。
//!
//! 探测策略（"models 列表拉取或最小补全"两策略皆合法，三方言
//! 都按两段走）：
//! 1. **models 清单拉取**——OpenAI 系 `GET {base}/models`（Bearer），
//!    Anthropic `GET {base}/v1/models`（`x-api-key` + `anthropic-version`，
//!    两种鉴权头都是各自方言对话请求的同款）；2xx 且能解析出模型 id 列表
//!    即连通；
//! 2. 端点不支持（404/405，常见于精简的兼容网关）→ 回退**最小补全**——
//!    openai_chat `POST {base}/chat/completions`（max_tokens=1）、
//!    openai_responses `POST {base}/responses`（max_output_tokens=16，
//!    Responses API 的最小合法值）、anthropic_messages
//!    `POST {base}/v1/messages`（max_tokens=1）——能收 2xx 即连通
//!    （模型列表无从得知，models 字段缺省）。
//!
//! 错误分类复用 的供应商七类（auth/billing/rate_limited/
//! bad_request/server/protocol/network），错误消息脱敏：只带状态码与响应
//! 体摘要（≤200 字符），永不含请求头（Key 不回显）。

use serde::Deserialize;
use serde::Serialize;
use std::time::{Duration, Instant};

use super::types::ApiFormat;

/// 探测总预算（命令面收口；对齐 search_test 的 30 秒口径）。
pub const PROBE_BUDGET: Duration = Duration::from_secs(30);

/// 供应商七类错误（ E_PROVIDER_* 的分类面）。
/// 序列化为 snake_case，前端据 code 映射人话文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ProbeErrorKind {
    /// 401/403：Key 无效或无权限（auth 行）。
    Auth,
    /// 402/配额尽：余额不足（billing 行）。
    Billing,
    /// 429：限流（rate_limited 行）。
    RateLimited,
    /// 400/404/422 等请求面拒绝：地址/模型参数有误（bad_request 行）。
    BadRequest,
    /// 5xx：服务端错误（server 行）。
    Server,
    /// 响应不可解析：状态/结构不符合协议预期（protocol 行）。
    Protocol,
    /// 连接/超时/DNS：目标不可达（network 行）。
    Network,
}

// ProbeErrorKind 的分类码映射（错误码注册表 E_PROVIDER_* 键面）。
impl ProbeErrorKind {
    /// 错误码（E_PROVIDER_ 前缀 + 分类大写）。
    pub fn code(self) -> &'static str {
        match self {
            Self::Auth => "E_PROVIDER_AUTH",
            Self::Billing => "E_PROVIDER_BILLING",
            Self::RateLimited => "E_PROVIDER_RATE_LIMITED",
            Self::BadRequest => "E_PROVIDER_BAD_REQUEST",
            Self::Server => "E_PROVIDER_SERVER",
            Self::Protocol => "E_PROVIDER_PROTOCOL",
            Self::Network => "E_PROVIDER_NETWORK",
        }
    }
}

/// 探测失败的结构化错误（脱敏后）：code = 七类之一，message 人读。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ProbeError {
    pub code: String,
    pub message: String,
}

/// 探测结果（返回契约的领域面）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    /// 是否连通（models 拉取或最小补全任一策略 2xx 即算）。
    pub ok: bool,
    /// 首响应耗时毫秒（发出请求到收到响应；探测失败时不带）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub latency_ms: Option<u64>,
    /// 模型 wire id 列表（models 拉取成功时；最小补全路径无从得知 → 缺省）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub models: Option<Vec<String>>,
    /// 失败原因（ok=false 时；code 七类 + 人读消息）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<ProbeError>,
}

// ---- 探测实现 ----

/// /models 响应的宽松解析形状：OpenAI 官方为 `{"data":[{"id":...}]}`；
/// 部分兼容网关直接给 `{"models":[...]}` 或裸数组 `[{"id":...}]`。
/// 按"能挖出字符串 id 列表"的口径宽松取，解析不出视为不支持。
#[derive(Deserialize)]
struct ModelsShape {
    #[serde(default)]
    data: Option<Vec<ModelIdShape>>,
    #[serde(default)]
    models: Option<Vec<ModelIdShape>>,
}

#[derive(Deserialize)]
struct ModelIdShape {
    id: Option<serde_json::Value>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

/// 从 /models 响应体挖模型 id 列表（宽松三形状；挖不出 → None）。
fn parse_models(body: &str) -> Option<Vec<String>> {
    let ids = |items: Vec<ModelIdShape>| -> Vec<String> {
        items
            .into_iter()
            .filter_map(|m| {
                if let Some(id) = m.id {
                    Some(match id {
                        serde_json::Value::String(s) => s,
                        other => other.to_string(),
                    })
                } else {
                    m.model.or(m.name)
                }
            })
            .collect()
    };
    // 形状 1/2：对象包 data[] 或 models[]（Ollama /v1/models 是 OpenAI 的 data 形状）
    if let Ok(shape) = serde_json::from_str::<ModelsShape>(body) {
        if let Some(data) = shape.data {
            return Some(ids(data));
        }
        if let Some(models) = shape.models {
            return Some(ids(models));
        }
    }
    // 形状 3：裸数组
    if let Ok(items) = serde_json::from_str::<Vec<ModelIdShape>>(body) {
        return Some(ids(items));
    }
    None
}

/// HTTP 状态码 → 七类（表的探测面；404/405 不在此列——它们是
/// "models 端点不支持"的回退信号，由调用方单独识别）。
fn classify_status(status: u16, body: &str) -> ProbeErrorKind {
    match status {
        401 | 403 => ProbeErrorKind::Auth,
        402 => ProbeErrorKind::Billing,
        429 => ProbeErrorKind::RateLimited,
        400 | 404 | 405 | 422 => ProbeErrorKind::BadRequest,
        s if (500..600).contains(&s) => ProbeErrorKind::Server,
        // 其余非 2xx（3xx 意外重定向、1xx 等）：协议面不符
        _ => {
            let _ = body;
            ProbeErrorKind::Protocol
        }
    }
}

/// 响应体摘要（错误消息脱敏面）：截断到 200 字符，控制消息体积。
fn body_digest(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= 200 {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(200).collect();
    format!("{cut}…（已截断）")
}

/// 把 reqwest 错误归七类：超时/连接/DNS → network（network 行）；
/// reqwest 错误链里没有状态概念，全部按不可达处理（探测不走 SSRF
/// resolver，无该侧错误源）。
fn classify_reqwest(err: &reqwest::Error) -> ProbeErrorKind {
    if err.is_timeout() || err.is_connect() {
        ProbeErrorKind::Network
    } else {
        // 请求构造/编码等其他 transport 错误：网络面兜底
        ProbeErrorKind::Network
    }
}

/// 按方言探测（分发入口）：连接测试与实际对话走同一
/// `ApiFormat` 选择（连接测试与实际对话同口径——不出现"测试成功却因固定
/// openai_chat 限制无法聊天"）。`base_url` 为供应商基地址（草稿试连时传表单当前值）；
/// `api_key` 可空（本地免 Key 服务/未填 Key 的探测：请求不带鉴权头，错误
/// 按响应分类）。
pub async fn probe(
    client: &reqwest::Client,
    api_format: ApiFormat,
    base_url: &str,
    api_key: Option<&str>,
) -> ProbeResult {
    match api_format {
        ApiFormat::OpenaiChat => probe_openai_compatible(client, base_url, api_key).await,
        ApiFormat::OpenaiResponses => probe_responses_api(client, base_url, api_key).await,
        ApiFormat::AnthropicMessages => probe_anthropic_messages(client, base_url, api_key).await,
    }
}

/// 探测一个 OpenAI 方言端点。`base_url` 为供应商基地址
/// （已含 /v1 之类前缀的完整 base；剥尾部 `/` 后拼路径）——草稿试连时
/// 传表单当前值（baseUrlOverride 语义，不落库）；`api_key` 可空（本地
/// 免 Key 服务/未填 Key 的探测：请求不带 Authorization，错误按响应分类）。
pub async fn probe_openai_compatible(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> ProbeResult {
    probe_two_stage(client, base_url, api_key, openai_chat_endpoints()).await
}

/// 探测一个 OpenAI Responses 方言端点：`GET {base}/models`
/// 与 chat 方言同端点同鉴权（Bearer）；回退段打 `POST {base}/responses`
/// （Responses API 的参数名是 `max_output_tokens` 且最小 16）。
pub async fn probe_responses_api(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> ProbeResult {
    probe_two_stage(client, base_url, api_key, openai_responses_endpoints()).await
}

/// 探测一个 Anthropic Messages 方言端点：地址先按 rig
/// `normalize_base_url` 同款规则剥 `/v1`、`/messages`、`/v1/messages` 尾缀
/// （用户常把端点路径一并粘进 baseUrl），再拼官方路径 `/v1/models` 与
/// `/v1/messages`；鉴权头是 `x-api-key` + `anthropic-version`（对话请求
/// 同款，与 OpenAI 系的 Bearer 不同）。
pub async fn probe_anthropic_messages(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> ProbeResult {
    let base = normalize_anthropic_base(base_url);
    probe_two_stage(client, &base, api_key, ANTHROPIC_ENDPOINTS).await
}

/// Anthropic 基地址归一（剥尾 `/` 与端点路径后缀；对齐 rig
/// `anthropic::wire::normalize_base_url` 的口径）。
fn normalize_anthropic_base(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    for suffix in ["/v1/messages", "/messages", "/v1"] {
        if let Some(stripped) = trimmed.strip_suffix(suffix) {
            return stripped.to_owned();
        }
    }
    trimmed.to_owned()
}

/// 两段探测的端点描述（三方言共用的策略骨架在此，方言差异只落在
/// 端点路径、鉴权头与最小补全体三处）。
#[derive(Clone, Copy)]
struct Endpoints {
    /// models 清单路径（相对 base）。
    models_path: &'static str,
    /// 最小补全路径（相对 base）。
    completion_path: &'static str,
    /// 鉴权头装配：None = Bearer（OpenAI 系），Some = x-api-key +
    /// anthropic-version（Anthropic 系）。
    anthropic_auth: bool,
    /// 最小补全请求体装配。
    completion_body: fn() -> serde_json::Value,
}

/// OpenAI 系端点组（Bearer 鉴权）：chat 与 responses 两路线只在补全路径
/// 与最小补全体上不同（Responses API 的参数名与下限不同）。
fn openai_chat_endpoints() -> Endpoints {
    Endpoints {
        models_path: "/models",
        completion_path: "/chat/completions",
        anthropic_auth: false,
        completion_body: || {
            serde_json::json!({
                "model": "craftwave-connection-test",
                "messages": [{"role": "user", "content": "ping"}],
                "max_tokens": 1,
                "stream": false,
            })
        },
    }
}

/// OpenAI Responses 端点组（Bearer 鉴权；回退段打 /responses，
/// max_output_tokens=16 为 Responses API 的最小合法输出上限）。
fn openai_responses_endpoints() -> Endpoints {
    Endpoints {
        models_path: "/models",
        completion_path: "/responses",
        anthropic_auth: false,
        completion_body: || {
            serde_json::json!({
                "model": "craftwave-connection-test",
                "input": "ping",
                "max_output_tokens": 16,
                "stream": false,
            })
        },
    }
}

/// Anthropic Messages 端点组（x-api-key + anthropic-version 鉴权）。
const ANTHROPIC_ENDPOINTS: Endpoints = Endpoints {
    models_path: "/v1/models",
    completion_path: "/v1/messages",
    anthropic_auth: true,
    // 常量上下文装不下 json! 宏，运行时函数体兜住
    completion_body: anthropic_probe_body,
};

/// Anthropic 最小补全体（max_tokens=1 的单句请求）。
fn anthropic_probe_body() -> serde_json::Value {
    serde_json::json!({
        "model": "craftwave-connection-test",
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 1,
        "stream": false,
    })
}

/// Anthropic Messages API 的 `anthropic-version` 头取值（对齐 rig
/// `ANTHROPIC_VERSION_LATEST`）。
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// 探测一个方言端点（两段策略 + 七类错误归面的共享骨架）。返回
/// Ok(首响应耗时, 模型列表?)。
async fn probe_two_stage(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    endpoints: Endpoints,
) -> ProbeResult {
    let started = Instant::now();
    let outcome = probe_inner(client, base_url, api_key, endpoints, started).await;
    match outcome {
        Ok((latency_ms, models)) => ProbeResult {
            ok: true,
            latency_ms: Some(latency_ms.as_millis() as u64),
            models,
            error: None,
        },
        Err((kind, detail)) => ProbeResult {
            ok: false,
            latency_ms: Some(started.elapsed().as_millis() as u64),
            models: None,
            error: Some(ProbeError {
                code: kind.code().to_string(),
                message: detail,
            }),
        },
    }
}

/// 探测内部：两段策略 + 七类错误归面。返回 Ok(首响应耗时, 模型列表?)。
/// `started` 由外层传入（延迟计量以探测起点为基准，两段策略共享）。
async fn probe_inner(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    endpoints: Endpoints,
    started: Instant,
) -> Result<(Duration, Option<Vec<String>>), (ProbeErrorKind, String)> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err((ProbeErrorKind::BadRequest, "基地址为空".to_string()));
    }
    // ---- 策略 1：models 清单拉取（鉴权头按方言装配）----
    let mut request = client
        .get(format!("{base}{}", endpoints.models_path))
        .timeout(PROBE_BUDGET);
    request = apply_auth(request, api_key, endpoints.anthropic_auth);
    let response = request
        .send()
        .await
        .map_err(|e| (classify_reqwest(&e), format!("无法连接（{base}）：{e}")))?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if status.is_success() {
        // 2xx 但解析不出模型列表：端点在但形状不符（协议面）
        return match parse_models(&body) {
            Some(models) => Ok((started.elapsed(), Some(models))),
            None => Err((
                ProbeErrorKind::Protocol,
                format!(
                    "{} 响应无法解析为模型列表：{}",
                    endpoints.models_path,
                    body_digest(&body)
                ),
            )),
        };
    }
    let digest = body_digest(&body);
    // 404/405 = models 端点不存在（精简兼容网关常见）→ 回退策略 2；
    // 其余状态直接按七类报错（401 不该试补全——Key 都错了，补全也会错）。
    if status.as_u16() != 404 && status.as_u16() != 405 {
        let kind = classify_status(status.as_u16(), &body);
        return Err((
            kind,
            format!("{base}{} 返回 {status}：{digest}", endpoints.models_path),
        ));
    }
    // ---- 策略 2：最小补全（方言各自的补全端点与最小请求体）----
    minimal_completion(client, base, api_key, endpoints)
        .await
        .map(|latency| (latency, None))
        .map_err(|(kind, detail)| match kind {
            // 补全端点的 404/405：两条探测路径都不支持 → bad_request
            //（地址不像该方言的端点）
            ProbeErrorKind::BadRequest => (
                ProbeErrorKind::BadRequest,
                format!(
                    "{base} 不支持 {} 与 {}（确认地址是 {kind_label}）",
                    endpoints.models_path,
                    endpoints.completion_path,
                    kind_label = if endpoints.anthropic_auth {
                        "Anthropic Messages 兼容端点"
                    } else {
                        "OpenAI 兼容端点（chat 路径需含 /v1 前缀）"
                    }
                ),
            ),
            other => (other, detail),
        })
}

/// 按方言装配鉴权头：OpenAI 系 `Authorization: Bearer <key>`；Anthropic 系
/// `x-api-key` + `anthropic-version`（rig wire 同款请求头）。
fn apply_auth(
    request: reqwest::RequestBuilder,
    api_key: Option<&str>,
    anthropic_auth: bool,
) -> reqwest::RequestBuilder {
    match (api_key, anthropic_auth) {
        (Some(key), false) => request.bearer_auth(key),
        (Some(key), true) => request
            .header("x-api-key", key)
            .header("anthropic-version", ANTHROPIC_VERSION),
        // 本地免 Key / 未填 Key：不带鉴权头（服务端按响应分类）
        (None, _) => request,
    }
}

/// 最小补全探测：POST 方言补全端点，单句 + 最小输出上限。
async fn minimal_completion(
    client: &reqwest::Client,
    base: &str,
    api_key: Option<&str>,
    endpoints: Endpoints,
) -> Result<Duration, (ProbeErrorKind, String)> {
    let mut request = client
        .post(format!("{base}{}", endpoints.completion_path))
        .timeout(PROBE_BUDGET)
        .json(&(endpoints.completion_body)());
    request = apply_auth(request, api_key, endpoints.anthropic_auth);
    let started = Instant::now();
    let response = request
        .send()
        .await
        .map_err(|e| (classify_reqwest(&e), format!("无法连接（{base}）：{e}")))?;
    let status = response.status();
    // 响应体读完（错误摘要要用；成功路径也排干连接便于复用）
    let body = response.text().await.unwrap_or_default();
    if status.is_success() {
        return Ok(started.elapsed());
    }
    Err((
        classify_status(status.as_u16(), &body),
        format!(
            "{base}{} 返回 {status}：{}",
            endpoints.completion_path,
            body_digest(&body)
        ),
    ))
}

#[cfg(test)]
#[path = "probe_test.rs"]
mod probe_test;
