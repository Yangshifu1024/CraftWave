//! search 域命令（§3.3 #59f，13 号票）：网络搜索通道的连通测试。
//!
//! 对齐 08 号票 `mcp_test` 的「草稿试连」模式：不经保存直接用表单当前值
//! 发起一次真实探测请求，返回 `{ ok, latencyMs?, error? }`——设置页
//! 「连通测试」按钮的 IPC 承载面。SearXNG 测试 = GET
//! `{baseUrl}/search?q=…&format=json`（本地回环直连，不经代理）；Tavily
//! 测试 = POST `/search`（Bearer Key，走应用代理设置）——二者都复用
//! tools/web_search 的通道实现（测试与工具执行同一条请求路径，测通即用通）。

use serde::Deserialize;
use serde::Serialize;
use tauri::State;
use tokio_util::sync::CancellationToken;

use crate::host::AppState;
use crate::providers::types::SearxngConfig;
use crate::tools::web_search::SearchProvider;
use crate::tools::web_search::SearxngProvider;
use crate::tools::web_search::TavilyProvider;
use crate::util::errors::IpcError;

/// 连通测试总预算（秒）：SearXNG 通道自身 15s 超时、Tavily 走公网兜底
/// 60s，命令面统一 30s 收口（超时按失败返回，不挂设置页）。
const SEARCH_TEST_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// `search_test` 参数契约（§3.3 #59f）：目标通道 + 草稿配置（不经保存）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SearchTestArgs {
    /// 目标通道："searxng" | "tavily"（其余值按参数错误拒绝）。
    pub target: String,
    /// SearXNG 草稿基地址（target=searxng 时必填；空/缺失按失败返回引导）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub searxng_base_url: Option<String>,
    /// Tavily 草稿 Key（target=tavily 时必填；空/缺失按失败返回引导）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub tavily_api_key: Option<String>,
}

/// `search_test` 返回契约（§3.3 #59f）：ok + 时延或错误原因。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SearchTestResult {
    /// 是否连通（探测请求 2xx 且响应结构可解析）。
    pub ok: bool,
    /// 时延毫秒（ok=true 时）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub latency_ms: Option<u64>,
    /// 失败原因（ok=false 时；含配置引导，如 SearXNG 403 → settings.yml
    /// 的 search.formats 加 json）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub error: Option<String>,
}

/// `search_test`（§3.3 #59f）：通道连通测试（草稿试连，不写配置、不落盘；
/// Tavily 测试会消耗 1 credit——设置页指引文案已注明）。
#[tauri::command]
pub async fn search_test(
    state: State<'_, AppState>,
    args: SearchTestArgs,
) -> Result<SearchTestResult, IpcError> {
    // 目标通道白名单（非法值按参数错误拒绝，区别于「测了但不通」）
    if args.target != "searxng" && args.target != "tavily" {
        return Err(IpcError::invalid_arg(format!(
            "target 仅支持 searxng/tavily（实际 {}）",
            args.target
        )));
    }
    // 代理/SSRF 策略按当前配置装配（与工具执行同源；测试通道 = 真实通道）
    let config = state.config.load()?.as_ref().clone();
    let env = crate::tools::web_search::SearchEnv::from_config(&config);
    let token = CancellationToken::new();
    // 探测 future：两通道统一为 Result<(), String>（通道错误取 message），
    // 总预算 30s 收口（tokio::time::timeout——超时按失败返回）
    let probe = async {
        match args.target.as_str() {
            // SearXNG：本地回环直连（不经代理），一次 format=json 搜索探测
            "searxng" => {
                let base_url = args.searxng_base_url.as_deref().unwrap_or("").trim();
                if base_url.is_empty() {
                    return Err("请先填写 SearXNG 服务地址".to_owned());
                }
                let config = SearxngConfig {
                    enabled: true,
                    base_url: base_url.trim_end_matches('/').to_owned(),
                    max_results: 1,
                };
                let provider = SearxngProvider::production(config, env.throttle.clone());
                provider
                    .search("craftwave connectivity test", 1, &token)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.message.to_string())
            }
            // Tavily：Bearer Key 试连（走应用代理设置；消耗 1 credit）
            _ => {
                let api_key = args.tavily_api_key.as_deref().unwrap_or("").trim();
                if api_key.is_empty() {
                    return Err("请先填写 Tavily API Key（tavily.com 免费注册）".to_owned());
                }
                let provider = TavilyProvider {
                    api_key: api_key.to_owned(),
                    policy: env.policy,
                    proxy: env.proxy.clone(),
                    throttle: env.throttle.clone(),
                    endpoint: crate::tools::web_search::TAVILY_ENDPOINT.to_string(),
                };
                provider
                    .search("craftwave connectivity test", 1, &token)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.message.to_string())
            }
        }
    };
    let started = std::time::Instant::now();
    match tokio::time::timeout(SEARCH_TEST_BUDGET, probe).await {
        // 2xx 且结构可解析：连通（带时延）
        Ok(Ok(())) => Ok(SearchTestResult {
            ok: true,
            latency_ms: Some(started.elapsed().as_millis() as u64),
            error: None,
        }),
        // 探测失败：带通道给出的原因（含配置引导）
        Ok(Err(message)) => Ok(SearchTestResult {
            ok: false,
            latency_ms: None,
            error: Some(message),
        }),
        // 总预算超时
        Err(_) => Ok(SearchTestResult {
            ok: false,
            latency_ms: None,
            error: Some(format!(
                "连通测试超时（超过 {} 秒未收到响应）",
                SEARCH_TEST_BUDGET.as_secs()
            )),
        }),
    }
}
