//! 供应商连接探测（FR-02-09，19 号票）：`test_provider` 命令的领域实现。
//!
//! 不依赖 Tauri（入参全是值类型），单测直接打本模块；`host/commands/
//! provider.rs` 只是参数契约薄壳。探测请求走代理工厂提供的 client
//! （providers/http.rs），与模型请求同一条代理语义。
//!
//! 探测策略（FR-02-09"models 列表拉取或最小补全"两策略皆合法，两段都实现）：
//! 1. `GET {base}/models`（带 Key；本地免 Key 服务该头被忽略）——OpenAI
//!    方言的官方模型清单端点；2xx 且能解析出模型 id 列表即连通；
//! 2. 端点不支持（404/405，常见于精简的 OpenAI 兼容网关）→ 回退**最小补全**：
//!    `POST {base}/chat/completions`，`max_tokens=1` 的单句对话——能收 2xx
//!    即连通（模型列表无从得知，models 字段缺省）。
//!
//! 错误分类复用 §11.3/§3.6 的供应商七类（auth/billing/rate_limited/
//! bad_request/server/protocol/network），错误消息脱敏：只带状态码与响应
//! 体摘要（≤200 字符），永不含请求头（Key 不回显）。

use serde::Deserialize;
use serde::Serialize;
use std::time::{Duration, Instant};

use crate::util::errors::IpcError;

/// 探测总预算（命令面收口；对齐 13 号票 search_test 的 30 秒口径）。
pub const PROBE_BUDGET: Duration = Duration::from_secs(30);

/// 供应商七类错误（FR-03-04/§3.6 E_PROVIDER_* 的分类面）。
/// 序列化为 snake_case，前端据 code 映射人话文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ProbeErrorKind {
    /// 401/403：Key 无效或无权限（§11.3 auth 行）。
    Auth,
    /// 402/配额尽：余额不足（§11.3 billing 行）。
    Billing,
    /// 429：限流（§11.3 rate_limited 行）。
    RateLimited,
    /// 400/404/422 等请求面拒绝：地址/模型参数有误（§11.3 bad_request 行）。
    BadRequest,
    /// 5xx：服务端错误（§11.3 server 行）。
    Server,
    /// 响应不可解析：状态/结构不符合协议预期（§11.3 protocol 行）。
    Protocol,
    /// 连接/超时/DNS：目标不可达（§11.3 network 行）。
    Network,
}

// ProbeErrorKind 的分类码映射（§3.6 错误码注册表 E_PROVIDER_* 键面）。
impl ProbeErrorKind {
    /// §3.6 错误码（E_PROVIDER_ 前缀 + 分类大写）。
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
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ProbeError {
    pub code: String,
    pub message: String,
}

/// 探测结果（§3.3 #32 返回契约的领域面）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    /// 是否连通（models 拉取或最小补全任一策略 2xx 即算）。
    pub ok: bool,
    /// 首响应耗时毫秒（发出请求到收到响应；探测失败时不带）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub latency_ms: Option<u64>,
    /// 模型 wire id 列表（models 拉取成功时；最小补全路径无从得知 → 缺省）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub models: Option<Vec<String>>,
    /// 失败原因（ok=false 时；code 七类 + 人读消息）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
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

/// HTTP 状态码 → 七类（§11.3 表的探测面；404/405 不在此列——它们是
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

/// 把 reqwest 错误归七类：超时/连接/DNS → network（§11.3 network 行）；
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

/// 探测一个 OpenAI 方言端点（FR-02-09）。`base_url` 为供应商基地址
/// （已含 /v1 之类前缀的完整 base；剥尾部 `/` 后拼路径）——草稿试连时
/// 传表单当前值（baseUrlOverride 语义，不落库）；`api_key` 可空（本地
/// 免 Key 服务/未填 Key 的探测：请求不带 Authorization，错误按响应分类）。
pub async fn probe_openai_compatible(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
) -> ProbeResult {
    let started = Instant::now();
    let outcome = probe_inner(client, base_url, api_key, started).await;
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
    started: Instant,
) -> Result<(Duration, Option<Vec<String>>), (ProbeErrorKind, String)> {
    let base = base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err((ProbeErrorKind::BadRequest, "基地址为空".to_string()));
    }
    // ---- 策略 1：GET {base}/models ----
    let mut request = client.get(format!("{base}/models")).timeout(PROBE_BUDGET);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
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
                format!("/models 响应无法解析为模型列表：{}", body_digest(&body)),
            )),
        };
    }
    let digest = body_digest(&body);
    // 404/405 = models 端点不存在（精简兼容网关常见）→ 回退策略 2；
    // 其余状态直接按七类报错（401 不该试补全——Key 都错了，补全也会错）。
    if status.as_u16() != 404 && status.as_u16() != 405 {
        let kind = classify_status(status.as_u16(), &body);
        return Err((kind, format!("{base}/models 返回 {status}：{digest}")));
    }
    // ---- 策略 2：最小补全（max_tokens=1 的 chat 请求）----
    minimal_completion(client, base, api_key)
        .await
        .map(|latency| (latency, None))
        .map_err(|(kind, detail)| match kind {
            // 补全端点的 404/405：两条探测路径都不支持 → bad_request
            //（地址不像是 OpenAI 兼容端点）
            ProbeErrorKind::BadRequest => (
                ProbeErrorKind::BadRequest,
                format!("{base} 不支持 /models 与 /chat/completions（确认是 OpenAI 兼容端点且地址含 /v1 前缀）"),
            ),
            other => (other, detail),
        })
}

/// 最小补全探测：POST {base}/chat/completions，单句 + max_tokens=1。
async fn minimal_completion(
    client: &reqwest::Client,
    base: &str,
    api_key: Option<&str>,
) -> Result<Duration, (ProbeErrorKind, String)> {
    let payload = serde_json::json!({
        "model": "craftwave-connection-test",
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 1,
        "stream": false,
    });
    let mut request = client
        .post(format!("{base}/chat/completions"))
        .timeout(PROBE_BUDGET)
        .json(&payload);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
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
            "{base}/chat/completions 返回 {status}：{}",
            body_digest(&body)
        ),
    ))
}

/// 领域错误 → IPC 面（命令层薄壳用）：探测本身不 reject（结果承载在
/// ProbeResult），只有参数面错误（方言不支持等）走 IpcError。
pub fn unsupported_format(api_format: &str) -> IpcError {
    IpcError::invalid_arg(format!(
        "连接测试暂只支持 openai_chat 方言（当前 {api_format}）；anthropic_messages / openai_responses 由三协议接入票接通"
    ))
}

#[cfg(test)]
#[path = "probe_test.rs"]
mod probe_test;
