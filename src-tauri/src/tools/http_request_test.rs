//! http_request 工具测试（12 号票）：方法白名单、§8.8 响应 spill 治理、
//! 自定义请求头透传与规格 schema 面。
//!
//! 集成用例经本地 fixture HTTP server（127.0.0.1 随机端口、零外网）+
//! 例外策略（回环放行）走完整 execute 链路。

use std::collections::BTreeMap;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::http_request::{ALLOWED_METHODS, HttpRequestArgs, RequestExit, execute, spec};
use crate::safety::ssrf::{HostThrottle, SsrfPolicy};
use crate::tools::{ToolsContext, command::SPILL_THRESHOLD};

/// 例外策略（本地 fixture 在回环段，测试需要放行）。
fn permissive() -> SsrfPolicy {
    SsrfPolicy {
        allow_private_network: true,
    }
}

/// 测试执行环境：临时数据根 + 零节流。
fn fixture_ctx(dir: &tempfile::TempDir) -> ToolsContext {
    ToolsContext::new(dir.path().to_path_buf(), dir.path().to_path_buf())
}

fn no_throttle() -> HostThrottle {
    HostThrottle::new(Duration::ZERO)
}

/// 起本地 axum fixture（127.0.0.1 随机端口）。
async fn spawn_fixture(routes: axum::Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地回环端口");
    let addr = listener.local_addr().expect("端口");
    tokio::spawn(async move {
        let _ = axum::serve(listener, routes).await;
    });
    addr
}

/// execute 的收口提取（Done → ToolOutcome；Cancelled 即 panic）。
async fn run(
    ctx: &ToolsContext,
    throttle: &HostThrottle,
    args: &HttpRequestArgs,
) -> crate::tools::ToolOutcome {
    let token = CancellationToken::new();
    match execute(ctx, permissive(), None, throttle, "call-test", args, &token).await {
        RequestExit::Done(outcome) => *outcome,
        RequestExit::Cancelled { .. } => panic!("测试路径不应取消"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn get_round_trips_status_headers_and_body() {
    let app = axum::Router::new().route(
        "/ping",
        axum::routing::get(|| async {
            (
                axum::http::StatusCode::CREATED,
                [("x-probe", "1")],
                "响应体内容",
            )
        }),
    );
    let addr = spawn_fixture(app).await;
    let dir = tempfile::tempdir().expect("临时目录");
    let outcome = run(
        &fixture_ctx(&dir),
        &no_throttle(),
        &serde_json::from_value::<HttpRequestArgs>(serde_json::json!({
            "method": "get",
            "url": format!("http://{addr}/ping"),
        }))
        .expect("参数"),
    )
    .await;
    assert!(outcome.ok, "GET 应成功：{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["status"], 201);
    assert_eq!(data["headers"]["x-probe"], "1");
    assert_eq!(data["body"], "响应体内容");
    assert_eq!(data["bodyTruncated"], false);
    assert!(data["spillPath"].is_null());
}

#[tokio::test(flavor = "current_thread")]
async fn post_carries_body_and_custom_headers() {
    // POST 回显：服务端断言收到的 method/body/自定义头并回显
    use axum::http::HeaderMap as AxumHeaders;
    let seen: std::sync::Arc<std::sync::Mutex<Option<(String, String, String)>>> =
        Default::default();
    let capture = seen.clone();
    let app = axum::Router::new().route(
        "/echo",
        axum::routing::post(
            |method: axum::http::Method, headers: AxumHeaders, body: String| async move {
                let custom = headers
                    .get("x-custom")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_owned();
                *capture.lock().unwrap() = Some((method.to_string(), body, custom));
                "已收到"
            },
        ),
    );
    let addr = spawn_fixture(app).await;
    let dir = tempfile::tempdir().expect("临时目录");
    let outcome = run(
        &fixture_ctx(&dir),
        &no_throttle(),
        &serde_json::from_value::<HttpRequestArgs>(serde_json::json!({
            "method": "POST",
            "url": format!("http://{addr}/echo"),
            "headers": { "x-custom": "cw-test", "content-type": "text/plain" },
            "body": "请求体文本",
        }))
        .expect("参数"),
    )
    .await;
    assert!(outcome.ok, "POST 应成功：{outcome:?}");
    let (method, body, custom) = seen.lock().unwrap().clone().expect("服务端应收到请求");
    assert_eq!(method, "POST");
    assert_eq!(body, "请求体文本");
    assert_eq!(custom, "cw-test");
}

#[tokio::test(flavor = "current_thread")]
async fn oversized_body_spills_with_tail() {
    // §8.8：>96KB 全量 spill 至 <data>/logs/spill/<callId>.txt + 尾 16KB
    let served: String = "中".repeat(SPILL_THRESHOLD); // 96KB+ 的多字节文本
    assert!(served.len() > SPILL_THRESHOLD);
    let for_handler = served.clone();
    let app = axum::Router::new().route(
        "/big",
        axum::routing::get(move || {
            let body = for_handler.clone();
            async move { body }
        }),
    );
    let addr = spawn_fixture(app).await;
    let dir = tempfile::tempdir().expect("临时目录");
    let outcome = run(
        &fixture_ctx(&dir),
        &no_throttle(),
        &serde_json::from_value::<HttpRequestArgs>(serde_json::json!({
            "method": "GET",
            "url": format!("http://{addr}/big"),
        }))
        .expect("参数"),
    )
    .await;
    assert!(outcome.ok, "spill 情形请求本身应成功：{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["bodyTruncated"], true);
    let spill_path = data["spillPath"].as_str().expect("应带 spill 路径");
    // 全量落盘字节 = 服务器发的全部
    let spilled = std::fs::read_to_string(spill_path).expect("spill 文件可读");
    assert_eq!(spilled.len(), served.len(), "spill 应承载全量响应体");
    // 结果 body 只带尾部 16KB（§8.8；按 UTF-8 字符边界取整，多字节文本
    // 可能略短于 16KB 字节）
    let tail = data["body"].as_str().expect("body");
    assert!(tail.len() <= crate::tools::command::TAIL_BYTES);
    assert!(
        served.ends_with(tail),
        "body 应为响应体尾部（字符边界取整）"
    );
    // spill 文件落在数据根的 logs/spill/ 下（§7.1/§8.8）
    assert!(spill_path.contains("logs/spill/"));
}

#[test]
fn method_whitelist_rejects_others() {
    // 白名单外方法在参数预检即拒（不发请求）
    for method in ["CONNECT", "TRACE", "OPTIONS", "get;", ""] {
        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let dir = tempfile::tempdir().unwrap();
                run(
                    &fixture_ctx(&dir),
                    &no_throttle(),
                    &HttpRequestArgs {
                        method: method.to_owned(),
                        url: "http://127.0.0.1:1/".to_owned(),
                        headers: None,
                        body: None,
                    },
                )
                .await
            });
        assert!(!outcome.ok, "方法 {method:?} 应拒绝");
        assert_eq!(outcome.error.expect("error").code, "E_INVALID_ARG");
    }
    // 白名单内方法（大小写不敏感）可解析
    assert_eq!(ALLOWED_METHODS.len(), 6);
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_header_name_or_value_rejected() {
    let dir = tempfile::tempdir().expect("临时目录");
    // 非法头名（含空格）
    let outcome = run(
        &fixture_ctx(&dir),
        &no_throttle(),
        &HttpRequestArgs {
            method: "GET".to_owned(),
            url: "http://127.0.0.1:1/".to_owned(),
            headers: Some(BTreeMap::from([("bad name".to_owned(), "v".to_owned())])),
            body: None,
        },
    )
    .await;
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("error").code, "E_INVALID_ARG");
    // 非法头值（含换行注入）
    let outcome = run(
        &fixture_ctx(&dir),
        &no_throttle(),
        &HttpRequestArgs {
            method: "GET".to_owned(),
            url: "http://127.0.0.1:1/".to_owned(),
            headers: Some(BTreeMap::from([(
                "x-a".to_owned(),
                "v\ninjected".to_owned(),
            )])),
            body: None,
        },
    )
    .await;
    assert!(!outcome.ok);
}

#[test]
fn spec_is_registered_as_network_tool() {
    let spec = spec();
    assert!(crate::tools::STATIC_TOOL_NAMES.contains(&spec.name.as_str()));
    assert!(matches!(
        spec.capability,
        crate::safety::tier::Capability::Network
    ));
    // schema 形状（与前端 CW_TOOL_PARAMETERS 逐字镜像的源）
    let required = spec.params_schema["required"].as_array().expect("required");
    assert!(required.contains(&serde_json::json!("method")));
    assert!(required.contains(&serde_json::json!("url")));
    let methods = spec.params_schema["properties"]["method"]["enum"]
        .as_array()
        .expect("enum");
    assert_eq!(methods.len(), 6);
    assert!(!spec.annotations.read_only_hint, "POST 有副作用，非只读");
    assert!(spec.annotations.open_world_hint);
}
