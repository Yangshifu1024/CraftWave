//! 【38 号票】本地 OpenAI Responses API 兼容 mock SSE 服务：脚本化输出
//! Responses 事件流（reasoning summary delta / output_text delta /
//! function_call_arguments 增量含半截 JSON），无网络、无 API key。
//! 仅测试编译单元（tests/）可见，不进产物。

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

/// 脚本第 1 轮的思考文本（reasoning summary，两段 delta 拼接）。
pub const TURN1_REASONING: &str = "用户要我看配置，先调用 read。";
/// 脚本第 1 轮的正文文本（两段 delta 拼接）。
pub const TURN1_TEXT: &str = "我来读一下配置文件。";
/// 脚本第 1 轮的工具调用（Responses 事件面 call_id / item id / 名称）。
pub const TURN1_CALL_ID: &str = "call_mock_1";
pub const TURN1_ITEM_ID: &str = "fc_mock_1";
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
    /// Responses 事件的 type 字段（response.output_text.delta / …）。
    pub kind: String,
    /// 事件负载 JSON。
    pub payload: Value,
}

/// 运行中的 mock 服务句柄。
pub struct MockResponses {
    /// rig OpenAIConfig 的 base_url（含 /v1，与官方一致）。
    pub base_url: String,
    pub sent: Arc<Mutex<Vec<SentEvent>>>,
    /// mock 收到的每个 responses 请求体（route/回喂断言用）。
    pub requests: Arc<Mutex<Vec<Value>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    server: JoinHandle<()>,
}

/// 一条脚本事件（Responses 流协议的 data 负载）。
pub type ScriptEvent = Value;

impl MockResponses {
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
                "/v1/responses",
                post(move |Json(body): Json<Value>| {
                    let sent = route_sent.clone();
                    let requests = route_requests.clone();
                    async move { handle_responses(body, sent, requests, gap).await }
                }),
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
            base_url: format!("http://{addr}/v1"),
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

impl Drop for MockResponses {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.server.abort();
    }
}

async fn get_models() -> Json<Value> {
    Json(json!({"object": "list", "data": [{"id": "gpt-responses-mock", "object": "model"}]}))
}

/// 判断请求轮次：input 数组里出现 function_call_output 项即第 2 轮
/// （工具结果已回喂——Responses API 的工具结果条目类型）。
fn has_tool_result(body: &Value) -> bool {
    body["input"]
        .as_array()
        .map(|items| items.iter().any(|i| i["type"] == "function_call_output"))
        .unwrap_or(false)
}

/// SSE data 行。
fn sse(event: Value) -> String {
    format!("data: {}\n\n", event)
}

/// `response.created` 事件（流的开始标记）。
fn created_event() -> Value {
    json!({"type": "response.created", "response": {"id": "resp_mock", "status": "in_progress"}})
}

/// 思考项：reasoning item added + 两段 summary delta + item done。
fn reasoning_events() -> Vec<Value> {
    vec![
        json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {"type": "reasoning", "id": "rs_mock", "summary": []}
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_mock",
            "summary_index": 0,
            "delta": "用户要我看配置，"
        }),
        json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_mock",
            "summary_index": 0,
            "delta": "先调用 read。"
        }),
        json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {"type": "reasoning", "id": "rs_mock",
                "summary": [{"type": "summary_text", "text": TURN1_REASONING}]}
        }),
    ]
}

/// 正文消息项：message item added + 两段 text delta + item done（done 里
/// 重量化全文——官方流语义）。
fn message_events(output_index: usize, parts: [&str; 2]) -> Vec<Value> {
    let full = parts.concat();
    vec![
        json!({
            "type": "response.output_item.added",
            "output_index": output_index,
            "item": {"type": "message", "id": "msg_mock", "role": "assistant", "content": []}
        }),
        json!({
            "type": "response.output_text.delta",
            "item_id": "msg_mock",
            "content_index": 0,
            "delta": parts[0]
        }),
        json!({
            "type": "response.output_text.delta",
            "item_id": "msg_mock",
            "content_index": 0,
            "delta": parts[1]
        }),
        json!({
            "type": "response.output_item.done",
            "output_index": output_index,
            "item": {"type": "message", "id": "msg_mock", "role": "assistant",
                "content": [{"type": "output_text", "text": full}]}
        }),
    ]
}

/// 工具调用项：function_call added + arguments delta 三段（末段前是半截
/// JSON）+ item done（带完整入参）。
fn function_call_events(output_index: usize) -> Vec<Value> {
    let full_args = TURN1_TOOL_ARG_PARTS.concat();
    vec![
        json!({
            "type": "response.output_item.added",
            "output_index": output_index,
            "item": {"type": "function_call", "id": TURN1_ITEM_ID,
                "call_id": TURN1_CALL_ID, "name": TURN1_TOOL_NAME, "arguments": ""}
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "item_id": TURN1_ITEM_ID,
            "delta": TURN1_TOOL_ARG_PARTS[0]
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "item_id": TURN1_ITEM_ID,
            "delta": TURN1_TOOL_ARG_PARTS[1]
        }),
        json!({
            "type": "response.function_call_arguments.delta",
            "item_id": TURN1_ITEM_ID,
            "delta": TURN1_TOOL_ARG_PARTS[2]
        }),
        json!({
            "type": "response.output_item.done",
            "output_index": output_index,
            "item": {"type": "function_call", "id": TURN1_ITEM_ID,
                "call_id": TURN1_CALL_ID, "name": TURN1_TOOL_NAME, "arguments": full_args}
        }),
    ]
}

/// `response.completed` 终态事件（usage 从这里来）。
fn completed_event() -> Value {
    json!({
        "type": "response.completed",
        "response": {
            "id": "resp_mock",
            "status": "completed",
            "model": "gpt-responses-mock",
            "output": [],
            "usage": {
                "input_tokens": 120,
                "output_tokens": 48,
                "total_tokens": 168,
                "input_tokens_details": {"cached_tokens": 0},
                "output_tokens_details": {"reasoning_tokens": 16}
            }
        }
    })
}

/// 第 1 轮脚本：created → 思考 → 正文 → 工具调用 → completed。
fn turn1_events() -> Vec<ScriptEvent> {
    let mut events = vec![created_event()];
    events.extend(reasoning_events());
    events.extend(message_events(1, ["我来读一下", "配置文件。"]));
    events.extend(function_call_events(2));
    events.push(completed_event());
    events
}

/// 第 2 轮脚本：created → 纯文本答复 → completed。
fn turn2_events() -> Vec<ScriptEvent> {
    let mut events = vec![created_event()];
    events.extend(message_events(0, ["配置文件共 12 行，", "端口是 8087。"]));
    events.push(completed_event());
    events
}

/// 非流式请求的固定答复（suggest 等单次调用路径）：完整 response JSON。
pub const NONSTREAM_TEXT: &str = "试试让我总结配置要点。";

async fn handle_responses(
    body: Value,
    sent: Arc<Mutex<Vec<SentEvent>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    gap: Duration,
) -> Response {
    requests.lock().await.push(body.clone());
    if body["stream"] != Value::Bool(true) {
        return Json(json!({
            "id": "resp_mock",
            "status": "completed",
            "model": "gpt-responses-mock",
            "output": [{"type": "message", "id": "msg_mock", "role": "assistant",
                "content": [{"type": "output_text", "text": NONSTREAM_TEXT}]}],
            "usage": {"input_tokens": 120, "output_tokens": 48, "total_tokens": 168}
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
