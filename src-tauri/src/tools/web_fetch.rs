//! web_fetch 工具（技术设计文档 §8.3 M2 表 web_fetch 行，12 号票）：
//! 抓取网页并提取正文回喂模型。
//!
//! - 两种输出模式：`readability`（默认）= dom_smoothie 正文提取
//!   （Mozilla Readability 算法移植，输出 Markdown，选型理由见票内
//!   决议记录）；`raw` = 清洗后的原始 HTML（compact 口径头 4KB + 尾 8KB）；
//! - SSRF 五步防线与节流在 [`crate::safety::ssrf`]（§9.2），本文件只组装；
//! - 正文 ≤50MiB（Content-Length 预检 + 流式计数双保险）；
//! - 能力 Network：plan 档过滤、confirm_each/auto_edit 档审批、
//!   full_access 放行（§8.5，gate.rs 零改动）。
//!
//! 已知取舍（票面边界）：无 headless 浏览器，JS 渲染的页面只能拿到
//! raw HTML 骨架；正文提取质量以静态 HTML 为限。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::safety::ssrf::{
    FetchRequest, HostThrottle, SsrfPolicy, fetch, build_client,
};
use crate::util::errors::ToolError;

use super::ToolAnnotations;
use super::ToolOutcome;
use super::ToolOrigin;
use super::ToolSpec;

/// readability 模式正文的额外保护上限（1MiB）：正常文章在几 KB～几十 KB
/// 量级；异常超长时先本地截断，避免 50MiB 级正文整个序列化进 data 再被
/// compact 丢弃（模型通道反正只见头 4KB + 尾 8KB，§8.2）。
pub const READABILITY_CONTENT_LIMIT: usize = 1024 * 1024;

/// web_fetch 参数（§8.3：`{ url, format?: "readability"|"raw" }`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebFetchArgs {
    /// 目标 URL（仅 http/https；SSRF 校验见 §9.2）。
    pub url: String,
    /// 输出模式：缺省 readability。
    #[serde(default)]
    pub format: Option<WebFetchFormat>,
}

/// 输出模式（serde 小写对齐票面枚举值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebFetchFormat {
    /// 正文提取（dom_smoothie，Markdown 输出）。
    Readability,
    /// 原始 HTML（清洗控制字符，compact 截断）。
    Raw,
}

/// 取消可感知的执行退出态（executor 映射 ToolExecution，同 command）。
pub enum FetchExit {
    /// 正常结束（含各类工具级失败，结果在 ToolOutcome 里）。
    Done(Box<ToolOutcome>),
    /// 取消：`executed` 表示请求是否已到达对端（GET 只读，重试无害，
    /// 但已发出的请求按"已执行"口径报告）。
    Cancelled { executed: bool },
}

/// 执行一次网页抓取（门控放行后调用）。`call_id` 供未来 spill 复用
/// （web_fetch 的结果经 compact/1MiB 截断，不 spill）。
pub async fn execute(
    policy: SsrfPolicy,
    proxy: Option<&crate::providers::types::ProxyConfig>,
    throttle: &HostThrottle,
    _call_id: &str,
    args: &WebFetchArgs,
    token: &CancellationToken,
) -> FetchExit {
    // 参数预检放最前：URL 语法错误不进入网络层
    let url = match Url::parse(args.url.trim()) {
        Ok(u) => u,
        Err(e) => {
            return FetchExit::Done(Box::new(ToolOutcome::failure(
                ToolError::invalid_arg(format!("url 不是合法地址（{}）：{e}", args.url)),
                "URL 解析失败",
            )));
        }
    };
    let format = args.format.unwrap_or(WebFetchFormat::Readability);

    let client = match build_client(policy, proxy) {
        Ok(c) => c,
        Err(err) => return FetchExit::Done(Box::new(ToolOutcome::from(err))),
    };

    let request = FetchRequest {
        url,
        method: reqwest::Method::GET,
        headers: Default::default(),
        body: None,
    };
    let sent = AtomicBool::new(false);
    let response = tokio::select! {
        biased;
        _ = token.cancelled() => {
            return FetchExit::Cancelled { executed: false };
        }
        result = fetch(&client, request, policy, throttle, token, &sent) => {
            match result {
                Ok(r) => r,
                Err(err) => return FetchExit::Done(Box::new(ToolOutcome::from(err))),
            }
        }
    };

    let status = response.status;
    // 非 2xx 一律失败回喂（3xx 已在 fetch 循环内逐跳处理；此处是终态码）
    if !(200..300).contains(&status) {
        return FetchExit::Done(Box::new(ToolOutcome::failure(
            ToolError::new(
                "E_INTERNAL",
                format!("目标返回非成功状态码 {status}（{}）", response.url),
            ),
            format!("抓取失败（HTTP {status}）"),
        )));
    }

    let html = String::from_utf8_lossy(&response.body).into_owned();
    match format {
        WebFetchFormat::Raw => {
            // 清洗：去控制字符（NUL 等），保留原始 HTML 结构
            let cleaned: String = html.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect();
            let truncated = cleaned.len() > super::COMPACT_HEAD_BYTES + super::COMPACT_TAIL_BYTES;
            let content = super::truncate_middle(&cleaned, super::COMPACT_HEAD_BYTES, super::COMPACT_TAIL_BYTES);
            FetchExit::Done(Box::new(ToolOutcome::success(
                serde_json::json!({
                    "url": response.url.as_str(),
                    "format": "raw",
                    "content": content,
                    "length": cleaned.chars().count(),
                    "truncated": truncated,
                }),
                format!("已抓取网页（raw，HTTP {status}）"),
            )))
        }
        WebFetchFormat::Readability => {
            // CPU 密集（DOM 解析）放 blocking 线程池；Article 非 Send，
            // 在闭包内转成纯数据元组再返回
            let document_url = response.url.as_str().to_owned();
            let parse = move || extract_readability(&html, &document_url);
            let extracted = match tokio::task::spawn_blocking(parse).await {
                Ok(Ok(result)) => result,
                Ok(Err(message)) => {
                    // 提取失败（非正文页/超元素上限）：降级建议 raw 模式
                    return FetchExit::Done(Box::new(ToolOutcome::failure(
                        ToolError::new(
                            "E_INTERNAL",
                            format!("正文提取失败：{message}；可改用 format=raw 获取原始 HTML"),
                        ),
                        "正文提取失败",
                    )));
                }
                Err(join_err) => {
                    return FetchExit::Done(Box::new(ToolOutcome::failure(
                        ToolError::new("E_INTERNAL", format!("正文提取任务异常：{join_err}")),
                        "正文提取任务异常",
                    )));
                }
            };
            let (title, content, full_length) = extracted;
            let truncated = content.len() > READABILITY_CONTENT_LIMIT;
            let content = if truncated {
                super::truncate_middle(&content, super::COMPACT_HEAD_BYTES, super::COMPACT_TAIL_BYTES)
            } else {
                content
            };
            FetchExit::Done(Box::new(ToolOutcome::success(
                serde_json::json!({
                    "url": response.url.as_str(),
                    "title": title,
                    "content": content,
                    "length": full_length,
                    "truncated": truncated,
                }),
                format!("已抓取「{title}」（{full_length} 字符，HTTP {status}）"),
            )))
        }
    }
}

/// readability 提取（dom_smoothie，TextMode::Markdown 输出）。
/// 返回（标题, Markdown 正文, 正文字符数）；提取失败返回人话原因。
/// `pub(crate)`：web_fetch_test 快照测试直接驱动本函数。
pub(crate) fn extract_readability(
    html: &str,
    document_url: &str,
) -> Result<(String, String, usize), String> {
    let config = dom_smoothie::Config {
        text_mode: dom_smoothie::TextMode::Markdown,
        ..dom_smoothie::Config::default()
    };
    let mut readability = dom_smoothie::Readability::new(html, Some(document_url), Some(config))
        .map_err(|e| format!("文档解析失败：{e}"))?;
    let article = readability.parse().map_err(|e| format!("正文提取失败：{e}"))?;
    let content = article.content.to_string();
    let length = content.chars().count();
    Ok((article.title, content, length))
}

/// web_fetch 工具规格（能力 Network：plan 档过滤、非 full_access 档审批）。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "web_fetch".to_string(),
        description: "抓取网页并提取正文（Mozilla Readability 算法，输出 Markdown）。\
仅支持 http/https；内网与回环地址会被 SSRF 防护拒绝。format=raw 返回清洗后的原始 HTML \
（头部 4KB + 尾部 8KB 截断）。单次正文上限 50MiB。"
            .to_string(),
        capability: crate::safety::tier::Capability::Network,
        params_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "目标网页地址（http/https）" },
                "format": {
                    "type": "string",
                    "enum": ["readability", "raw"],
                    "description": "输出模式：readability=正文 Markdown（默认）；raw=清洗后的原始 HTML"
                }
            },
            "required": ["url"],
            "additionalProperties": false
        }),
        annotations: ToolAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: true,
        },
        origin: ToolOrigin::Static,
    }
}

/// 测试与执行器共用的环境句柄别名（Arc 化的节流器由装配方持有）。
pub type SharedThrottle = Arc<HostThrottle>;
