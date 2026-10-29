//! 【S2+S3 Spike】本地 OpenAI 兼容 mock SSE 服务：脚本化输出混合事件序列
//! （文本 delta / reasoning delta / 工具参数增量含半截 JSON），无网络、无 API key。
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
    /// 非流式请求失败注入（04 号票压缩摘要失败的 e2e 面）：置 true 后
    /// 非流式补全一律返回 500，流式脚本不受影响。
    pub fail_nonstream: Arc<std::sync::atomic::AtomicBool>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    server: JoinHandle<()>,
}

/// SSE 脚本事件：`(kind 摘要, OpenAI 兼容 chunk 负载)`。
pub type ScriptEvent = (&'static str, String);

impl MockOpenAI {
    /// 启动 mock（随机端口），chunk 间隔 30ms，默认 S2/S3 脚本。
    pub async fn start() -> Self {
        Self::start_with_gap(CHUNK_GAP).await
    }

    /// 启动 mock，第 1 轮脚本换成调用方给的序列（S5：长流 / 双工具）；
    /// 带 tool_result 的第 2 轮请求仍走默认答复脚本。chunk 间隔 30ms。
    pub async fn start_with_turn1(turn1: Vec<ScriptEvent>) -> Self {
        Self::start_scripts(CHUNK_GAP, turn1).await
    }

    pub async fn start_with_gap(gap: Duration) -> Self {
        Self::start_scripts(gap, turn1_events()).await
    }

    async fn start_scripts(gap: Duration, turn1: Vec<ScriptEvent>) -> Self {
        let sent: Arc<Mutex<Vec<SentChunk>>> = Arc::default();
        let requests: Arc<Mutex<Vec<Value>>> = Arc::default();
        let fail_nonstream: Arc<std::sync::atomic::AtomicBool> = Arc::default();
        let route_sent = sent.clone();
        let route_requests = requests.clone();
        let route_fail = fail_nonstream.clone();
        let app = Router::new()
            .route(
                "/v1/chat/completions",
                post(move |Json(body): Json<Value>| {
                    let sent = route_sent.clone();
                    let requests = route_requests.clone();
                    let fail = route_fail.clone();
                    let turn1 = turn1.clone();
                    async move { handle_completion(body, sent, requests, gap, turn1, fail).await }
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
            fail_nonstream,
            shutdown: Some(tx),
            server,
        }
    }

    /// 置位/解除非流式失败注入（压缩摘要失败 e2e 用）。
    pub fn set_fail_nonstream(&self, on: bool) {
        self.fail_nonstream
            .store(on, std::sync::atomic::Ordering::SeqCst);
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

// —— 脚本组合子（公开：S5 测试自造 turn1 脚本用） ——

/// 一条正文 delta chunk。
pub fn content_delta(text: &str) -> ScriptEvent {
    ("content", chunk(json!({"content": text}), None))
}

/// 工具调用开启 chunk（带 id/name；参数由后续 tool_args 增量给出）。
pub fn tool_open(index: usize, id: &str, name: &str) -> ScriptEvent {
    (
        "tool_name",
        chunk(
            json!({"tool_calls": [{"index": index, "id": id, "type": "function",
                "function": {"name": name, "arguments": ""}}]}),
            None,
        ),
    )
}

/// 工具参数增量 chunk（半截 JSON 原样）。
pub fn tool_args_delta(index: usize, part: &str) -> ScriptEvent {
    (
        "tool_args",
        chunk(
            json!({"tool_calls": [{"index": index,
                "function": {"arguments": part}}]}),
            None,
        ),
    )
}

/// finish 收尾 chunk。
pub fn finish_chunk(reason: &str) -> ScriptEvent {
    ("finish", chunk(json!({}), Some(reason)))
}

/// usage 记账 chunk。
pub fn usage_event() -> ScriptEvent {
    ("usage", usage_chunk())
}

/// 流终止标记。
pub fn done_marker() -> ScriptEvent {
    ("done", "data: [DONE]\n\n".to_string())
}

/// 第 1 轮脚本：思考 → 文本 → 工具名 → 参数三段（第二段结尾半截 JSON）→ 收尾。
fn turn1_events() -> Vec<ScriptEvent> {
    vec![
        (
            "reasoning",
            chunk(json!({"reasoning_content": "用户要我看配置，"}), None),
        ),
        (
            "reasoning",
            chunk(json!({"reasoning_content": "先调用 read。"}), None),
        ),
        content_delta("我来读一下"),
        content_delta("配置文件。"),
        tool_open(0, "call_1", TURN1_TOOL_NAME),
        tool_args_delta(0, TURN1_TOOL_ARG_PARTS[0]),
        tool_args_delta(0, TURN1_TOOL_ARG_PARTS[1]),
        tool_args_delta(0, TURN1_TOOL_ARG_PARTS[2]),
        finish_chunk("tool_calls"),
        usage_event(),
        done_marker(),
    ]
}

/// 第 2 轮脚本：纯文本答复。
fn turn2_events() -> Vec<ScriptEvent> {
    vec![
        content_delta("配置文件共 12 行，"),
        content_delta("端口是"),
        content_delta(" 8087。"),
        finish_chunk("stop"),
        usage_event(),
        done_marker(),
    ]
}

/// 非流式（stream != true）请求的固定答复（suggest 等单次调用路径）。
pub const NONSTREAM_TEXT: &str = "试试让我总结配置要点。";

async fn handle_completion(
    body: Value,
    sent: Arc<Mutex<Vec<SentChunk>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    gap: Duration,
    turn1: Vec<ScriptEvent>,
    fail_nonstream: Arc<std::sync::atomic::AtomicBool>,
) -> Response {
    requests.lock().await.push(body.clone());
    // 非流式请求（rig Model::call 单次调用，如 suggest/压缩摘要的后台小调用）：
    // 失败注入开启时返回 500（压缩失败兜底 e2e 面）；否则返回单个
    // chat.completion JSON（OpenAI 兼容端点两形态都支持）。
    if body["stream"] != Value::Bool(true) {
        if fail_nonstream.load(std::sync::atomic::Ordering::SeqCst) {
            return (StatusCode::INTERNAL_SERVER_ERROR, "injected failure").into_response();
        }
        return Json(json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "created": 1,
            "model": "gpt-4o-mock",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": NONSTREAM_TEXT}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 120, "completion_tokens": 48, "total_tokens": 168}
        }))
        .into_response();
    }
    let script = if has_tool_result(&body) {
        turn2_events()
    } else {
        turn1
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
