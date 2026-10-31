//! web_search 工具：本地优先三通道搜索。
//!
//! 对 AI 始终是同一个 `web_search` 工具：执行器按**钉死优先级**自动选通道，
//! 用户按需配置、零配置也可用（默认通道 2 无需任何设置）：
//! 1. **SearXNG 自托管元搜索**（通道 1，用户显式启用才参与）：调用
//!    `GET {baseUrl}/search?q=…&format=json`。请求走本地回环、不经应用代理
//!    （自托管服务无代理必要，走代理反而可能被规则拦掉回环地址），也**不经
//!    SSRF 校验**——用户显式启用即信任该地址，而回环地址天然被 SSRF 防线
//!    拒绝。上游 SearXNG 的 settings.yml 需开
//!    `format=json`，配置文档与错误提示都要引导这一点；
//! 2. **内置直接抓取**（通道 2，默认兜底）：DuckDuckGo HTML 版
//!    （`html.duckduckgo.com/html`）GET + 宽松解析结果列表（标题/URL/摘要）。
//!    必须走应用代理（`AppConfig.proxy`）与 SSRF 管线（复用
//!    `safety/ssrf.rs` 的 build_client/fetch/节流）。引擎改版风险为已声明
//!    取舍（个人自用口径）：解析出 0 条时做**空结果自检**——页面明确无结果
//!    → 真空结果（成功返回空列表）；否则视为疑似改版/反爬页（告警并视为
//!    通道失败，让优先级链继续）；
//! 3. **Tavily 云搜索**（通道 3，BYOK 高级区，Key 已配才参与）：
//!    `POST https://api.tavily.com/search`，Bearer Key，`search_depth=basic`
//!    （1 credit/次）。Key 经现行存储协议随 config.json 0600 落盘。
//!    大陆直连可用，仍统一走应用代理设置。免费 1000 credits/月
//!    用尽即当月停用（结构化错误分类，不重试）；429 带 Retry-After 单次
//!    退避后重试一次；401/403 各给明确错误分类。
//!
//! 全部通道失败 → 结构化错误 `E_SEARCH_UNAVAILABLE`：error.message
//! 带逐层失败原因与配置引导（模型通道可读），detail 带结构化 attempts 数组
//! （UI 通道，工具卡逐层展示），不崩不挂。
//!
//! [`SearchProvider`] trait 为通道抽象面（Brave 留位不实现，MCP
//! 搜索服务通道共存、不占内置位）。

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::Future;
use serde::Deserialize;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::providers::types::ProxyConfig;
use crate::providers::types::SearxngConfig;
use crate::safety::ssrf::FetchRequest;
use crate::safety::ssrf::HostThrottle;
use crate::safety::ssrf::SsrfPolicy;
use crate::safety::ssrf::build_client;
use crate::safety::ssrf::fetch;
use crate::util::errors::ToolError;

use super::ToolAnnotations;
use super::ToolOrigin;
use super::ToolOutcome;
use super::ToolSpec;

// ---- 通道端点常量（测试经 SearchEnv 注入面覆盖）----

/// DuckDuckGo HTML 版结果页端点（通道 2）。
pub const DDG_HTML_ENDPOINT: &str = "https://html.duckduckgo.com/html/";
/// Tavily 搜索 API 端点（通道 3）。
pub const TAVILY_ENDPOINT: &str = "https://api.tavily.com/search";

/// SearXNG 通道专用超时（本地自托管服务理应毫秒级响应）：连接 3 秒、
/// 总 15 秒——远短于公网通道的 10s/60s（build_client 兜底常量），让
/// "服务挂了"快速落通道 2 而不是干等（实现细节）。
const SEARXNG_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const SEARXNG_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Tavily 429 限流的单次退避时长上限：Retry-After 头缺省取 5 秒，
/// 超过 30 秒按 30 秒封顶（重试只做一次，再做就是浪费模型等待时间）。
const TAVILY_RETRY_DEFAULT: Duration = Duration::from_secs(5);
const TAVILY_RETRY_CAP: Duration = Duration::from_secs(30);

// ---- 结果数据形状（工具卡与模型通道共用）----

/// 单条搜索结果（结果带来源 URL）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchHit {
    /// 结果标题。
    pub title: String,
    /// 结果链接（http/https 绝对地址）。
    pub url: String,
    /// 摘要/内容片段（通道给什么带什么，可能为空串）。
    pub snippet: String,
}

/// 一个通道的搜索结果页。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchPage {
    /// 来源通道标识（"searxng" | "ddg" | "tavily"；工具卡据此标注来源）。
    pub channel: &'static str,
    /// 结果列表（已按 limit 截断）。
    pub hits: Vec<SearchHit>,
}

// ---- SearchProvider trait（Brave 留 trait 位）----

/// 搜索提供方抽象：
/// 每个通道一个实现；返回的 future 为 boxed 以支持 dyn 分发。
/// 取消令牌透传——通道内部请求以 `tokio::select!` 响应取消。
pub trait SearchProvider: Send + Sync {
    /// 通道标识（结果 channel 字段与逐层失败原因的键）。
    fn channel(&self) -> &'static str;
    /// 通道中文名（错误信息/卡片展示用，如「本地 SearXNG」）。
    fn channel_label(&self) -> &'static str;
    /// 执行一次搜索；失败返回带明确分类的 ToolError（优先级链按层记录）。
    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
        token: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<SearchPage, ToolError>> + Send + 'a>>;
}

// ---- 通道 1：SearXNG（本地自托管，不经代理/不经 SSRF 校验）----

/// SearXNG 通道（优先级①）：用户显式启用才装配进优先级链。
pub struct SearxngProvider {
    /// SearXNG 配置（baseUrl/maxResults；enabled 判定在装配面完成）。
    pub config: SearxngConfig,
    /// 进程级每 host 节流器（与公网通道同一实例，跨通道共用 1s 间隔）。
    pub throttle: Arc<HostThrottle>,
    /// 连接超时（生产 [`SEARXNG_CONNECT_TIMEOUT`]；测试注入毫秒级以便
    /// 快速验证超时回落，不真等 15 秒）。
    pub connect_timeout: Duration,
    /// 总超时（生产 [`SEARXNG_REQUEST_TIMEOUT`]；同上测试注入面）。
    pub request_timeout: Duration,
}

// SearxngProvider 的构造。
impl SearxngProvider {
    /// 生产构造：本地服务专用短超时（3s 连接 / 15s 总）。
    pub fn production(config: SearxngConfig, throttle: Arc<HostThrottle>) -> Self {
        Self {
            config,
            throttle,
            connect_timeout: SEARXNG_CONNECT_TIMEOUT,
            request_timeout: SEARXNG_REQUEST_TIMEOUT,
        }
    }
}

// SearxngProvider 的 SearchProvider 实现。
impl SearchProvider for SearxngProvider {
    fn channel(&self) -> &'static str {
        "searxng"
    }

    fn channel_label(&self) -> &'static str {
        "本地 SearXNG"
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
        token: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<SearchPage, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            // 专用 client：本地回环直连（no_proxy，不经应用代理）。
            // 不走 SSRF 校验管线：用户显式启用即信任该地址，回环地址会被
            // 全地址校验拒绝（SearXNG 默认地址就是 127.0.0.1:8888）。
            let client = reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(self.connect_timeout)
                .timeout(self.request_timeout)
                .user_agent(concat!("CraftWave/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|e| ToolError::new("E_INTERNAL", format!("HTTP 客户端构建失败：{e}")))?;
            // baseUrl 归一化已剥尾 `/`，join 安全拼出 /search；查询串经
            // url crate 拼装（q 做 percent 编码；reqwest 的 .query() 需要未
            // 启用的 feature）
            let base = Url::parse(&self.config.base_url)
                .map_err(|e| ToolError::invalid_arg(format!("SearXNG 地址非法：{e}")))?;
            let mut url = base
                .join("search")
                .map_err(|e| ToolError::invalid_arg(format!("SearXNG 地址拼装失败：{e}")))?;
            url.query_pairs_mut()
                .append_pair("q", query)
                .append_pair("format", "json");
            // 节流（与公网通道同口径；本地连续搜索同样间隔 ≥1s）
            if let Some(wait) = self
                .throttle
                .reserve(url.host_str().unwrap_or(""), std::time::Instant::now())
            {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        return Err(ToolError::new("E_CANCELLED", "搜索在节流等待期间被取消"));
                    }
                    _ = tokio::time::sleep(wait) => {}
                }
            }
            let req = client
                .get(url.clone())
                .build()
                .map_err(|e| ToolError::new("E_INTERNAL", format!("请求构造失败：{e}")))?;
            let resp = tokio::select! {
                biased;
                _ = token.cancelled() => {
                    return Err(ToolError::new("E_CANCELLED", "SearXNG 搜索被取消"));
                }
                resp = client.execute(req) => {
                    resp.map_err(|e| ToolError::new(
                        "E_INTERNAL",
                        format!("无法连接 SearXNG（{}）：{e}", self.config.base_url),
                    ))?
                }
            };
            let status = resp.status().as_u16();
            let body = tokio::select! {
                biased;
                _ = token.cancelled() => {
                    return Err(ToolError::new("E_CANCELLED", "SearXNG 搜索被取消"));
                }
                body = resp.text() => {
                    body.map_err(|e| ToolError::new("E_INTERNAL", format!("SearXNG 响应读取失败：{e}")))?
                }
            };
            // 非 2xx：最常见的 403 = SearXNG settings.yml 未开 format=json
            // （错误提示要引导这一点）
            if !(200..300).contains(&status) {
                return Err(ToolError::new(
                    "E_INTERNAL",
                    format!(
                        "SearXNG 返回 HTTP {status}：若为 403 请在 SearXNG 的 settings.yml \
                         的 search.formats 中加入 json 后重启服务"
                    ),
                ));
            }
            let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
                ToolError::new("E_INTERNAL", format!("SearXNG 响应不是合法 JSON：{e}"))
            })?;
            let mut hits = Vec::new();
            if let Some(results) = json.get("results").and_then(|v| v.as_array()) {
                for item in results {
                    if hits.len() >= limit {
                        break;
                    }
                    // 宽松取字段：url/title 缺失的条目跳过（自托管实例插件
                    // 输出形状偶有差异，能给的都给）
                    let Some(url) = item.get("url").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    hits.push(SearchHit {
                        title: item
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("(无标题)")
                            .to_string(),
                        url: url.to_string(),
                        snippet: item
                            .get("content")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    });
                }
            }
            Ok(SearchPage {
                channel: "searxng",
                hits,
            })
        })
    }
}

// ---- 通道 2：DuckDuckGo HTML 直接抓取（默认兜底，走代理 + SSRF 管线）----

/// DuckDuckGo HTML 通道（优先级②，默认兜底）。
pub struct DdgProvider {
    /// SSRF 策略（与 web_fetch 同源装配）。
    pub policy: SsrfPolicy,
    /// 应用代理配置（通道 2 必须走代理）。
    pub proxy: Option<ProxyConfig>,
    /// 进程级每 host 节流器。
    pub throttle: Arc<HostThrottle>,
    /// 结果页端点（生产 = [`DDG_HTML_ENDPOINT`]；测试注入本地假服务）。
    pub endpoint: String,
}

// DdgProvider 的 SearchProvider 实现。
impl SearchProvider for DdgProvider {
    fn channel(&self) -> &'static str {
        "ddg"
    }

    fn channel_label(&self) -> &'static str {
        "内置抓取（DuckDuckGo）"
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
        token: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<SearchPage, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            // 走 build_client：SSRF 钉址 resolver + 按配置代理 + 超时兜底
            //（与 web_fetch 同源）
            let client = build_client(self.policy, self.proxy.as_ref())?;
            let mut url = Url::parse(&self.endpoint)
                .map_err(|e| ToolError::invalid_arg(format!("抓取端点非法：{e}")))?;
            // 查询串经 url crate 拼装（q 做 percent 编码）
            url.query_pairs_mut().append_pair("q", query);
            let request = FetchRequest {
                url: url.clone(),
                method: reqwest::Method::GET,
                headers: Default::default(),
                body: None,
            };
            let sent = std::sync::atomic::AtomicBool::new(false);
            let response =
                fetch(&client, request, self.policy, &self.throttle, token, &sent).await?;
            if !(200..300).contains(&response.status) {
                return Err(ToolError::new(
                    "E_INTERNAL",
                    format!(
                        "DuckDuckGo 返回非成功状态码 {status}",
                        status = response.status
                    ),
                ));
            }
            let html = String::from_utf8_lossy(&response.body).into_owned();
            match parse_ddg_html(&html, limit) {
                DdgParse::Hits(hits) => Ok(SearchPage {
                    channel: "ddg",
                    hits,
                }),
                // 页面明确无结果：真空结果（成功返回空列表，不落通道 3）
                DdgParse::NoResults => Ok(SearchPage {
                    channel: "ddg",
                    hits: Vec::new(),
                }),
                // 解析出 0 条且页面没有无结果标志：疑似改版/反爬页——
                // 空结果自检告警，视为通道失败让优先级链继续
                DdgParse::LikelyRedesign => Err(ToolError::new(
                    "E_INTERNAL",
                    "DuckDuckGo 页面解析出 0 条结果且无「无结果」标志：搜索引擎可能已改版或返回了反爬页",
                )),
            }
        })
    }
}

/// DDG HTML 解析结论（空结果自检的三态：解析器宽松 + 空结果自检告警）。
pub(crate) enum DdgParse {
    /// 解析出结果（可能截断到 limit）。
    Hits(Vec<SearchHit>),
    /// 页面明确显示无结果（真空结果，成功）。
    NoResults,
    /// 0 条且无无结果标志：疑似改版/反爬页（通道失败）。
    LikelyRedesign,
}

/// 解包 DDG 跳转包装链接，还原真实目标 URL：
/// 结果锚点 href 多为 `//duckduckgo.com/l/?uddg=<percent-encoded>&rut=…`
/// 跳转包装（解析器宽松的一部分）；直接 http(s) 链接原样返回。
/// 解析不出可用的绝对 http(s) 地址 → None（该条目跳过）。
/// `pub(crate)`：web_search_test 向量表直接驱动。
pub(crate) fn unwrap_ddg_link(href: &str) -> Option<String> {
    let raw = href.trim();
    if raw.is_empty() {
        return None;
    }
    // 相对协议补 https（DDG 跳转链接的常见形态）
    let full = if raw.starts_with("//") {
        format!("https:{raw}")
    } else {
        raw.to_string()
    };
    let parsed = Url::parse(&full).ok()?;
    // 跳转包装特征：duckduckgo.com/l/ 的 uddg 查询参数
    if parsed
        .host_str()
        .is_some_and(|h| h.ends_with("duckduckgo.com"))
        && parsed.path() == "/l/"
    {
        for (key, value) in parsed.query_pairs() {
            if key == "uddg" {
                let target = value.into_owned();
                // uddg 值必须是合法 http(s) 绝对地址（防垃圾条目混入）
                if target.starts_with("http://") || target.starts_with("https://") {
                    return Some(target);
                }
                return None;
            }
        }
        return None;
    }
    if parsed.scheme() == "http" || parsed.scheme() == "https" {
        return Some(parsed.to_string());
    }
    None
}

/// 结果文本清洗：压平空白（HTML 源里的换行/缩进无意义）并去首尾空白。
/// 输入来自 dom_query 的 `.text()`——HTML5 解析器已解码实体（`&amp;` →
/// `&`）且不含标签标记（`<b>` 高亮的文字保留、标签本身已剥离），此处不再
/// 做实体/标签处理（误吞字面尖括号文本的教训见快照测试断言）。
fn clean_html_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 解析 DuckDuckGo HTML 结果页（宽松 + 空结果自检）：
/// 按 class 定位结果锚点（`.result__a`）与摘要块（`.result__snippet`），
/// 属性顺序/引号形式由 HTML5 解析器归一（对改版的容错点是 class 名本身，
/// 改版即触发空结果自检的疑似告警——已声明取舍）。
/// `pub(crate)`：web_search_test 快照测试直接驱动本函数。
pub(crate) fn parse_ddg_html(html: &str, limit: usize) -> DdgParse {
    let doc = dom_query::Document::from(html);
    // 首选结构：结果容器块（.result / .web-result）内成对的锚点+摘要；
    // 容器选择落空时退化为全局锚点列表（宽松：结构变体仍可出结果）
    let mut hits = Vec::new();
    let containers = doc.select("div.result, div.web-result");
    if containers.length() > 0 {
        for block in containers.iter() {
            if hits.len() >= limit {
                break;
            }
            let Some(anchor) = block.select("a.result__a").iter().next() else {
                continue;
            };
            let Some(url) = anchor.attr("href").and_then(|h| unwrap_ddg_link(&h)) else {
                continue;
            };
            let title = clean_html_text(anchor.text().as_ref());
            let snippet = block
                .select("a.result__snippet, div.result__snippet")
                .iter()
                .next()
                .map(|s| clean_html_text(s.text().as_ref()))
                .unwrap_or_default();
            hits.push(SearchHit {
                title,
                url,
                snippet,
            });
        }
    } else {
        // 退化路径：容器块改版但锚点 class 仍在
        for anchor in doc.select("a.result__a").iter() {
            if hits.len() >= limit {
                break;
            }
            let Some(url) = anchor.attr("href").and_then(|h| unwrap_ddg_link(&h)) else {
                continue;
            };
            hits.push(SearchHit {
                title: clean_html_text(anchor.text().as_ref()),
                url,
                snippet: String::new(),
            });
        }
    }
    if !hits.is_empty() {
        return DdgParse::Hits(hits);
    }
    // 空结果自检：页面明确无结果标志 → 真空结果。DDG HTML 版的
    // 无结果页含 "did not match any documents" 提示；中文提示兜底匹配
    // 「没有找到」。命中标志 = NoResults；否则视为疑似改版/反爬页。
    let lower = html.to_ascii_lowercase();
    if lower.contains("did not match any documents")
        || lower.contains("no results")
        || html.contains("没有找到")
    {
        return DdgParse::NoResults;
    }
    DdgParse::LikelyRedesign
}

// ---- 通道 3：Tavily（BYOK 云搜索，走应用代理 + SSRF 管线）----

/// Tavily 通道（优先级③）：Key 已配才装配进优先级链。
pub struct TavilyProvider {
    /// Tavily API Key（Bearer；随 config.json 落盘）。
    pub api_key: String,
    /// SSRF 策略（公网请求正常过 SSRF 管线）。
    pub policy: SsrfPolicy,
    /// 应用代理配置（大陆直连可用，仍统一走代理设置）。
    pub proxy: Option<ProxyConfig>,
    /// 进程级每 host 节流器。
    pub throttle: Arc<HostThrottle>,
    /// API 端点（生产 = [`TAVILY_ENDPOINT`]；测试注入本地假服务）。
    pub endpoint: String,
}

/// Tavily 的错误分类（逐层失败原因的分类键，进 attempts 结构化数组）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TavilyErrorKind {
    /// Key 无效（HTTP 401）。
    InvalidKey,
    /// Key 无权限/被封禁（HTTP 403）。
    Forbidden,
    /// 限流（429，单次退避后仍 429）。
    RateLimited,
    /// 当月配额用尽（免费 1000 credits/月）——当月停用，不重试。
    QuotaExhausted,
    /// 其余网络/服务端错误。
    RequestFailed,
}

// TavilyErrorKind 的中文分类名（错误 message 用）。
impl TavilyErrorKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::InvalidKey => "Key 无效（401）",
            Self::Forbidden => "Key 无权限（403）",
            Self::RateLimited => "限流（429，退避重试后仍被拒）",
            Self::QuotaExhausted => {
                "当月配额已用尽（免费 1000 credits/月）——本月停用，下月 1 日重置"
            }
            Self::RequestFailed => "请求失败",
        }
    }
}

/// 把 Tavily 的失败形态分类为 [`TavilyErrorKind`]（分类核，纯逻辑）：
/// 429 且响应体带 quota/plan limit 字样、或 432 → 配额尽（当月停用不重试）；
/// 其余 429 → 限流（可退避重试）；401/403 各自分类。
/// `pub(crate)`：测试直接驱动分类逻辑（录制回放的 429/401/配额尽形态）。
pub(crate) fn tavily_classify(status: u16, body: &str) -> TavilyErrorKind {
    let lower = body.to_ascii_lowercase();
    if status == 432 || (status == 429 && (lower.contains("quota") || lower.contains("plan limit")))
    {
        TavilyErrorKind::QuotaExhausted
    } else {
        match status {
            401 => TavilyErrorKind::InvalidKey,
            403 => TavilyErrorKind::Forbidden,
            429 | 432 => TavilyErrorKind::RateLimited,
            _ => TavilyErrorKind::RequestFailed,
        }
    }
}

/// Tavily 分类错误构造（message 带中文分类 + 响应体片段）。
fn tavily_error(kind: TavilyErrorKind, body: &str) -> ToolError {
    let brief: String = body.chars().take(200).collect();
    ToolError::new("E_INTERNAL", format!("Tavily（{}）：{brief}", kind.label()))
}

/// 发起一次 Tavily 搜索请求（POST + Bearer + basic 深度档）。
/// 失败返回 `(分类, Retry-After 时长, 响应体)`——分类与退避参数延迟到
/// 调用方（trait 面）处理，供 429 退避重试链复用。`pub(crate)`：测试直接驱动。
async fn tavily_request(
    provider: &TavilyProvider,
    query: &str,
    limit: usize,
    token: &CancellationToken,
) -> Result<SearchPage, (TavilyErrorKind, Option<Duration>, String)> {
    let client = build_client(provider.policy, provider.proxy.as_ref())
        .map_err(|e| (TavilyErrorKind::RequestFailed, None, e.message.to_string()))?;
    let url = Url::parse(&provider.endpoint).map_err(|e| {
        (
            TavilyErrorKind::RequestFailed,
            None,
            format!("端点非法：{e}"),
        )
    })?;
    let payload = serde_json::json!({
        "query": query,
        "search_depth": "basic",
        "max_results": limit,
        "include_answer": false,
    });
    let headers = {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            reqwest::header::AUTHORIZATION,
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", provider.api_key))
                .map_err(|e| {
                    (
                        TavilyErrorKind::RequestFailed,
                        None,
                        format!("Key 含非法字符：{e}"),
                    )
                })?,
        );
        headers
    };
    let request = FetchRequest {
        url,
        method: reqwest::Method::POST,
        headers,
        body: Some(payload.to_string().into_bytes()),
    };
    let sent = std::sync::atomic::AtomicBool::new(false);
    let response = fetch(
        &client,
        request,
        provider.policy,
        &provider.throttle,
        token,
        &sent,
    )
    .await
    .map_err(|e| (TavilyErrorKind::RequestFailed, None, e.message.to_string()))?;
    let status = response.status;
    let body = String::from_utf8_lossy(&response.body).into_owned();
    if !(200..300).contains(&status) {
        // Retry-After：仅秒数形态（Tavily 文档口径）；解析不出按缺省退避
        let retry_after = response
            .headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        return Err((tavily_classify(status, &body), retry_after, body));
    }
    // 200：解析结果数组
    let json: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
        (
            TavilyErrorKind::RequestFailed,
            None,
            format!("响应不是合法 JSON：{e}"),
        )
    })?;
    let mut hits = Vec::new();
    if let Some(results) = json.get("results").and_then(|v| v.as_array()) {
        for item in results {
            if hits.len() >= limit {
                break;
            }
            let Some(url) = item.get("url").and_then(|v| v.as_str()) else {
                continue;
            };
            hits.push(SearchHit {
                title: item
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(无标题)")
                    .to_string(),
                url: url.to_string(),
                // content = Tavily 的摘要/正文片段字段
                snippet: item
                    .get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            });
        }
    }
    Ok(SearchPage {
        channel: "tavily",
        hits,
    })
}

// TavilyProvider 的 SearchProvider 实现（分类与退避在 trait 面收口）。
impl SearchProvider for TavilyProvider {
    fn channel(&self) -> &'static str {
        "tavily"
    }

    fn channel_label(&self) -> &'static str {
        "云搜索（Tavily）"
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
        token: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<SearchPage, ToolError>> + Send + 'a>> {
        Box::pin(async move {
            match tavily_request(self, query, limit, token).await {
                Ok(page) => Ok(page),
                Err((kind, retry_after, body)) => {
                    // 限流（429 且非配额尽）：单次退避后重试一次
                    //（Retry-After 优先，缺省 5s 封顶 30s）；配额尽不重试
                    if kind == TavilyErrorKind::RateLimited {
                        let wait = retry_after
                            .unwrap_or(TAVILY_RETRY_DEFAULT)
                            .min(TAVILY_RETRY_CAP);
                        tokio::select! {
                            biased;
                            _ = token.cancelled() => {
                                return Err(ToolError::new("E_CANCELLED", "Tavily 搜索在退避等待期间被取消"));
                            }
                            _ = tokio::time::sleep(wait) => {}
                        }
                        return match tavily_request(self, query, limit, token).await {
                            Ok(page) => Ok(page),
                            Err((kind, _, body)) => Err(tavily_error(kind, &body)),
                        };
                    }
                    Err(tavily_error(kind, &body))
                }
            }
        })
    }
}

// ---- 逐层失败记录与结构化错误（全败面）----

/// 单层通道的失败记录（detail.attempts 数组元素 + message 拼装来源）。
#[derive(Debug, Clone, Serialize)]
pub struct ChannelAttempt {
    /// 通道标识。
    pub channel: &'static str,
    /// 通道中文名。
    pub label: &'static str,
    /// 是否实际尝试（false = 未参与：SearXNG 未启用 / Tavily 未配 Key）。
    pub tried: bool,
    /// 失败原因（tried=false 时省略）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 三层全败的结构化错误构造：error.message 带逐层原因 + 配置引导（模型
/// 通道）；detail 带结构化 attempts（UI 通道，工具卡逐层展示）。
fn all_failed_error(attempts: &[ChannelAttempt]) -> ToolOutcome {
    let mut message = String::from("网络搜索全部通道失败：");
    for attempt in attempts {
        match (attempt.tried, &attempt.error) {
            (true, Some(err)) => message.push_str(&format!(
                "\n- {}（{}）：{err}",
                attempt.label, attempt.channel
            )),
            _ => message.push_str(&format!(
                "\n- {}（{}）：未启用/未配置",
                attempt.label, attempt.channel
            )),
        }
    }
    message.push_str(
        "\n配置引导：① SearXNG 在「设置 → 网络 → 网络搜索」启用并填写服务地址\
         （自托管实例需在 settings.yml 的 search.formats 中加入 json）；\
         ② 内置抓取无需配置但依赖 DuckDuckGo 可达（必要时配置应用代理）；\
         ③ Tavily 在同页高级区填入 API Key（tavily.com 免费注册，每月 1000 次免费额度）。",
    );
    ToolOutcome::failure(
        ToolError::new("E_SEARCH_UNAVAILABLE", message),
        "网络搜索失败（全部通道不可用）",
    )
    .with_detail(serde_json::json!({ "attempts": attempts }))
}

// ---- 执行环境与编排（优先级链）----

/// web_search 执行环境：由 [`crate::tools::NetworkEnv`] 按 AppConfig 装配
/// （save_config 后下一次 start_chat 生效，与 web_fetch 同语义）。
/// 端点字段为测试注入面（生产构造置 None = 用常量真实端点）。
#[derive(Clone)]
pub struct SearchEnv {
    /// 通道 1 配置：`search.searxng.enabled == true` 才 Some（未启用不参与）。
    pub searxng: Option<SearxngConfig>,
    /// 通道 3 Key：未配（None/空白）不参与。
    pub tavily_api_key: Option<String>,
    /// 工具未带 limit 参数时的缺省结果条数（`search.searxng.maxResults`
    /// 配置透出：各通道共用同一缺省）。
    pub max_results_default: u32,
    /// SSRF 策略（通道 2/3 共用）。
    pub policy: SsrfPolicy,
    /// 应用代理配置（通道 2 必经；通道 3 统一走代理设置）。
    pub proxy: Option<ProxyConfig>,
    /// 进程级每 host 节流器（三通道共用同一实例）。
    pub throttle: Arc<HostThrottle>,
    /// 通道 2 端点（None = [`DDG_HTML_ENDPOINT`]）。
    pub ddg_endpoint: Option<String>,
    /// 通道 3 端点（None = [`TAVILY_ENDPOINT`]）。
    pub tavily_endpoint: Option<String>,
}

// SearchEnv 的构造。
impl SearchEnv {
    /// 按应用配置装配（search 组 + network 组 + 代理组一次配齐）。
    pub fn from_config(config: &crate::providers::types::AppConfig) -> Self {
        Self {
            searxng: config
                .search
                .searxng
                .enabled
                .then(|| config.search.searxng.clone()),
            tavily_api_key: config.search.tavily_api_key.clone(),
            max_results_default: config.search.searxng.max_results,
            policy: SsrfPolicy {
                allow_private_network: config.network.allow_private_network,
            },
            proxy: config.proxy.clone(),
            throttle: Arc::new(HostThrottle::production()),
            ddg_endpoint: None,
            tavily_endpoint: None,
        }
    }

    /// 通道 2 端点（生产常量或测试注入）。
    fn ddg_endpoint(&self) -> String {
        self.ddg_endpoint
            .clone()
            .unwrap_or_else(|| DDG_HTML_ENDPOINT.to_string())
    }

    /// 通道 3 端点（生产常量或测试注入）。
    fn tavily_endpoint(&self) -> String {
        self.tavily_endpoint
            .clone()
            .unwrap_or_else(|| TAVILY_ENDPOINT.to_string())
    }
}

/// web_search 参数：`{ query, limit? }`。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSearchArgs {
    /// 搜索关键词。
    pub query: String,
    /// 结果条数（1–30；缺省取 search.searxng.maxResults 配置，再缺省 10）。
    #[serde(default)]
    pub limit: Option<u32>,
}

/// 取消可感知的执行退出态（executor 映射 ToolExecution，同 web_fetch）。
pub enum SearchExit {
    /// 正常结束（含全部通道失败的工具级失败，结果在 ToolOutcome 里）。
    Done(Box<ToolOutcome>),
    /// 取消：搜索是只读 GET/POST 查询，按"未执行"口径报告（重试无害）。
    Cancelled,
}

/// 执行一次网络搜索（门控放行后调用）：按钉死优先级 ①SearXNG（启用且
/// 探活成功）→ ②内置抓取 → ③Tavily（Key 已配）线性回落，全败结构化错误。
/// `max_results_default` 为工具未带 limit 时的缺省条数（来自
/// search.searxng.maxResults 配置；执行器从 NetworkEnv 带出）。
pub async fn execute(
    env: &SearchEnv,
    max_results_default: u32,
    args: &WebSearchArgs,
    token: &CancellationToken,
) -> SearchExit {
    // 参数预检：空查询不进通道链
    let query = args.query.trim();
    if query.is_empty() {
        return SearchExit::Done(Box::new(ToolOutcome::failure(
            ToolError::invalid_arg("query 不能为空"),
            "搜索关键词为空",
        )));
    }
    // limit：显式参数 > 配置缺省 > 常量 10；clamp 1..=30（search 组配置）
    let limit = args
        .limit
        .unwrap_or(max_results_default)
        .clamp(1, crate::providers::types::MAX_SEARCH_RESULTS_LIMIT) as usize;
    // 取消在整个通道链期间可中断（try_channel 内部也逐请求监听，此处
    // 兜底通道切换间隙的取消）
    tokio::select! {
        biased;
        _ = token.cancelled() => SearchExit::Cancelled,
        outcome = run_chain(env, query, limit, token) => SearchExit::Done(Box::new(outcome)),
    }
}

/// 按优先级跑通道链：单通道成功即返回成功结果（附逐层尝试记录进 detail）；
/// 全败返回结构化错误。
async fn run_chain(
    env: &SearchEnv,
    query: &str,
    limit: usize,
    token: &CancellationToken,
) -> ToolOutcome {
    let mut attempts: Vec<ChannelAttempt> = Vec::with_capacity(3);

    // ① SearXNG（用户显式启用才参与）
    if let Some(config) = &env.searxng {
        let provider = SearxngProvider::production(config.clone(), env.throttle.clone());
        if let Some(page) = try_channel(&provider, query, limit, token, &mut attempts).await {
            return success_outcome(query, page, &attempts);
        }
    } else {
        attempts.push(ChannelAttempt {
            channel: "searxng",
            label: "本地 SearXNG",
            tried: false,
            error: None,
        });
    }

    // ② 内置直接抓取（默认兜底）
    let ddg = DdgProvider {
        policy: env.policy,
        proxy: env.proxy.clone(),
        throttle: env.throttle.clone(),
        endpoint: env.ddg_endpoint(),
    };
    if let Some(page) = try_channel(&ddg, query, limit, token, &mut attempts).await {
        return success_outcome(query, page, &attempts);
    }

    // ③ Tavily（Key 已配才参与）
    if let Some(api_key) = &env.tavily_api_key {
        let tavily = TavilyProvider {
            api_key: api_key.clone(),
            policy: env.policy,
            proxy: env.proxy.clone(),
            throttle: env.throttle.clone(),
            endpoint: env.tavily_endpoint(),
        };
        if let Some(page) = try_channel(&tavily, query, limit, token, &mut attempts).await {
            return success_outcome(query, page, &attempts);
        }
    } else {
        attempts.push(ChannelAttempt {
            channel: "tavily",
            label: "云搜索（Tavily）",
            tried: false,
            error: None,
        });
    }

    all_failed_error(&attempts)
}

/// 跑单通道并把结果/失败记进 attempts；成功返回 Some(结果页)。
async fn try_channel(
    provider: &dyn SearchProvider,
    query: &str,
    limit: usize,
    token: &CancellationToken,
    attempts: &mut Vec<ChannelAttempt>,
) -> Option<SearchPage> {
    // 取消在整个通道尝试期间可中断
    let result = tokio::select! {
        biased;
        _ = token.cancelled() => return None, // 取消由 execute 统一上浮
        result = provider.search(query, limit, token) => result,
    };
    match result {
        Ok(page) => {
            attempts.push(ChannelAttempt {
                channel: provider.channel(),
                label: provider.channel_label(),
                tried: true,
                error: None,
            });
            Some(page)
        }
        Err(err) => {
            attempts.push(ChannelAttempt {
                channel: provider.channel(),
                label: provider.channel_label(),
                tried: true,
                error: Some(err.message.to_string()),
            });
            None
        }
    }
}

/// 成功结果的 ToolOutcome：data 带 query/channel/results/count（模型通道
/// 要求"据此行动的结论必须写入 data"——结果列表本身就是结论）；
/// detail 附逐层尝试记录（UI 通道，工具卡展示回落过程）。
fn success_outcome(query: &str, page: SearchPage, attempts: &[ChannelAttempt]) -> ToolOutcome {
    let count = page.hits.len();
    let channel_label = match page.channel {
        "searxng" => "本地",
        "ddg" => "抓取",
        "tavily" => "云",
        _ => "未知",
    };
    ToolOutcome::success(
        serde_json::json!({
            "query": query,
            "channel": page.channel,
            "results": page.hits,
            "count": count,
        }),
        format!("已搜索「{query}」（{channel_label}通道，{count} 条结果）"),
    )
    .with_detail(serde_json::json!({ "attempts": attempts }))
}

/// web_search 工具规格（能力 Network：plan 档过滤、非 full_access 档审批）。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "web_search".to_string(),
        description: "联网搜索并返回标题/链接/摘要列表；按优先级自动选通道（本地 SearXNG → \
内置抓取 → Tavily 云），全通道失败时返回逐层原因与配置引导。"
            .to_string(),
        capability: crate::safety::tier::Capability::Network,
        params_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "搜索关键词" },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 30,
                    "description": "返回条数上限（默认 10）"
                }
            },
            "required": ["query"],
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
