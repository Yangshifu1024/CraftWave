//! 【S2+S3 Spike】本地 OpenAI 兼容 mock SSE 服务：脚本化输出混合事件序列
//! （文本 delta / reasoning delta / 工具参数增量含半截 JSON），无网络、无 API key。
//! 仅测试编译单元（tests/）可见，不进产物。

// 各集成测试二进制独立链接本模块，未用到的导出属预期。
#![allow(dead_code)]

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// mock 每轮脚本的默认 chunk 间隔（让打字机节奏可测）。
pub const CHUNK_GAP: Duration = Duration::from_millis(30);

/// 脚本第 1 轮的固定文本（两段 delta 拼接）。
pub const TURN1_TEXT: &str = "我来读一下配置文件。";
/// 脚本第 1 轮的思考文本（两段 delta 拼接）。
pub const TURN1_REASONING: &str = "用户要我看配置，先调用 read。";
/// 脚本第 1 轮的工具调用参数（分三段流式发出，第二段结尾是半截 JSON）。
pub const TURN1_TOOL_NAME: &str = "read";
pub const TURN1_TOOL_ARG_PARTS: [&str; 3] = [
    "{\"path\": \"/etc/app",
    "/config.toml\", \"off",
    "set\": 0}",
];
/// 脚本第 2 轮的最终答复（三段 delta 拼接）。
pub const TURN2_TEXT: &str = "配置文件共 12 行，端口是 8087。";

/// mock 发出的一个 SSE chunk 的发送记录（端到端延迟埋点用）。
#[derive(Debug, Clone)]
pub struct SentChunk {
    /// 相对服务启动的时刻。
    pub at: Instant,
    /// 摘要：reasoning / content / tool_name / tool_args / finish / usage
    pub kind: &'static str,
    /// 负载文本（delta 内容或参数片段）。
    pub payload: String,
}

/// 运行中的 mock 服务句柄。
pub struct MockOpenAI {
    /// rig OpenAIConfig 的 base_url（含 /v1）。
    pub base_url: String,
    pub sent: Arc<Mutex<Vec<SentChunk>>>,
    /// mock 收到的每个补全请求体（轮次与回喂断言用）。
    pub requests: Arc<Mutex<Vec<Value>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    server: JoinHandle<()>,
}

impl MockOpenAI {
    /// 启动 mock（随机端口），chunk 间隔 30ms。
    pub async fn start() -> Self {
        Self::start_with_gap(CHUNK_GAP).await
    }

    pub async fn start_with_gap(gap: Duration) -> Self {
        let sent: Arc<Mutex<Vec<SentChunk>>> = Arc::default();
        let requests: Arc<Mutex<Vec<Value>>> = Arc::default();
        let route_sent = sent.clone();
        let route_requests = requests.clone();
        let app = Router::new()
            .route(
                "/v1/chat/completions",
                post(move |Json(body): Json<Value>| {
                    let sent = route_sent.clone();
                    let requests = route_requests.clone();
                    async move { handle_completion(body, sent, requests, gap).await }
                }),
            )
            .route("/v1/models", get(get_models));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
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

    pub async fn sent_chunks(&self) -> Vec<SentChunk> {
        self.sent.lock().await.clone()
    }

    pub async fn request_bodies(&self) -> Vec<Value> {
        self.requests.lock().await.clone()
    }
}

impl Drop for MockOpenAI {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.server.abort();
    }
}

async fn get_models() -> Json<Value> {
    Json(json!({"object": "list", "data": [{"id": "gpt-4o-mock", "object": "model"}]}))
}

/// 判断请求轮次：messages 里出现 role=tool 即第 2 轮（工具结果已回喂）。
fn has_tool_result(body: &Value) -> bool {
    body["messages"]
        .as_array()
        .map(|msgs| msgs.iter().any(|m| m["role"] == "tool"))
        .unwrap_or(false)
}

fn chunk(delta: Value, finish: Option<&str>) -> String {
    let finish = finish.map(Value::from).unwrap_or(Value::Null);
    format!(
        "data: {}\n\n",
        json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "gpt-4o-mock",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]
        })
    )
}

fn usage_chunk() -> String {
    format!(
        "data: {}\n\n",
        json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "gpt-4o-mock",
            "choices": [],
            "usage": {"prompt_tokens": 120, "completion_tokens": 48, "total_tokens": 168}
        })
    )
}

/// 第 1 轮脚本：思考 → 文本 → 工具名 → 参数三段（第二段结尾半截 JSON）→ 收尾。
fn turn1_events() -> Vec<(&'static str, String)> {
    let mut events = Vec::new();
    events.push(("reasoning", chunk(json!({"reasoning_content": "用户要我看配置，"}), None)));
    events.push(("reasoning", chunk(json!({"reasoning_content": "先调用 read。"}), None)));
    events.push(("content", chunk(json!({"content": "我来读一下"}), None)));
    events.push(("content", chunk(json!({"content": "配置文件。"}), None)));
    events.push((
        "tool_name",
        chunk(
            json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                "function": {"name": TURN1_TOOL_NAME, "arguments": ""}}]}),
            None,
        ),
    ));
    for part in TURN1_TOOL_ARG_PARTS {
        events.push((
            "tool_args",
            chunk(
                json!({"tool_calls": [{"index": 0,
                    "function": {"arguments": part}}]}),
                None,
            ),
        ));
    }
    events.push(("finish", chunk(json!({}), Some("tool_calls"))));
    events.push(("usage", usage_chunk()));
    events.push(("done", "data: [DONE]\n\n".to_string()));
    events
}

/// 第 2 轮脚本：纯文本答复。
fn turn2_events() -> Vec<(&'static str, String)> {
    let mut events = Vec::new();
    events.push(("content", chunk(json!({"content": "配置文件共 12 行，"}), None)));
    events.push(("content", chunk(json!({"content": "端口是"}), None)));
    events.push(("content", chunk(json!({"content": " 8087。"}), None)));
    events.push(("finish", chunk(json!({}), Some("stop"))));
    events.push(("usage", usage_chunk()));
    events.push(("done", "data: [DONE]\n\n".to_string()));
    events
}

async fn handle_completion(
    body: Value,
    sent: Arc<Mutex<Vec<SentChunk>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    gap: Duration,
) -> Response {
    requests.lock().await.push(body.clone());
    let script = if has_tool_result(&body) {
        turn2_events()
    } else {
        turn1_events()
    };
    let stream =
        futures::stream::iter(script.into_iter().map(Ok::<_, Infallible>)).then(move |item| {
            let sent = sent.clone();
            async move {
                let (kind, payload) = item?;
                tokio::time::sleep(gap).await;
                sent.lock().await.push(SentChunk {
                    at: Instant::now(),
                    kind,
                    payload: payload.clone(),
                });
                Ok::<_, Infallible>(payload)
            }
        });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(stream))
        .expect("mock response")
}
