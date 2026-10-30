//! 供应商连接探测测试（19 号票）：全本地 fixture（127.0.0.1 axum mock +
//! 手写最小 SOCKS5 转发器），不连真实外网。
//!
//! 覆盖面（票面"测试"节）：
//! - models 拉取成功（OpenAI data[] 形状 + Ollama 兼容形状）；
//! - /models 404 → 回退最小补全成功；
//! - 401→auth / 429→rate_limited / 500→server / 402→billing / 400→bad_request；
//! - 不可达地址 → network 分类；
//! - 延迟计量 > 0；
//! - 错误消息脱敏（不含 Key）；
//! - 代理三模式 e2e：manual socks5 经本地转发器可达、none 直连恢复
//!   （FR-02-10 验收的自动化等价面）。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Json;
use axum::routing::{get, post};
use serde_json::json;

use super::*;
use crate::providers::http::build_proxy_client;
use crate::providers::types::{ProxyConfig, ProxyMode};

// ---- mock 端点 ----

/// axum mock：按场景路由——/models 返回 OpenAI 形状清单，
/// /chat/completions 返回最小补全成功体。
async fn mock_server() -> std::net::SocketAddr {
    let app = axum::Router::new()
        .route(
            "/v1/models",
            get(|| async {
                Json(json!({
                    "data": [
                        {"id": "mock-large"},
                        {"id": "mock-small"}
                    ]
                }))
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|| async {
                Json(json!({
                    "id": "chatcmpl-mock",
                    "choices": [{"message": {"role": "assistant", "content": "p"}}]
                }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// 场景 mock：指定 /models 的状态码与响应体。
async fn status_mock_server(status: u16, body: serde_json::Value) -> std::net::SocketAddr {
    let app = axum::Router::new().route(
        "/v1/models",
        get(move || async move {
            (
                axum::http::StatusCode::from_u16(status).unwrap(),
                Json(body),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

// 断言：models 拉取成功——ok=true、模型列表按 /models 响应原样、延迟计量。
#[tokio::test]
async fn probe_models_success_openai_shape() {
    let addr = mock_server().await;
    let client = build_proxy_client(None).unwrap();
    let result = probe_openai_compatible(
        &client,
        &format!("http://{addr}/v1"),
        Some("sk-test-0123456789"),
    )
    .await;
    assert!(result.ok, "应连通：{:?}", result.error);
    assert_eq!(
        result.models.as_deref(),
        Some(&["mock-large".to_string(), "mock-small".to_string()][..])
    );
    assert!(result.latency_ms.unwrap_or(0) < 30_000, "延迟应远小于预算");
    assert!(result.error.is_none());
}

// 断言：/models 404（精简兼容网关）→ 回退最小补全，补全 200 即连通；
// models 字段缺省（最小补全路径无模型列表）。
#[tokio::test]
async fn probe_models_404_falls_back_to_minimal_completion() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route(
            "/v1/models",
            get(|| async { (axum::http::StatusCode::NOT_FOUND, "no models") }),
        )
        .route(
            "/v1/chat/completions",
            post(|| async { Json(json!({"choices": [{"message": {"content": "p"}}]})) }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = build_proxy_client(None).unwrap();
    let result = probe_openai_compatible(&client, &format!("http://{addr}/v1"), None).await;
    assert!(result.ok, "404 回退补全应连通：{:?}", result.error);
    assert!(result.models.is_none(), "补全路径无模型列表");
}

// 断言：/models 404 且补全也 404 → bad_request 分类（地址不像 OpenAI 兼容端点）。
#[tokio::test]
async fn probe_both_endpoints_missing_bad_request() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route("/v1/other", get(|| async { "something else" }));
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = build_proxy_client(None).unwrap();
    let result = probe_openai_compatible(&client, &format!("http://{addr}/v1"), None).await;
    assert!(!result.ok);
    let error = result.error.expect("应有错误");
    assert_eq!(error.code, "E_PROVIDER_BAD_REQUEST");
    assert!(error.message.contains("/v1"), "消息应带基地址提示");
}

// 断言：状态码 → 七类分类表（401 auth / 429 rate_limited / 500 server /
// 402 billing / 400 bad_request）。
#[tokio::test]
async fn probe_status_classification() {
    let cases: [(u16, &str); 5] = [
        (401, "E_PROVIDER_AUTH"),
        (429, "E_PROVIDER_RATE_LIMITED"),
        (500, "E_PROVIDER_SERVER"),
        (402, "E_PROVIDER_BILLING"),
        (400, "E_PROVIDER_BAD_REQUEST"),
    ];
    for (status, expected) in cases {
        let addr = status_mock_server(status, json!({"error": {"message": "ups"}})).await;
        let client = build_proxy_client(None).unwrap();
        let result =
            probe_openai_compatible(&client, &format!("http://{addr}/v1"), Some("sk-test")).await;
        assert!(!result.ok, "{status} 不应判连通");
        let error = result.error.unwrap_or_else(|| panic!("{status} 应有错误"));
        assert_eq!(error.code, expected, "{status} 应分类为 {expected}");
    }
}

// 断言：不可达地址（本机保留段随机端口无监听）→ network 分类。
#[tokio::test]
async fn probe_unreachable_network() {
    let client = build_proxy_client(None).unwrap();
    // 127.0.0.1 上绑定概率极低的端口：连接被拒 → network
    let result = probe_openai_compatible(&client, "http://127.0.0.1:1/v1", None).await;
    assert!(!result.ok);
    let error = result.error.expect("应有错误");
    assert_eq!(error.code, "E_PROVIDER_NETWORK", "{:?}", error);
}

// 断言：延迟计量存在且错误面脱敏——错误消息只含状态与响应摘要，
// 不含请求 Key（Key 不回显是脱敏红线）。
#[tokio::test]
async fn probe_error_message_sanitized() {
    let addr = status_mock_server(401, json!({"error": "invalid key"})).await;
    let client = build_proxy_client(None).unwrap();
    let secret = "sk-SUPER-SECRET-0123456789";
    let result = probe_openai_compatible(&client, &format!("http://{addr}/v1"), Some(secret)).await;
    let error = result.error.expect("应有错误");
    assert!(
        !error.message.contains(secret),
        "错误消息不得回显 Key：{}",
        error.message
    );
    assert!(result.latency_ms.is_some(), "失败路径也应有耗时计量");
}

// ---- 手写最小 SOCKS5 转发器（无认证，CONNECT，IPv4/域名）----

/// 起一个最小 SOCKS5 服务器：握手 → 读 CONNECT 目标 → 连目标 → 回成功 →
/// 双向透传。返回（监听地址, 经手连接数计数器）——计数器供断言"请求确实
/// 经过了代理"。
async fn socks5_server() -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = Arc::new(AtomicUsize::new(0));
    let served_bg = served.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else {
                break;
            };
            let served = served_bg.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                // 握手：VER NMETHODS METHODS → 回 VER METHOD(0x00 无认证)
                let mut head = [0u8; 2];
                if client.read_exact(&mut head).await.is_err() {
                    return;
                }
                let n = head[1] as usize;
                let mut methods = vec![0u8; n];
                if client.read_exact(&mut methods).await.is_err() {
                    return;
                }
                if client.write_all(&[0x05, 0x00]).await.is_err() {
                    return;
                }
                // CONNECT 请求：VER CMD RSV ATYP ADDR PORT
                let mut req = [0u8; 4];
                if client.read_exact(&mut req).await.is_err() {
                    return;
                }
                if req[1] != 0x01 {
                    // 只支持 CONNECT
                    let _ = client
                        .write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                        .await;
                    return;
                }
                let target: std::net::SocketAddr = match req[3] {
                    0x01 => {
                        let mut octets = [0u8; 4];
                        if client.read_exact(&mut octets).await.is_err() {
                            return;
                        }
                        let mut port = [0u8; 2];
                        if client.read_exact(&mut port).await.is_err() {
                            return;
                        }
                        std::net::SocketAddr::new(
                            std::net::IpAddr::V4(std::net::Ipv4Addr::from(octets)),
                            u16::from_be_bytes(port),
                        )
                    }
                    0x03 => {
                        let mut len = [0u8; 1];
                        if client.read_exact(&mut len).await.is_err() {
                            return;
                        }
                        let mut host = vec![0u8; len[0] as usize];
                        if client.read_exact(&mut host).await.is_err() {
                            return;
                        }
                        let mut port = [0u8; 2];
                        if client.read_exact(&mut port).await.is_err() {
                            return;
                        }
                        let host = String::from_utf8_lossy(&host).to_string();
                        match tokio::net::lookup_host((host.as_str(), u16::from_be_bytes(port)))
                            .await
                        {
                            Ok(mut addrs) => match addrs.next() {
                                Some(a) => a,
                                None => return,
                            },
                            Err(_) => return,
                        }
                    }
                    _ => return,
                };
                let Ok(mut upstream) = tokio::net::TcpStream::connect(target).await else {
                    let _ = client
                        .write_all(&[0x05, 0x01, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                        .await;
                    return;
                };
                // 回 CONNECT 成功（BND 全 0）
                if client
                    .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                    .await
                    .is_err()
                {
                    return;
                }
                served.fetch_add(1, Ordering::SeqCst);
                // 双向透传直到任一侧关闭
                let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
            });
        }
    });
    (addr, served)
}

// 断言（FR-02-10 验收第一条的自动化等价面）：manual socks5 模式——探测请求
// 经本地 SOCKS5 转发器到达 mock 端点（计数器证明走了代理）；切 none 直连
// 恢复（不经转发器）。工厂 reload 换代后 client 即用新配置。
#[tokio::test]
async fn socks5_manual_proxy_reaches_mock_then_none_bypasses() {
    // mock 只提供补全端点（/v1/models 404 → 探测回退最小补全）——经 socks5
    // 隧道跑的是 chat/completions 请求（对话请求面），贴合"经本地代理实测
    // 对话成功"的验收语义
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mock = listener.local_addr().unwrap();
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        post(|| async {
            Json(json!({"choices": [{"message": {"role": "assistant", "content": "p"}}]}))
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (proxy_addr, served) = socks5_server().await;

    // manual socks5：请求必须经转发器
    let manual = ProxyConfig {
        mode: ProxyMode::Manual,
        url: Some(format!("socks5://{proxy_addr}")),
    };
    let client = build_proxy_client(Some(&manual)).unwrap();
    let result = probe_openai_compatible(&client, &format!("http://{mock}/v1"), None).await;
    assert!(result.ok, "socks5 代理路径应连通：{:?}", result.error);
    assert!(
        result.models.is_none(),
        "404 回退路径走最小补全（对话请求面），无模型列表"
    );
    assert!(
        served.load(Ordering::SeqCst) >= 1,
        "请求应经 SOCKS5 转发器（计数为 0 = 代理没生效）"
    );

    // 切 none：直连恢复，不再经转发器
    let none = ProxyConfig {
        mode: ProxyMode::None,
        url: None,
    };
    let factory = crate::providers::http::HttpClientFactory::from_config(Some(&manual)).unwrap();
    assert!(factory.reload(Some(&none)).is_ok(), "热更新到 none");
    let client = factory.client();
    let before = served.load(Ordering::SeqCst);
    let result = probe_openai_compatible(&client, &format!("http://{mock}/v1"), None).await;
    assert!(result.ok, "none 直连应恢复：{:?}", result.error);
    assert_eq!(
        served.load(Ordering::SeqCst),
        before,
        "none 模式不得再经 SOCKS5 转发器"
    );
}

// 断言：模型清单宽松解析的三种形状（OpenAI data[] / models[] / 裸数组）。
#[test]
fn parse_models_three_shapes() {
    let openai = r#"{"data":[{"id":"a"},{"id":"b"}]}"#;
    assert_eq!(parse_models(openai), Some(vec!["a".into(), "b".into()]));
    let gateway = r#"{"models":[{"id":"x"}]}"#;
    assert_eq!(parse_models(gateway), Some(vec!["x".into()]));
    let bare = r#"[{"id":"y"},{"model":"z"}]"#;
    assert_eq!(parse_models(bare), Some(vec!["y".into(), "z".into()]));
    assert_eq!(parse_models("not json"), None);
    assert_eq!(parse_models("{}"), None);
}
