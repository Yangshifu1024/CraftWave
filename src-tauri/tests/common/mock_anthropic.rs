//! 本地 Anthropic Messages 兼容 mock SSE 服务：脚本化输出
//! Anthropic 流协议事件序列（thinking delta / text delta / tool_use 的
//! input_json_delta 增量含分段 JSON），无网络、无 API key。仅测试编译
//! 单元（tests/）可见，不进产物。

// 各集成测试二进制独立链接本模块，未用到的导出属预期。
#![allow(dead_code)]

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// mock 每个事件的默认间隔（对齐 mock_openai 的 30ms 节奏）。
pub const CHUNK_GAP: Duration = Duration::from_millis(30);

/// 脚本第 1 轮的思考文本（两段 delta 拼接）。
pub const TURN1_REASONING: &str = "用户要我看配置，先调用 read。";
/// 脚本第 1 轮的正文文本（两段 delta 拼接）。
pub const TURN1_TEXT: &str = "我来读一下配置文件。";
/// 脚本第 1 轮的工具调用（id / 名称）。
pub const TURN1_TOOL_ID: &str = "toolu_mock_1";
pub const TURN1_TOOL_NAME: &str = "read";
/// 工具入参（分段流式发出，第二段结尾是半截 JSON）。
pub const TURN1_TOOL_ARG_PARTS: [&str; 3] = ["{\"path\": \"config", ".toml\", \"off", "set\": 0}"];
/// 脚本第 2 轮的最终答复。
pub const TURN2_TEXT: &str = "配置文件共 12 行，端口是 8087。";

/// mock 记录的流事件发送情况（端到端断言用）。
#[derive(Debug, Clone)]
pub struct SentEvent {
    /// 相对首个请求收到时的偏移。
    pub at: Duration,
    /// Anthropic 事件的 type 字段（message_start / content_block_delta / …）。
    pub kind: String,
    /// 事件负载 JSON。
    pub payload: Value,
}

/// 运行中的 mock 服务句柄。
pub struct MockAnthropic {
    /// rig AnthropicConfig 的 base_url（不含 /v1——Anthropic wire 自己拼
    /// /v1/messages；与 OpenAI mock 的含 /v1 base 不同）。
    pub base_url: String,
    pub sent: Arc<Mutex<Vec<SentEvent>>>,
    /// mock 收到的每个 messages 请求体（thinking/max_tokens/回喂断言用）。
    pub requests: Arc<Mutex<Vec<Value>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    server: JoinHandle<()>,
}

/// 一条脚本事件（Anthropic 流协议的 data 负载）。
pub type ScriptEvent = Value;

impl MockAnthropic {
    /// 启动 mock（随机端口），默认两轮脚本。
    pub async fn start() -> Self {
        Self::start_scripts(CHUNK_GAP).await
    }

    async fn start_scripts(gap: Duration) -> Self {
        let sent: Arc<Mutex<Vec<SentEvent>>> = Arc::default();
        let requests: Arc<Mutex<Vec<Value>>> = Arc::default();
        let route_sent = sent.clone();
        let route_requests = requests.clone();
        let app = Router::new()
            .route(
                "/v1/messages",
                post(
                    move |headers: axum::http::HeaderMap, Json(body): Json<Value>| {
                        let sent = route_sent.clone();
                        let requests = route_requests.clone();
                        async move { handle_messages(headers, body, sent, requests, gap).await }
                    },
                ),
            )
            .route("/v1/models", get(get_models));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock");
        let addr = listener.local_addr().expect("mock addr");
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let graceful = axum::serve(listener, app).with_graceful_shutdown(async {
                let _ = rx.await;
            });
            let _ = graceful.await;
        });
        Self {
            // Anthropic 官方基地址形态（无 /v1；rig wire 自己拼 /v1/messages）
            base_url: format!("http://{addr}"),
            sent,
            requests,
            shutdown: Some(tx),
            server,
        }
    }

    pub async fn sent_events(&self) -> Vec<SentEvent> {
        self.sent.lock().await.clone()
    }

    pub async fn request_bodies(&self) -> Vec<Value> {
        self.requests.lock().await.clone()
    }
}

impl Drop for MockAnthropic {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.server.abort();
    }
}

async fn get_models() -> Json<Value> {
    Json(json!({"data": [{"id": "claude-mock", "type": "model"}]}))
}

/// 判断请求轮次：messages 里出现 tool_result 内容块即第 2 轮（工具结果已回喂）。
fn has_tool_result(body: &Value) -> bool {
    body["messages"]
        .as_array()
        .map(|msgs| {
            msgs.iter().any(|m| {
                m["content"]
                    .as_array()
                    .map(|blocks| blocks.iter().any(|b| b["type"] == "tool_result"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// SSE data 行。
fn sse(event: Value) -> String {
    format!("data: {}\n\n", event)
}

/// message_start 事件（usage 的 input 计数从这里来）。
fn message_start() -> Value {
    json!({
        "type": "message_start",
        "message": {
            "id": "msg_mock",
            "type": "message",
            "role": "assistant",
            "model": "claude-mock",
            "content": [],
            "stop_reason": null,
            "usage": {"input_tokens": 10, "output_tokens": 1}
        }
    })
}

/// 思考块开启 + 两段 thinking_delta + 收口。
fn thinking_events(text_parts: [&str; 2]) -> Vec<Value> {
    let mut events = vec![json!({
        "type": "content_block_start",
        "index": 0,
        "content_block": {"type": "thinking", "thinking": ""}
    })];
    for (i, part) in text_parts.iter().enumerate() {
        events.push(json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "thinking_delta", "thinking": part}
        }));
        let _ = i;
    }
    events.push(json!({"type": "content_block_stop", "index": 0}));
    events
}

/// 文本块开启 + 两段 text_delta + 收口。
fn text_events(index: usize, text_parts: [&str; 2]) -> Vec<Value> {
    let mut events = vec![json!({
        "type": "content_block_start",
        "index": index,
        "content_block": {"type": "text", "text": ""}
    })];
    for part in text_parts {
        events.push(json!({
            "type": "content_block_delta",
            "index": index,
            "delta": {"type": "text_delta", "text": part}
        }));
    }
    events.push(json!({"type": "content_block_stop", "index": index}));
    events
}

/// 工具调用块：tool_use 开启 + input_json_delta 三段（末段前是半截 JSON）
/// + 收口。
fn tool_events(index: usize) -> Vec<Value> {
    let mut events = vec![json!({
        "type": "content_block_start",
        "index": index,
        "content_block": {"type": "tool_use", "id": TURN1_TOOL_ID, "name": TURN1_TOOL_NAME, "input": {}}
    })];
    for part in TURN1_TOOL_ARG_PARTS {
        events.push(json!({
            "type": "content_block_delta",
            "index": index,
            "delta": {"type": "input_json_delta", "partial_json": part}
        }));
    }
    events.push(json!({"type": "content_block_stop", "index": index}));
    events
}

/// message_delta（stop_reason + 产出计数）与 message_stop。
fn finish_events(stop_reason: &str, output_tokens: u64) -> Vec<Value> {
    vec![
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop_reason},
            "usage": {"output_tokens": output_tokens}
        }),
        json!({"type": "message_stop"}),
    ]
}

/// 第 1 轮脚本：message_start → 思考 → 正文 → 工具调用 → tool_use 收尾。
fn turn1_events() -> Vec<ScriptEvent> {
    let mut events = vec![message_start()];
    events.extend(thinking_events(["用户要我看配置，", "先调用 read。"]));
    events.extend(text_events(1, ["我来读一下", "配置文件。"]));
    events.extend(tool_events(2));
    events.extend(finish_events("tool_use", 48));
    events
}

/// 第 2 轮脚本：message_start → 纯文本答复 → end_turn 收尾。
fn turn2_events() -> Vec<ScriptEvent> {
    let mut events = vec![message_start()];
    events.extend(text_events(0, ["配置文件共 12 行，", "端口是 8087。"]));
    events.extend(finish_events("end_turn", 24));
    events
}

/// 非流式（stream != true）请求的固定答复（suggest/压缩摘要等单次调用路径）。
pub const NONSTREAM_TEXT: &str = "试试让我总结配置要点。";

async fn handle_messages(
    headers: axum::http::HeaderMap,
    body: Value,
    sent: Arc<Mutex<Vec<SentEvent>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    gap: Duration,
) -> Response {
    // 鉴权头记录（x-api-key 断言面；rig wire 装配同款）
    let _ = headers.get("x-api-key").and_then(|v| v.to_str().ok());
    requests.lock().await.push(body.clone());
    if body["stream"] != Value::Bool(true) {
        return Json(json!({
            "id": "msg_mock",
            "type": "message",
            "role": "assistant",
            "model": "claude-mock",
            "content": [{"type": "text", "text": NONSTREAM_TEXT}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 8}
        }))
        .into_response();
    }
    let script = if has_tool_result(&body) {
        turn2_events()
    } else {
        turn1_events()
    };
    let started = Instant::now();
    let stream =
        futures::stream::iter(script.into_iter().map(Ok::<_, Infallible>)).then(move |item| {
            let sent = sent.clone();
            async move {
                let event = item?;
                tokio::time::sleep(gap).await;
                sent.lock().await.push(SentEvent {
                    at: started.elapsed(),
                    kind: event["type"].as_str().unwrap_or_default().to_owned(),
                    payload: event.clone(),
                });
                Ok::<_, Infallible>(sse(event))
            }
        });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .expect("mock response")
}
