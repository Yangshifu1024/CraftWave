//! http_request 工具：通用 HTTP 请求（模型调试 API / 拉取资源用）。
//!
//! - 方法限 GET/POST/PUT/PATCH/DELETE/HEAD；
//! - SSRF 五步防线与节流在 [`crate::safety::ssrf`]，重定向逐跳校验
//!   与跨源剥敏感头同 web_fetch；
//! - 响应治理同命令输出：>96KB 全量 spill 至
//!   `<data>/logs/spill/<callId>.txt` + 尾部 16KB + spill 路径（24h lazy
//!   清理，复用 tools/command 的 spill 原语）；
//! - 结果 data=`{ status, headers(截断), body }`；
//! - 能力 Network：plan 档过滤、confirm_each/auto_edit 档审批、
//!   full_access 放行。

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use crate::safety::ssrf::{FetchRequest, HostThrottle, SsrfPolicy, build_client, fetch};
use crate::util::errors::ToolError;

use super::{ToolAnnotations, ToolOrigin, ToolOutcome, ToolSpec};

/// 允许的 HTTP 方法白名单（method 限 GET/POST/PUT/PATCH/DELETE/HEAD）。
pub const ALLOWED_METHODS: [&str; 6] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"];

/// 单个响应头的值保留上限（防巨型头挤爆工具结果；头总数一般 <100）。
const HEADER_VALUE_LIMIT: usize = 512;

/// http_request 参数：`{ method, url, headers?, body? }`。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpRequestArgs {
    /// HTTP 方法（白名单内，大小写不敏感）。
    pub method: String,
    /// 目标 URL（仅 http/https；SSRF 校验见 safety/ssrf）。
    pub url: String,
    /// 自定义请求头（跨源重定向时敏感头自动剥除）。
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    /// 请求体（文本；GET/HEAD 忽略）。
    #[serde(default)]
    pub body: Option<String>,
}

/// 取消可感知的执行退出态（executor 映射 ToolExecution，同 command）。
pub enum RequestExit {
    /// 正常结束（含各类工具级失败，结果在 ToolOutcome 里）。
    Done(Box<ToolOutcome>),
    /// 取消：`executed` 表示请求是否已到达对端（POST 等可能已产生服务端
    /// 副作用——已发出即按"已执行"口径报告，不回滚）。
    Cancelled { executed: bool },
}

/// 执行一次 HTTP 请求（门控放行后调用）。`call_id` 为 spill 文件名；
/// `ctx` 提供数据根目录（spill 落 `<data>/logs/spill/`）。
pub async fn execute(
    ctx: &super::ToolsContext,
    policy: SsrfPolicy,
    proxy: Option<&crate::providers::types::ProxyConfig>,
    throttle: &HostThrottle,
    call_id: &str,
    args: &HttpRequestArgs,
    token: &CancellationToken,
) -> RequestExit {
    // 参数预检：方法白名单 + URL 语法
    let method_name = args.method.trim().to_ascii_uppercase();
    if !ALLOWED_METHODS.contains(&method_name.as_str()) {
        return RequestExit::Done(Box::new(ToolOutcome::failure(
            ToolError::invalid_arg(format!(
                "method 仅允许 {}（实际 {method_name}）",
                ALLOWED_METHODS.join("/")
            )),
            "HTTP 方法不支持",
        )));
    }
    let url = match url::Url::parse(args.url.trim()) {
        Ok(u) => u,
        Err(e) => {
            return RequestExit::Done(Box::new(ToolOutcome::failure(
                ToolError::invalid_arg(format!("url 不是合法地址（{}）：{e}", args.url)),
                "URL 解析失败",
            )));
        }
    };
    let method = match reqwest::Method::from_bytes(method_name.as_bytes()) {
        Ok(m) => m,
        Err(e) => {
            return RequestExit::Done(Box::new(ToolOutcome::failure(
                ToolError::invalid_arg(format!("method 解析失败：{e}")),
                "HTTP 方法不支持",
            )));
        }
    };
    // 自定义请求头（非法头名/值在构造边界拒绝，不发半截请求）
    let mut headers = reqwest::header::HeaderMap::new();
    for (name, value) in args.headers.iter().flatten() {
        let header_name = match reqwest::header::HeaderName::from_bytes(name.as_bytes()) {
            Ok(n) => n,
            Err(e) => {
                return RequestExit::Done(Box::new(ToolOutcome::failure(
                    ToolError::invalid_arg(format!("请求头名非法（{name}）：{e}")),
                    "请求头非法",
                )));
            }
        };
        let header_value = match reqwest::header::HeaderValue::from_str(value) {
            Ok(v) => v,
            Err(e) => {
                return RequestExit::Done(Box::new(ToolOutcome::failure(
                    ToolError::invalid_arg(format!("请求头值非法（{name}）：{e}")),
                    "请求头非法",
                )));
            }
        };
        headers.insert(header_name, header_value);
    }
    // GET/HEAD 不带 body（HTTP 语义；reqwest 对这类方法带 body 会报错）
    let body = match method {
        reqwest::Method::GET | reqwest::Method::HEAD => None,
        _ => args.body.clone().map(String::into_bytes),
    };

    let client = match build_client(policy, proxy) {
        Ok(c) => c,
        Err(err) => return RequestExit::Done(Box::new(ToolOutcome::from(err))),
    };

    let request = FetchRequest {
        url,
        method,
        headers,
        body,
    };
    let sent = AtomicBool::new(false);
    let response = tokio::select! {
        biased;
        _ = token.cancelled() => {
            return RequestExit::Cancelled { executed: false };
        }
        result = fetch(&client, request, policy, throttle, token, &sent) => {
            match result {
                Ok(r) => r,
                Err(err) => {
                    // 用户取消从 fetch 内部冒出（节流等待/下载中被取消）：
                    // 按 Cancelled 出口收口，executed 以 sent 为准
                    if err.code == "E_CANCELLED" {
                        return RequestExit::Cancelled {
                            executed: sent.load(std::sync::atomic::Ordering::Relaxed),
                        };
                    }
                    return RequestExit::Done(Box::new(ToolOutcome::from(err)));
                }
            }
        }
    };

    // 响应头截断（每值 ≤512 字节，超长截断标注）
    let mut headers_json = serde_json::Map::new();
    for (name, value) in &response.headers {
        let key = name.as_str().to_owned();
        let raw = String::from_utf8_lossy(value.as_bytes()).into_owned();
        let display = if raw.len() > HEADER_VALUE_LIMIT {
            format!("{}…[截断]", &raw[..HEADER_VALUE_LIMIT])
        } else {
            raw
        };
        headers_json.insert(key, serde_json::Value::String(display));
    }

    // 响应体：文本化（lossy）+ spill 治理（>96KB spill + 尾 16KB）
    let body_text = String::from_utf8_lossy(&response.body).into_owned();
    let mut data = serde_json::json!({
        "status": response.status,
        "url": response.url.as_str(),
        "headers": headers_json,
    });
    let mut ui_summary = format!(
        "HTTP {}（{} 字节响应体）",
        response.status,
        response.body.len()
    );

    if body_text.len() > super::command::SPILL_THRESHOLD {
        // spill 情形：全量落盘 + body 只带尾部 16KB + spill 路径
        match super::command::spill_full(ctx, call_id, &body_text) {
            Ok(path) => {
                data["spillPath"] = serde_json::Value::String(path.clone());
                data["body"] = serde_json::Value::String(
                    super::command::tail_of(&body_text, super::command::TAIL_BYTES).to_owned(),
                );
                data["bodyTruncated"] = serde_json::Value::Bool(true);
                ui_summary.push_str(&format!("（全量已落盘 {path}）"));
            }
            Err(_) => {
                // spill 失败不阻断：结果仍带尾部
                data["body"] = serde_json::Value::String(
                    super::command::tail_of(&body_text, super::command::TAIL_BYTES).to_owned(),
                );
                data["bodyTruncated"] = serde_json::Value::Bool(true);
                ui_summary.push_str("（全量落盘失败，仅尾部）");
            }
        }
    } else {
        data["body"] = serde_json::Value::String(body_text);
        data["bodyTruncated"] = serde_json::Value::Bool(false);
    }

    RequestExit::Done(Box::new(ToolOutcome::success(data, ui_summary)))
}

/// http_request 工具规格（能力 Network：plan 档过滤、非 full_access 档审批）。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "http_request".to_string(),
        description: "发送通用 HTTP 请求并返回响应（状态码、响应头、响应体）。\
仅支持 http/https 与 GET/POST/PUT/PATCH/DELETE/HEAD；内网与回环地址会被 SSRF 防护拒绝；\
重定向逐跳校验且 HTTPS 不降级。响应体超过 96KB 时全量落盘、只返回尾部 16KB 与文件路径。"
            .to_string(),
        capability: crate::safety::tier::Capability::Network,
        params_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "method": {
                    "type": "string",
                    "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"],
                    "description": "HTTP 方法"
                },
                "url": { "type": "string", "description": "目标地址（http/https）" },
                "headers": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": "自定义请求头（跨源重定向时 Authorization/Cookie 等敏感头自动剥除）"
                },
                "body": { "type": "string", "description": "请求体文本（GET/HEAD 忽略）" }
            },
            "required": ["method", "url"],
            "additionalProperties": false
        }),
        annotations: ToolAnnotations {
            read_only_hint: false,
            destructive_hint: false,
            idempotent_hint: false,
            open_world_hint: true,
        },
        origin: ToolOrigin::Static,
    }
}
