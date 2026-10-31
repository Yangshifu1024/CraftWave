//! web_search 工具测试：DDG HTML 快照解析（空结果自检三态）+
//! SearXNG/Tavily 本地假服务回放（不连真实外网）+ 三通道
//! 优先级切换 e2e（启用/停用 SearXNG 同查询走不同通道）。矩阵化合并为
//! 解析 / 通道状态 / 优先级链三个用例。
//!
//! 快照入库说明：`fixtures/duckduckgo_*.html` 为按真实 DuckDuckGo HTML 版
//! 结构手工构造的仿真快照（uddg 跳转包装/直接链接/HTML 实体/高亮标签等
//! 典型形态），真实站点改版不挂 CI——改版形态由 redesign 快照与
//! LikelyRedesign 判定覆盖。

use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use tokio_util::sync::CancellationToken;

use super::web_search::DdgParse;
use super::web_search::SearchEnv;
use super::web_search::SearchExit;
use super::web_search::SearchProvider;
use super::web_search::TavilyErrorKind;
use super::web_search::WebSearchArgs;
use super::web_search::execute;
use super::web_search::parse_ddg_html;
use super::web_search::tavily_classify;
use super::web_search::unwrap_ddg_link;
use crate::providers::types::SearxngConfig;
use crate::safety::ssrf::HostThrottle;
use crate::safety::ssrf::SsrfPolicy;

/// 中文/英文无关的快速节流器（测试内请求间距 1ms，避免真实 1s 节流拖慢）。
fn fast_throttle() -> Arc<HostThrottle> {
    Arc::new(HostThrottle::new(Duration::from_millis(1)))
}

/// 测试用 SSRF 策略：允许私网（本地 axum 假服务跑在 127.0.0.1，走
/// ssrf::fetch 的通道需要放行回环；生产缺省 false 不受影响）。
fn local_policy() -> SsrfPolicy {
    SsrfPolicy {
        allow_private_network: true,
    }
}

// ---- 快照（fixtures 入库，include_str 保编译期绑定）----

/// DDG 结果页快照（3 条结果，uddg 包装 + 直链混合）。
const DDG_RESULTS: &str = include_str!("fixtures/duckduckgo_results.html");
/// DDG 无结果页快照。
const DDG_EMPTY: &str = include_str!("fixtures/duckduckgo_empty.html");
/// DDG 疑似改版/反爬页快照。
const DDG_REDESIGN: &str = include_str!("fixtures/duckduckgo_redesign.html");

// ---- DDG 解析：快照回放（纯函数，不联网）----

// 断言：DDG 解析矩阵——结果页快照解析出 uddg 解包/实体解码/高亮剥除/非空
// 摘要、limit 截断生效；无结果页判定真空结果；反爬/改版页判定疑似改版；
// unwrap_ddg_link 六种链接形态（uddg 包装/直链/相对协议/非 http/非法 uddg/
// 缺参数）。
#[test]
fn ddg_parsing_matrix() {
    // 结果页快照：3 条结果。
    let DdgParse::Hits(hits) = parse_ddg_html(DDG_RESULTS, 30) else {
        panic!("结果页快照应解析出结果");
    };
    assert_eq!(hits.len(), 3, "快照含 3 条结果：{hits:?}");
    // uddg 跳转包装解包为真实 URL
    assert_eq!(hits[0].url, "https://www.rust-lang.org/");
    assert_eq!(hits[0].title, "Rust Programming Language");
    // 直接 http 链接原样保留
    assert_eq!(hits[1].url, "https://tokio.rs/");
    // 实体解码 + 高亮标签剥除（&amp; → &，<b> 剥除，&lt; 恢复字面 <）
    assert!(
        hits[0].snippet.contains("reliable & efficient"),
        "实体应解码：{:?}",
        hits[0].snippet
    );
    assert!(
        hits[2].snippet.contains("<第 3 章更深入>"),
        "字面尖括号应恢复：{:?}",
        hits[2].snippet
    );
    assert!(
        !hits
            .iter()
            .any(|h| h.snippet.contains('<') && h.snippet.contains("b>")),
        "高亮标签应剥除：{hits:?}"
    );
    // 摘要非空（三段都有 snippet）
    assert!(hits.iter().all(|h| !h.snippet.is_empty()));

    // limit 截断。
    let DdgParse::Hits(hits) = parse_ddg_html(DDG_RESULTS, 2) else {
        panic!("结果页快照应解析出结果");
    };
    assert_eq!(hits.len(), 2, "limit 截断到 2 条");

    // 无结果页：判定为真空结果（成功空列表），不触发改版告警。
    assert!(matches!(parse_ddg_html(DDG_EMPTY, 10), DdgParse::NoResults));
    // 反爬/改版页：判定为疑似改版（通道失败，让优先级链继续）。
    assert!(matches!(
        parse_ddg_html(DDG_REDESIGN, 10),
        DdgParse::LikelyRedesign
    ));

    // unwrap_ddg_link 变体表：(输入, 期望输出)。
    let link_cases: [(&str, Option<&str>); 6] = [
        (
            // uddg 跳转包装（相对协议 + percent 编码）
            "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa%20b&rut=x",
            Some("https://example.com/a b"),
        ),
        ("https://example.com/", Some("https://example.com/")),
        // 相对协议直接链接补 https
        ("//example.com/page", Some("https://example.com/page")),
        // 非 http(s)（javascript: 等）拒绝
        ("javascript:void(0)", None),
        // uddg 值非法（非 http 开头）拒绝
        ("//duckduckgo.com/l/?uddg=ftp://x&rut=y", None),
        // /l/ 路径但缺 uddg 参数拒绝
        ("//duckduckgo.com/l/?rut=y", None),
    ];
    for (input, expected) in link_cases {
        assert_eq!(
            unwrap_ddg_link(input).as_deref(),
            expected,
            "链接形态 {input} 解包不符"
        );
    }
}

// ---- 本地假服务（axum fixture）：SearXNG / DDG / Tavily 回放 ----

/// 起一个本地假服务，返回基地址（127.0.0.1 随机端口）。`routes` 即 axum
/// 路由定义；测试结束连接自然断开（Router 持有于任务内）。
async fn spawn_local_server(routes: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假服务端口");
    let addr = listener.local_addr().expect("读端口");
    tokio::spawn(async move {
        axum::serve(listener, routes).await.expect("假服务运行");
    });
    format!("http://{addr}")
}

/// SearXNG 假服务：`/search?q=…&format=json` 返回 SearXNG JSON API 形状。
/// `mode` 控制三态：正常 JSON / 403（format 未开）/ 挂起（超时态）。
#[derive(Clone, Copy, PartialEq)]
enum FakeMode {
    /// 正常返回（带结果 JSON）。
    Normal,
    /// HTTP 403（SearXNG settings.yml 未开 format=json 的真实形态）。
    Forbidden,
    /// 挂起 3 秒再返回（配合测试注入的毫秒级超时验证超时回落）。
    Hang,
}

async fn spawn_searxng_fake(mode: FakeMode) -> String {
    let app = axum::Router::new().route(
        "/search",
        axum::routing::get(move |req: Request| async move {
            // 查询串校验：q 与 format=json 都必须带（工具契约）
            let query = req.uri().query().unwrap_or_default();
            if mode == FakeMode::Hang {
                tokio::time::sleep(Duration::from_secs(3)).await;
                return (StatusCode::OK, "{}").into_response();
            }
            if !query.contains("format=json") {
                return (StatusCode::FORBIDDEN, "format not allowed").into_response();
            }
            match mode {
                FakeMode::Normal => (
                    StatusCode::OK,
                    serde_json::json!({
                        "query": "test",
                        "results": [
                            {"url": "https://docs.rs/", "title": "docs.rs", "content": "Rust 文档"},
                            {"url": "https://crates.io/", "title": "crates.io", "content": "Rust 包仓库"}
                        ],
                        "number_of_results": 2
                    })
                    .to_string(),
                )
                    .into_response(),
                _ => (StatusCode::FORBIDDEN, "format not allowed").into_response(),
            }
        }),
    );
    spawn_local_server(app).await
}

/// Tavily 假服务：`/search` 按录制形态回放（200/401/429+Retry-After/432）。
struct TavilyScript {
    /// 各次请求依次回放的状态形态（空则 200 正常）。
    responses: std::sync::Mutex<Vec<(StatusCode, &'static str)>>,
    /// 实际收到的请求计数（断言退避重试发生了第二次请求）。
    hits: std::sync::atomic::AtomicUsize,
    /// 实际收到的 Authorization 头（断言 Bearer Key）。
    auth_seen: std::sync::Mutex<Vec<String>>,
}

impl TavilyScript {
    fn new(responses: Vec<(StatusCode, &'static str)>) -> Arc<Self> {
        Arc::new(Self {
            responses: std::sync::Mutex::new(responses),
            hits: std::sync::atomic::AtomicUsize::new(0),
            auth_seen: std::sync::Mutex::new(Vec::new()),
        })
    }
}

async fn spawn_tavily_fake(script: Arc<TavilyScript>) -> String {
    let app = axum::Router::new().route(
        "/search",
        axum::routing::post(move |req: Request| {
            let script = script.clone();
            async move {
                script
                    .hits
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                script.auth_seen.lock().unwrap().push(
                    req.headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned(),
                );
                let mut queue = script.responses.lock().unwrap();
                if queue.is_empty() {
                    // 缺省 200 正常（录制回放的 happy path 形态）
                    return (
                        StatusCode::OK,
                        serde_json::json!({
                            "query": "test",
                            "results": [
                                {"title": "Tavily", "url": "https://tavily.com", "content": "Search API for AI agents", "score": 0.99}
                            ],
                            "response_time": 0.42
                        })
                        .to_string(),
                    )
                        .into_response();
                }
                let (status, body) = queue.remove(0);
                (status, body.to_string()).into_response()
            }
        }),
    );
    spawn_local_server(app).await
}

/// 短超时 SearXNG provider（测试注入毫秒级超时，不真等 15 秒）。
fn searxng_provider(base_url: String, timeout: Duration) -> super::web_search::SearxngProvider {
    super::web_search::SearxngProvider {
        config: SearxngConfig {
            enabled: true,
            base_url,
            max_results: 10,
        },
        throttle: fast_throttle(),
        connect_timeout: Duration::from_millis(500),
        request_timeout: timeout,
    }
}

/// Tavily provider（测试构造：假服务端点 + 本地策略）。
fn tavily_provider(endpoint: String, api_key: &str) -> super::web_search::TavilyProvider {
    super::web_search::TavilyProvider {
        api_key: api_key.to_string(),
        policy: local_policy(),
        proxy: None,
        throttle: fast_throttle(),
        endpoint,
    }
}

// 断言：通道状态矩阵——SearXNG 四态（正常 2 条命中/403 引导 settings.yml/
// 超时快速回落/不可达失败）与 limit 语义（缺省取配置、显式 0 clamp 到 1）；
// Tavily 五态（正常含 Bearer Key/429 带 Retry-After 单次退避重试/401 分类
// Key 无效不重试/432 配额尽不重试/分类核向量表 401-403-429-432-500）。
#[tokio::test]
async fn channel_provider_states_matrix() {
    let token = CancellationToken::new();

    // —— SearXNG 正常态 ——
    let base = spawn_searxng_fake(FakeMode::Normal).await;
    let page = searxng_provider(base.clone(), Duration::from_secs(5))
        .search("rust 文档", 10, &token)
        .await
        .expect("正常态应成功");
    assert_eq!(page.channel, "searxng");
    assert_eq!(page.hits.len(), 2);
    assert_eq!(page.hits[0].url, "https://docs.rs/");
    assert_eq!(page.hits[0].title, "docs.rs");
    assert_eq!(page.hits[0].snippet, "Rust 文档");

    // 403 = settings.yml 未开 format=json：错误信息必须引导配置。
    let base = spawn_searxng_fake(FakeMode::Forbidden).await;
    let err = searxng_provider(base, Duration::from_secs(5))
        .search("rust", 10, &token)
        .await
        .expect_err("403 应失败");
    assert!(
        err.message.contains("settings.yml"),
        "错误应引导 settings.yml 配置：{}",
        err.message
    );

    // 超时态：假服务挂 3 秒、provider 总超时 200ms——应快速失败。
    let base = spawn_searxng_fake(FakeMode::Hang).await;
    let started = std::time::Instant::now();
    let result = searxng_provider(base, Duration::from_millis(200))
        .search("rust", 10, &token)
        .await;
    let elapsed = started.elapsed();
    assert!(result.is_err(), "超时应失败");
    assert!(
        elapsed < Duration::from_secs(2),
        "应在超时阈值附近快速失败（实际 {elapsed:?}）"
    );

    // 探活失败态：连接不存在的端口。
    let result = searxng_provider("http://127.0.0.1:9".to_string(), Duration::from_secs(2))
        .search("rust", 10, &token)
        .await;
    assert!(result.is_err(), "不可达应失败");

    // —— limit 语义（独立正常态假服务）：缺省取配置 maxResults=2、显式 0
    //    clamp 到 1（schema minimum=1）——
    let base = spawn_searxng_fake(FakeMode::Normal).await;
    let env = SearchEnv {
        searxng: Some(SearxngConfig {
            enabled: true,
            base_url: base,
            max_results: 2,
        }),
        tavily_api_key: None,
        max_results_default: 2,
        policy: local_policy(),
        proxy: None,
        throttle: fast_throttle(),
        ddg_endpoint: None,
        tavily_endpoint: None,
    };
    let exit = execute(
        &env,
        2,
        &WebSearchArgs {
            query: "rust".to_string(),
            limit: None,
        },
        &token,
    )
    .await;
    let SearchExit::Done(outcome) = exit else {
        panic!("不应取消");
    };
    assert_eq!(
        outcome.data.expect("应有 data")["count"],
        2,
        "limit 缺省 = 配置 maxResults（2 条全保留）"
    );
    let exit = execute(
        &env,
        2,
        &WebSearchArgs {
            query: "rust".to_string(),
            limit: Some(0),
        },
        &token,
    )
    .await;
    let SearchExit::Done(outcome) = exit else {
        panic!("不应取消");
    };
    assert!(outcome.ok);
    assert_eq!(
        outcome.data.expect("应有 data")["count"],
        1,
        "limit=0 clamp 到 1（假服务 2 条被截为 1 条）"
    );

    // —— Tavily 正常态：解析结果 + Bearer Key 形态 ——
    let script = TavilyScript::new(vec![]);
    let base = spawn_tavily_fake(script.clone()).await;
    let page = tavily_provider(format!("{base}/search"), "tvly-test-key")
        .search("rust search", 10, &token)
        .await
        .expect("正常态应成功");
    assert_eq!(page.channel, "tavily");
    assert_eq!(page.hits.len(), 1);
    assert_eq!(page.hits[0].url, "https://tavily.com");
    assert_eq!(page.hits[0].snippet, "Search API for AI agents");
    assert_eq!(
        script.auth_seen.lock().unwrap().last().map(String::as_str),
        Some("Bearer tvly-test-key")
    );
    assert_eq!(script.hits.load(std::sync::atomic::Ordering::Relaxed), 1);

    // 429 + Retry-After: 1 → 单次退避后重试，第二次 200（Retry-After 秒数
    // 粒度最小可测值为 1，缺省 5s 的封顶逻辑由分类核覆盖）。
    let script = TavilyScript::new(vec![(
        StatusCode::TOO_MANY_REQUESTS,
        r#"{"detail":"Rate limit"}"#,
    )]);
    let base = spawn_tavily_fake_with_retry_after(script.clone(), 1).await;
    let page = tavily_provider(format!("{base}/search"), "tvly-test-key")
        .search("rust search", 10, &token)
        .await
        .expect("退避重试后应成功");
    assert_eq!(page.channel, "tavily");
    assert_eq!(
        script.hits.load(std::sync::atomic::Ordering::Relaxed),
        2,
        "429 后应重试一次（共 2 次请求）"
    );

    // 401：分类为 Key 无效，不重试。
    let script = TavilyScript::new(vec![(
        StatusCode::UNAUTHORIZED,
        r#"{"detail":{"error":"Invalid API key"}}"#,
    )]);
    let base = spawn_tavily_fake(script.clone()).await;
    let err = tavily_provider(format!("{base}/search"), "bad-key")
        .search("rust", 10, &token)
        .await
        .expect_err("401 应失败");
    assert!(
        err.message.contains("Key 无效"),
        "401 应分类为 Key 无效：{}",
        err.message
    );
    assert_eq!(
        script.hits.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "401 不应重试"
    );

    // 432 配额尽：分类为 QuotaExhausted，不重试——用尽即当月停用。
    let script = TavilyScript::new(vec![(
        StatusCode::from_u16(432).unwrap(),
        r#"{"detail":"Forbidden: monthly quota exceeded"}"#,
    )]);
    let base = spawn_tavily_fake(script.clone()).await;
    let err = tavily_provider(format!("{base}/search"), "tvly-test-key")
        .search("rust", 10, &token)
        .await
        .expect_err("配额尽应失败");
    assert!(
        err.message.contains("配额"),
        "432 应分类为配额尽：{}",
        err.message
    );
    assert_eq!(
        script.hits.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "配额尽不应重试"
    );

    // 分类核向量表：401/403/429/432/429+quota/500。
    assert_eq!(tavily_classify(401, "{}"), TavilyErrorKind::InvalidKey);
    assert_eq!(tavily_classify(403, "{}"), TavilyErrorKind::Forbidden);
    assert_eq!(tavily_classify(429, "{}"), TavilyErrorKind::RateLimited);
    assert_eq!(
        tavily_classify(429, r#"{"detail":"quota exceeded"}"#),
        TavilyErrorKind::QuotaExhausted
    );
    assert_eq!(
        tavily_classify(432, r#"{"detail":"plan limit reached"}"#),
        TavilyErrorKind::QuotaExhausted
    );
    assert_eq!(tavily_classify(500, "oops"), TavilyErrorKind::RequestFailed);
}

/// Tavily 假服务变体：429 响应带 Retry-After 头（秒），后续请求回 200。
async fn spawn_tavily_fake_with_retry_after(script: Arc<TavilyScript>, retry_after: u64) -> String {
    let app = axum::Router::new().route(
        "/search",
        axum::routing::post(move |_req: Request| {
            let script = script.clone();
            async move {
                script
                    .hits
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let mut queue = script.responses.lock().unwrap();
                if queue.is_empty() {
                    return (
                        StatusCode::OK,
                        serde_json::json!({"results": [{"title": "T", "url": "https://t.dev", "content": "c"}]})
                            .to_string(),
                    )
                        .into_response();
                }
                let (status, body) = queue.remove(0);
                (
                    status,
                    [("retry-after", retry_after.to_string())],
                    body.to_string(),
                )
                    .into_response()
            }
        }),
    );
    spawn_local_server(app).await
}

// ---- 优先级 e2e：启用/停用 SearXNG 同查询走不同通道 ----

/// 组装 SearchEnv（测试构造：全部端点可注入、节流 1ms）。
fn env_with(
    searxng_base: Option<String>,
    ddg_endpoint: Option<String>,
    tavily_endpoint: Option<String>,
    tavily_key: Option<String>,
) -> SearchEnv {
    SearchEnv {
        searxng: searxng_base.map(|base| SearxngConfig {
            enabled: true,
            base_url: base,
            max_results: 10,
        }),
        tavily_api_key: tavily_key,
        max_results_default: 10,
        policy: local_policy(),
        proxy: None,
        throttle: fast_throttle(),
        ddg_endpoint,
        tavily_endpoint,
    }
}

/// 测试辅助：执行一次搜索并取 Done 形态。
async fn search_once(env: &SearchEnv, query: &str) -> crate::tools::ToolOutcome {
    let token = CancellationToken::new();
    let exit = execute(
        env,
        10,
        &WebSearchArgs {
            query: query.to_string(),
            limit: None,
        },
        &token,
    )
    .await;
    let SearchExit::Done(outcome) = exit else {
        panic!("不应取消");
    };
    *outcome
}

// 断言：优先级链 e2e 与参数/配置校验矩阵——启用 SearXNG 本地通道获胜（后续
// 通道不参与）、停用后落 DDG、DDG 改版自动落 Tavily、Tavily 未配 Key 不参与
// （真空结果不算失败）、三层全败给结构化错误（逐层原因 + 配置引导 +
// detail.attempts）；空查询 E_INVALID_ARG 预检；配置归一化（baseUrl 校验、
// maxResults 区间、空白 tavilyKey 归 None）。
#[tokio::test]
async fn priority_chain_and_config_matrix() {
    let web_args = WebSearchArgs {
        query: "rust async".to_string(),
        limit: None,
    };

    // ① 启用 SearXNG（假服务正常）：同查询走本地通道，后续通道不参与。
    let searxng = spawn_searxng_fake(FakeMode::Normal).await;
    let script = TavilyScript::new(vec![]);
    let tavily = spawn_tavily_fake(script.clone()).await;
    let env = env_with(
        Some(searxng),
        Some("http://127.0.0.1:9/html/".to_string()), // DDG 假失败（不可达也不该被调）
        Some(format!("{tavily}/search")),
        Some("tvly-test-key".to_string()),
    );
    let outcome = search_once(&env, &web_args.query).await;
    assert!(outcome.ok, "SearXNG 正常时应成功：{outcome:?}");
    let data = outcome.data.expect("成功应有 data");
    assert_eq!(data["channel"], "searxng", "启用 SearXNG 应走本地通道");
    assert_eq!(
        script.hits.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "通道 1 成功时 Tavily 不应参与"
    );

    // ② 停用 SearXNG：同查询落通道 2（本地 DDG 假服务回放快照页）。
    let ddg = spawn_local_server(axum::Router::new().route(
        "/html/",
        axum::routing::get(|| async { (StatusCode::OK, DDG_RESULTS) }),
    ))
    .await;
    let env = env_with(
        None,
        Some(format!("{ddg}/html/")),
        Some("http://127.0.0.1:9/search".to_string()),
        Some("tvly-test-key".to_string()),
    );
    let outcome = search_once(&env, &web_args.query).await;
    assert!(outcome.ok, "DDG 正常时应成功：{outcome:?}");
    let data = outcome.data.expect("成功应有 data");
    assert_eq!(data["channel"], "ddg", "停用 SearXNG 应走内置抓取");
    assert_eq!(data["count"], 3);

    // ③ DDG 改版页（疑似告警=通道失败）+ Tavily Key 已配：自动落 Tavily。
    let ddg = spawn_local_server(axum::Router::new().route(
        "/html/",
        axum::routing::get(|| async { (StatusCode::OK, DDG_REDESIGN) }),
    ))
    .await;
    let script = TavilyScript::new(vec![]);
    let tavily = spawn_tavily_fake(script.clone()).await;
    let env = env_with(
        None,
        Some(format!("{ddg}/html/")),
        Some(format!("{tavily}/search")),
        Some("tvly-test-key".to_string()),
    );
    let outcome = search_once(&env, &web_args.query).await;
    assert!(outcome.ok, "Tavily 兜底应成功：{outcome:?}");
    let data = outcome.data.expect("成功应有 data");
    assert_eq!(data["channel"], "tavily", "抓取失败应自动落 Tavily");

    // ④ Tavily 未配 Key：不参与（真空结果不是失败），全败错误如实记录前
    //    先验证真空结果不落 Tavily。
    let ddg = spawn_local_server(axum::Router::new().route(
        "/html/",
        axum::routing::get(|| async { (StatusCode::OK, DDG_EMPTY) }),
    ))
    .await;
    let script = TavilyScript::new(vec![]);
    let tavily = spawn_tavily_fake(script.clone()).await;
    let env = env_with(
        None,
        Some(format!("{ddg}/html/")),
        Some(format!("{tavily}/search")),
        None, // 未配 Key
    );
    let outcome = search_once(&env, &web_args.query).await;
    assert!(outcome.ok, "真空结果应成功：{outcome:?}");
    let data = outcome.data.expect("成功应有 data");
    assert_eq!(data["channel"], "ddg");
    assert_eq!(data["count"], 0);
    assert_eq!(
        script.hits.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "真空结果不应触发 Tavily"
    );

    // ⑤ 三层全失败：结构化错误（逐层原因 + 配置引导），不崩不挂。
    let searxng = spawn_searxng_fake(FakeMode::Forbidden).await; // ①失败
    let ddg = spawn_local_server(axum::Router::new().route(
        "/html/",
        axum::routing::get(|| async { (StatusCode::OK, DDG_REDESIGN) }),
    ))
    .await; // ②失败（疑似改版）
    let script = TavilyScript::new(vec![(
        StatusCode::UNAUTHORIZED,
        r#"{"detail":"Invalid API key"}"#,
    )]);
    let tavily = spawn_tavily_fake(script.clone()).await; // ③失败（Key 无效）
    let env = env_with(
        Some(searxng),
        Some(format!("{ddg}/html/")),
        Some(format!("{tavily}/search")),
        Some("bad-key".to_string()),
    );
    let outcome = search_once(&env, &web_args.query).await;
    assert!(!outcome.ok, "三层全败应失败");
    let error = outcome.error.expect("失败应有 error");
    assert_eq!(error.code, "E_SEARCH_UNAVAILABLE");
    // message 带逐层失败原因与配置引导（模型通道可读）
    assert!(error.message.contains("SearXNG"), "message 应含通道①原因");
    assert!(
        error.message.contains("DuckDuckGo"),
        "message 应含通道②原因"
    );
    assert!(error.message.contains("Tavily"), "message 应含通道③原因");
    assert!(error.message.contains("tavily.com"), "message 应含配置引导");
    // detail.attempts 结构化（UI 通道）
    let detail = outcome.detail.expect("失败应带 detail");
    let attempts = detail["attempts"].as_array().expect("attempts 数组");
    assert_eq!(attempts.len(), 3);
    assert_eq!(attempts[0]["channel"], "searxng");
    assert_eq!(attempts[0]["tried"], true);
    assert!(attempts[0]["error"].is_string());
    assert_eq!(attempts[2]["channel"], "tavily");

    // ⑥ 参数预检：空查询不进通道链。
    let env = env_with(None, None, None, None);
    let outcome = search_once(&env, "   ").await;
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("应有错误").code, "E_INVALID_ARG");

    // ⑦ 配置归一化（同步面）：searxng.baseUrl 校验 + maxResults 区间 +
    //    tavilyKey 空白归 None。
    use crate::providers::types::AppConfig;
    let mut config = AppConfig::default();
    config.search.searxng.base_url = "http://127.0.0.1:8888/".to_string();
    config.search.searxng.max_results = 10;
    config.search.tavily_api_key = Some("  ".to_string());
    config.normalize().expect("合法配置应通过");
    assert_eq!(config.search.searxng.base_url, "http://127.0.0.1:8888");
    assert_eq!(config.search.tavily_api_key, None, "空白 Key 归 None");

    let mut bad = AppConfig::default();
    bad.search.searxng.max_results = 0;
    assert!(bad.normalize().is_err(), "maxResults=0 应拒绝");

    let mut bad = AppConfig::default();
    bad.search.searxng.max_results = 31;
    assert!(bad.normalize().is_err(), "maxResults=31 应拒绝");

    let mut bad = AppConfig::default();
    bad.search.searxng.base_url = "ftp://x".to_string();
    assert!(bad.normalize().is_err(), "非 http(s) 地址应拒绝");
}
