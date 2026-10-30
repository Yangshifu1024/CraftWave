//! 【38 号票】三协议（openai_chat / openai_responses / anthropic_messages）
//! 对话链路 e2e：本地 mock SSE 服务（tests/common），无网络、无真实 Key。
//!
//! 覆盖面（票面"测试"节）：
//! - anthropic_messages / openai_responses 的文本、reasoning、工具调用往返、
//!   四口径 usage（start_chat_core 命令面全链）；
//! - thinking 档（§11.4 Anthropic 预算轴 + max_tokens 上调规则）与不传档
//!   （协议必填的请求级 max_tokens 兜底）；
//! - 流中取消（Anthropic SSE 流丢弃语义与 openai_chat 同源）；
//! - 连接测试（providers::probe 三方言——probe_test.rs 单测覆盖，此处不重复）；
//! - 同一 run 内跨方言热切换（openai_chat → anthropic_messages，37 号票
//!   泛型化 TurnDriver 的收口验证）；
//! - 401 等供应商错误明确反馈（runEnded(error)，不伪装成功）。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use craftwave_lib::agent::RunManager;
use craftwave_lib::backend::frames::{
    ChatFrame, EndReason, FrameSink, SequencedFrame, SequencedSink,
};
use craftwave_lib::backend::run_loop::{AllowAllGate, RunExit, drive_run};
use craftwave_lib::backend::turn::TurnDriver;
use craftwave_lib::host::AppState;
use craftwave_lib::providers::types::{ApiFormat, AppConfig, ProviderConfig, ProviderModel};
use rig::completion::request::ToolDefinition;
use rig::message::ToolName;
use rig::run::AgentRun;
use rig::run::spec::RunSpec;
use tokio_util::sync::CancellationToken;

use common::mock_anthropic::MockAnthropic;
use common::mock_openai::MockOpenAI;
use common::mock_responses::MockResponses;

mod common;

const TOOL_READ: &str = "read";

/// 与 backend_e2e 同款读工具定义（mock 脚本的工具名一致）。
fn read_tool() -> Arc<ToolDefinition> {
    Arc::new(ToolDefinition::new(
        ToolName::new(TOOL_READ.to_owned()).expect("tool name"),
        "读取工作区内文件的一段文本",
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        }),
    ))
}

// ---- 帧收集器（与 backend_e2e 的 SharedSink 同款，跨测试二进制不共享）----

struct TimedFrame {
    frame: SequencedFrame,
}

#[derive(Clone, Default)]
struct SharedSink(Arc<Mutex<Vec<TimedFrame>>>);

impl FrameSink for SharedSink {
    fn send(&mut self, frame: ChatFrame) {
        let mut frames = self.0.lock().unwrap();
        let seq = frames.len() as u64;
        frames.push(TimedFrame {
            frame: SequencedFrame {
                seq,
                ts: None,
                frame,
            },
        });
    }
}

impl SequencedSink for SharedSink {
    fn send_seq(&mut self, frame: SequencedFrame) {
        self.0.lock().unwrap().push(TimedFrame { frame });
    }
}

impl SharedSink {
    fn frames(&self) -> Vec<ChatFrame> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.frame.frame.clone())
            .collect()
    }

    fn count_kind(&self, pred: impl Fn(&ChatFrame) -> bool) -> usize {
        self.frames().iter().filter(|f| pred(f)).count()
    }
}

/// 假工具执行器：返回固定文本（工具往返的第二拍驱动面）。
struct FakeExecutor;

impl craftwave_lib::backend::run_loop::CancellableExecutor for FakeExecutor {
    fn execute<'a>(
        &'a self,
        _call_id: &'a str,
        _name: &'a str,
        _args_json: &'a str,
        _token: &'a CancellationToken,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = craftwave_lib::backend::run_loop::ToolExecution> + Send + 'a>,
    > {
        Box::pin(async {
            craftwave_lib::backend::run_loop::ToolExecution::Completed {
                ok: true,
                ui_summary: "读取了配置文件".to_owned(),
                error: None,
                model_text: "共 12 行：port = 8087".to_owned(),
            }
        })
    }
}

// ---- start_chat_core 全链装配（照 backend_e2e command_face 先例）----

fn provider_fixture(api_format: ApiFormat, base_url: &str, vision: Option<bool>) -> ProviderConfig {
    ProviderConfig {
        id: "prov-1".to_owned(),
        name: "Mock 供应商".to_owned(),
        api_format,
        base_url: Some(base_url.to_owned()),
        api_key: Some("mock-key".into()),
        models: vec![ProviderModel {
            id: "model-1".to_owned(),
            model: match api_format {
                ApiFormat::AnthropicMessages => "claude-mock",
                _ => "gpt-mock",
            }
            .to_owned(),
            max_tokens: 32768,
            context_window: 128000,
            reasoning_effort: None,
            vision,
        }],
        headers: None,
        local: false,
    }
}

fn app_fixture(api_format: ApiFormat, base_url: &str) -> AppState {
    let dir = tempfile::tempdir().expect("tempdir");
    let events = Arc::new(craftwave_lib::util::events::RecordingEventSink::new());
    let app = AppState::new_in(dir.path().to_path_buf(), events);
    let config = AppConfig {
        providers: vec![provider_fixture(api_format, base_url, None)],
        active_model_id: Some("model-1".to_owned()),
        ..AppConfig::default()
    };
    app.config.save(config).expect("save config");
    // tempdir drop 会删目录——用 leak 保住（测试进程生命周期内可接受）
    std::mem::forget(dir);
    app
}

fn user_entry(text: &str) -> craftwave_lib::storage::MessageEntry {
    craftwave_lib::storage::MessageEntry {
        id: format!("u-{}", text.len()),
        parent_id: None,
        role: craftwave_lib::storage::MessageRole::User,
        parts: vec![craftwave_lib::storage::WirePart::Text {
            text: text.to_owned(),
        }],
        created_at: 1,
        run_id: None,
        status: None,
    }
}

fn run_config(
    thinking: Option<craftwave_lib::providers::reasoning::ThinkingLevel>,
) -> craftwave_lib::host::commands::chat::RunConfig {
    craftwave_lib::host::commands::chat::RunConfig {
        model_id: None,
        thinking,
        tier: craftwave_lib::safety::tier::PermissionTier::FullAccess,
        max_turns: 4,
        steer: false,
    }
}

/// 等待 runEnded 帧出现（命令面任务异步产帧）。
async fn wait_ended(sink: &SharedSink, timeout: Duration) -> ChatFrame {
    let deadline = Instant::now() + timeout;
    loop {
        let frames = sink.frames();
        if let Some(last) = frames.last()
            && matches!(last, ChatFrame::RunEnded { .. })
        {
            return last.clone();
        }
        assert!(Instant::now() < deadline, "等待 runEnded 超时");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// 四口径 usage 帧载荷断言助手。
fn usage_faces(sink: &SharedSink) -> Vec<craftwave_lib::backend::frames::UsageTotal> {
    sink.frames()
        .into_iter()
        .filter_map(|f| match f {
            ChatFrame::Usage { usage, .. } => Some(usage),
            _ => None,
        })
        .collect()
}

// ---- 1. anthropic_messages 全链（start_chat_core）----

#[tokio::test]
async fn anthropic_full_chain_text_reasoning_tool_usage() {
    let mock = MockAnthropic::start().await;
    let app = app_fixture(ApiFormat::AnthropicMessages, &mock.base_url);
    let runs = RunManager::new();
    let sink = SharedSink::default();

    // thinking=medium → §11.4 Anthropic 面预算轴 60%：budget = 128000×0.6 =
    // 76800（> max_tokens 32768 → max_tokens 上调为 min(128000, 76800+32768)
    // = 109568）
    let result = craftwave_lib::host::commands::chat::start_chat_core(
        &app,
        &runs,
        "sess-anthropic".to_owned(),
        vec![user_entry("帮我读配置文件")],
        run_config(Some(
            craftwave_lib::providers::reasoning::ThinkingLevel::Medium,
        )),
        Box::new(sink.clone()),
        false,
    )
    .await
    .expect("start_chat_core 应成功");

    let ended = wait_ended(&sink, Duration::from_secs(20)).await;
    assert!(
        matches!(
            &ended,
            ChatFrame::RunEnded {
                reason: EndReason::Done,
                ..
            }
        ),
        "run 应自然收口 Done，实际 {ended:?}（runId {}）",
        result.run_id
    );

    let _ = result;
    let bodies = mock.request_bodies().await;
    assert_eq!(bodies.len(), 2, "两拍请求（工具调用往返）");

    // ① thinking 档：第 1 拍请求带 60% 预算与上调后的 max_tokens（§11.4）
    assert_eq!(
        bodies[0]["thinking"],
        serde_json::json!({"type": "enabled", "budget_tokens": 76800}),
        "thinking=medium 应映射为 60% 预算：{}",
        bodies[0]
    );
    assert_eq!(
        bodies[0]["max_tokens"],
        serde_json::json!(109568),
        "budget 超过原 maxTokens 时应上调（min(window, budget+原 maxTokens)）：{}",
        bodies[0]
    );

    // ② 工具结果回喂：第 2 拍请求含 tool_result 块且 tool_use_id 同源
    let second = &bodies[1];
    let tool_result_id = second["messages"]
        .as_array()
        .and_then(|msgs| {
            msgs.iter().find_map(|m| {
                m["content"]
                    .as_array()
                    .and_then(|blocks| {
                        blocks
                            .iter()
                            .find(|b| b["type"] == "tool_result")
                            .map(|b| b["tool_use_id"].as_str().map(str::to_owned))
                    })
                    .flatten()
            })
        })
        .expect("第 2 拍应含 tool_result 块");
    assert_eq!(
        tool_result_id,
        common::mock_anthropic::TURN1_TOOL_ID,
        "工具结果应回喂给第 1 拍的同一个工具调用 id"
    );

    // ③ 帧面：reasoning/text/tool 三类增量 + 工具结果帧
    assert!(
        sink.count_kind(|f| matches!(f, ChatFrame::ReasoningDelta { .. })) > 0,
        "应有 reasoning 增量帧"
    );
    assert!(
        sink.count_kind(|f| matches!(f, ChatFrame::TextDelta { .. })) > 0,
        "应有文本增量帧"
    );
    assert!(
        sink.count_kind(|f| matches!(f, ChatFrame::ToolResult { .. })) >= 1,
        "应有工具结果帧"
    );

    // ④ 四口径 usage：首帧 = 第 1 拍（input = message_start 的 10、output =
    //    message_delta 的 48），末帧 = run 内累计（两拍：20 / 72）；cache 两
    //    口径 mock 未报告 → 0（未报告计数按 0 不伪造）
    let usages = usage_faces(&sink);
    assert_eq!(usages.len(), 2, "两拍各发一帧 usage");
    assert_eq!(
        usages[0].input, 10,
        "第 1 拍 input 口径 = message_start 计数"
    );
    assert_eq!(
        usages[0].output, 48,
        "第 1 拍 output 口径 = message_delta 计数"
    );
    let last = usages.last().unwrap();
    assert_eq!(last.input, 20, "usage 帧载荷是 run 内累计值");
    assert_eq!(last.output, 72);
    assert_eq!(last.cache_read, 0);
    assert_eq!(last.cache_write, 0);
}

/// thinking 未设置（None）时：Anthropic 协议必填的请求级 max_tokens 由
/// 模型配置兜底注入（32768），thinking 字段不出现（§11.4 off/不传 → 不启用）。
#[tokio::test]
async fn anthropic_without_thinking_injects_base_max_tokens() {
    let mock = MockAnthropic::start().await;
    let app = app_fixture(ApiFormat::AnthropicMessages, &mock.base_url);
    let runs = RunManager::new();
    let sink = SharedSink::default();

    craftwave_lib::host::commands::chat::start_chat_core(
        &app,
        &runs,
        "sess-anthropic-base".to_owned(),
        vec![user_entry("帮我读配置文件")],
        run_config(None),
        Box::new(sink.clone()),
        false,
    )
    .await
    .expect("start_chat_core 应成功");
    wait_ended(&sink, Duration::from_secs(20)).await;

    let bodies = mock.request_bodies().await;
    assert!(!bodies.is_empty(), "应有请求");
    assert_eq!(
        bodies[0]["max_tokens"],
        serde_json::json!(32768),
        "无 thinking 时请求级 max_tokens 应取模型配置值（协议必填）：{}",
        bodies[0]
    );
    assert!(
        bodies[0].get("thinking").is_none(),
        "未显式设置 thinking 不应发 thinking 字段：{}",
        bodies[0]
    );
}

/// Anthropic 鉴权失败（401）明确反馈：runEnded(error) 带错误（不静默、
/// 不伪装成功）。
#[tokio::test]
async fn anthropic_auth_error_reported_not_silent() {
    // 独立 mock：所有请求一律 401（Key 无效的真实形态）
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app_router = axum::Router::new().route(
        "/v1/messages",
        axum::routing::post(|| async {
            (
                axum::http::StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({
                    "type": "error",
                    "error": {"type": "authentication_error", "message": "invalid x-api-key"}
                })),
            )
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, app_router).await.unwrap();
    });
    let app = app_fixture(ApiFormat::AnthropicMessages, &format!("http://{addr}"));
    let runs = RunManager::new();
    let sink = SharedSink::default();

    craftwave_lib::host::commands::chat::start_chat_core(
        &app,
        &runs,
        "sess-anthropic-auth".to_owned(),
        vec![user_entry("帮我读配置文件")],
        run_config(None),
        Box::new(sink.clone()),
        false,
    )
    .await
    .expect("start_chat_core 命令应成功（失败在 run 内呈现）");

    let ended = wait_ended(&sink, Duration::from_secs(20)).await;
    match ended {
        ChatFrame::RunEnded {
            reason: EndReason::Error,
            error,
            ..
        } => {
            let error = error.expect("错误终态应带 error");
            assert!(!error.message.is_empty(), "错误消息应可读");
        }
        other => panic!("401 应以 runEnded(error) 明确反馈，实际 {other:?}"),
    }
}

// ---- 2. openai_responses 全链（start_chat_core）----

#[tokio::test]
async fn openai_responses_full_chain_text_reasoning_tool_usage() {
    let mock = MockResponses::start().await;
    let app = app_fixture(ApiFormat::OpenaiResponses, &mock.base_url);
    let runs = RunManager::new();
    let sink = SharedSink::default();

    // thinking=high → OpenAI 系 reasoning_effort="high"（§11.4 同名行）
    let result = craftwave_lib::host::commands::chat::start_chat_core(
        &app,
        &runs,
        "sess-responses".to_owned(),
        vec![user_entry("帮我读配置文件")],
        run_config(Some(
            craftwave_lib::providers::reasoning::ThinkingLevel::High,
        )),
        Box::new(sink.clone()),
        false,
    )
    .await
    .expect("start_chat_core 应成功");
    let _ = result;

    let ended = wait_ended(&sink, Duration::from_secs(20)).await;
    assert!(
        matches!(
            &ended,
            ChatFrame::RunEnded {
                reason: EndReason::Done,
                ..
            }
        ),
        "run 应自然收口 Done，实际 {ended:?}"
    );

    let bodies = mock.request_bodies().await;
    assert_eq!(bodies.len(), 2, "两拍请求（工具调用往返）");

    // ① 请求打 /responses 端点（Route 显式路由的 mock 侧实证——mock 只挂
    //    /v1/responses，能收到即路径正确）
    // ② thinking 档：reasoning 参数经 rig Responses wire 落在请求里
    //   （Responses API 的 reasoning.effort 形状）
    let reasoning = &bodies[0]["reasoning"];
    assert!(
        reasoning["effort"] == serde_json::json!("high"),
        "openai_responses 应带 reasoning.effort=high：{}",
        bodies[0]
    );

    // ③ 工具结果回喂：第 2 拍 input 含 function_call_output 且 call_id 同源
    let output_call_id = bodies[1]["input"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|i| i["type"] == "function_call_output")
                .and_then(|i| i["call_id"].as_str().map(str::to_owned))
        })
        .expect("第 2 拍应含 function_call_output 项");
    assert_eq!(
        output_call_id,
        common::mock_responses::TURN1_CALL_ID,
        "工具结果应回喂给第 1 拍的同一个 call_id"
    );

    // ④ 帧面与 usage（response.completed 的 usage：两拍各 input 120 /
    //    output 48，usage 帧为 run 内累计 → 末帧 240/96；cached 0 / cache_write 0）
    assert!(sink.count_kind(|f| matches!(f, ChatFrame::ReasoningDelta { .. })) > 0);
    assert!(sink.count_kind(|f| matches!(f, ChatFrame::TextDelta { .. })) > 0);
    assert!(sink.count_kind(|f| matches!(f, ChatFrame::ToolResult { .. })) >= 1);
    let usages = usage_faces(&sink);
    assert_eq!(usages.len(), 2, "两拍各发一帧 usage");
    assert_eq!(usages[0].input, 120);
    assert_eq!(usages[0].output, 48);
    let last = usages.last().expect("应有 usage 帧");
    assert_eq!(last.input, 240);
    assert_eq!(last.output, 96);
}

// ---- 3. 流中取消（Anthropic SSE 流丢弃）与 4. 跨方言热切换（drive_run 直驱）----

/// 指向 mock 的三方言模型驱动（与 chat.rs resolve_model 同一装配路径：
/// providers::wire::dialect_model）。直驱路径没有 HostRequestRefresher 的
/// 每拍注入面，spec 预置 Anthropic 协议必填的请求级 max_tokens（模拟宿主
/// 已按 §11.4 注入后的状态）。
fn dialect_driver(
    api_format: ApiFormat,
    base_url: &str,
    model: &str,
    tools: Vec<Arc<ToolDefinition>>,
) -> Arc<TurnDriver<craftwave_lib::providers::wire::DialectWire, rig::http_client::DynHttpClient>> {
    let wire =
        craftwave_lib::providers::wire::dialect_wire(api_format, base_url, "mock-key", model);
    let http = rig::http_client::DynHttpClient::new(rig_reqwest::ReqwestClient::from(
        reqwest::Client::new(),
    ));
    let spec = RunSpec {
        max_tokens: Some(32768),
        ..RunSpec::new()
    };
    Arc::new(TurnDriver::new(
        rig::driver::Model::new(wire, http),
        spec,
        tools,
    ))
}

/// Anthropic 慢流中取消：token 触发即丢弃流，runEnded(cancelled) 为末帧
/// （取消的 run 无 usage 收口——不计费的语义与方言无关）。
#[tokio::test]
async fn anthropic_cancel_midstream_discards_stream() {
    let mock = MockAnthropic::start().await;
    let driver = dialect_driver(
        ApiFormat::AnthropicMessages,
        &mock.base_url,
        "claude-mock",
        vec![read_tool()],
    );
    let sink = SharedSink::default();
    let mut sink_handle = sink.clone();
    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();
    let watcher_sink = sink.clone();
    // 出帧即取消（首个 reasoning delta 到达后）
    let watcher = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(5)).await;
            if !watcher_sink.frames().is_empty() {
                cancel_for_task.cancel();
                return;
            }
        }
    });
    let exit = drive_run(
        &driver,
        AgentRun::new(rig::completion::Message::user("帮我读配置文件")).max_turns(2),
        "run-anthropic-cancel",
        "claude-mock",
        cancel,
        Arc::new(FakeExecutor),
        Arc::new(AllowAllGate),
        &mut sink_handle,
        None,
        None,
        None,
        Duration::from_secs(30),
    )
    .await
    .expect("RunLoop 应收场");
    watcher.abort();

    assert!(
        matches!(exit, RunExit::Cancelled),
        "取消后退出态应为 Cancelled，实际 {exit:?}"
    );
    let frames = sink.frames();
    assert!(
        matches!(
            frames.last().expect("有帧"),
            ChatFrame::RunEnded {
                reason: EndReason::Cancelled,
                ..
            }
        ),
        "末帧应为 runEnded(cancelled)"
    );
    assert!(
        usage_faces(&sink).is_empty(),
        "取消的 run 无 usage 收口（不计费语义）"
    );
}

/// 同一 run 内跨方言热切换（37 号票泛型化 TurnDriver 的收口验证）：
/// 第 1 拍 openai_chat，refresher 在第二拍前 swap 到 anthropic_messages
/// wire + thinking 预算参数——第二拍请求落在 anthropic mock 且带
/// thinking 预算与请求级 max_tokens（§11.4 参数面随方言切换生效）。
#[tokio::test]
async fn cross_dialect_mid_run_switch_applies_on_next_request() {
    let mock_chat = MockOpenAI::start().await;
    let mock_anthropic = MockAnthropic::start().await;

    let driver = dialect_driver(
        ApiFormat::OpenaiChat,
        &mock_chat.base_url,
        "gpt-4o-mock",
        vec![read_tool()],
    );
    // 预构造 anthropic 侧的模型客户端（refresher 持有，第二拍前 swap）
    let anthropic_model = {
        let wire = craftwave_lib::providers::wire::dialect_wire(
            ApiFormat::AnthropicMessages,
            &mock_anthropic.base_url,
            "mock-key",
            "claude-mock",
        );
        rig::driver::Model::new(
            wire,
            rig::http_client::DynHttpClient::new(rig_reqwest::ReqwestClient::from(
                reqwest::Client::new(),
            )),
        )
    };

    struct SwapToAnthropic {
        driver: Arc<
            TurnDriver<
                craftwave_lib::providers::wire::DialectWire,
                rig::http_client::DynHttpClient,
            >,
        >,
        anthropic_model: rig::driver::Model<craftwave_lib::providers::wire::DialectWire>,
        calls: std::sync::atomic::AtomicUsize,
    }
    impl craftwave_lib::backend::run_loop::RequestRefresher for SwapToAnthropic {
        fn before_request(
            &self,
            _state: &craftwave_lib::backend::run_loop::DriveState,
        ) -> Result<(), craftwave_lib::backend::run_loop::RefreshFailure> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 1 {
                // 第二拍前生效：换 anthropic 客户端 + medium 预算参数
                self.driver.swap_model(self.anthropic_model.clone());
                self.driver.update_spec(|spec| {
                    spec.options = rig::completion::options::GenerationOptions::default()
                        .reasoning(rig::completion::Reasoning::Budget { tokens: 76800 })
                        .on_unsupported(rig::completion::options::OnUnsupported::Ignore);
                    spec.max_tokens = Some(109568);
                });
            }
            Ok(())
        }
    }

    let refresher = SwapToAnthropic {
        driver: driver.clone(),
        anthropic_model,
        calls: std::sync::atomic::AtomicUsize::new(0),
    };
    let sink = SharedSink::default();
    let mut sink_handle = sink.clone();
    let exit = drive_run(
        &driver,
        AgentRun::new(rig::completion::Message::user("帮我读配置文件")).max_turns(3),
        "run-cross-dialect",
        "gpt-4o-mock",
        CancellationToken::new(),
        Arc::new(FakeExecutor),
        Arc::new(AllowAllGate),
        &mut sink_handle,
        None,
        None,
        Some(&refresher),
        Duration::from_secs(30),
    )
    .await
    .expect("RunLoop 应正常跑完");

    assert!(
        matches!(exit, RunExit::Done(_)),
        "run 应自然收口 Done，实际 {exit:?}"
    );

    // 请求归属：openai_chat 收第 1 拍、anthropic 收第 2 拍（同 run 内切换）
    let chat_requests = mock_chat.request_bodies().await;
    let anthropic_requests = mock_anthropic.request_bodies().await;
    assert_eq!(chat_requests.len(), 1, "openai_chat 只应收到第 1 拍");
    assert_eq!(anthropic_requests.len(), 1, "anthropic 应收到第 2 拍");
    assert_eq!(
        anthropic_requests[0]["thinking"],
        serde_json::json!({"type": "enabled", "budget_tokens": 76800}),
        "切换后的第 2 拍应带 anthropic thinking 预算：{}",
        anthropic_requests[0]
    );
    assert_eq!(
        anthropic_requests[0]["max_tokens"],
        serde_json::json!(109568),
        "切换后的第 2 拍应带上调后的 max_tokens：{}",
        anthropic_requests[0]
    );
    // 工具结果跨方言回喂：第 2 拍 anthropic 请求含 tool_result（历史与
    // 工具配对在换方言后保持）
    let has_tool_result = anthropic_requests[0]["messages"]
        .as_array()
        .map(|msgs| {
            msgs.iter().any(|m| {
                m["content"]
                    .as_array()
                    .map(|blocks| blocks.iter().any(|b| b["type"] == "tool_result"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    assert!(has_tool_result, "跨方言切换后工具结果应照常回喂");
}
