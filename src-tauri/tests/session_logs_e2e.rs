//! 会话日志与脱敏 e2e（「日志全链路脱敏」常驻 CI 面）：
//! 本地 mock SSE 服务（tests/common），无网络、无真实 Key。
//!
//! 覆盖面：
//! - verbose（config.log.level=Debug）开 → start_chat_core 全链跑一轮
//!   带工具（ask 问卷）与审批（ask 协议两端口）的 run → 会话日志文件含
//!   run/llm/tool/approval 行，llm 含请求/响应原文行；
//! - 全文 grep 无 API Key 明文（`sk-` 前缀密钥在请求面被脱敏）；
//! - verbose 关（Info）→ 新日志不含请求/响应消息体行（只有元数据行）；
//! - 删除会话 → 本会话日志及轮转副本不存在。

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::mock_openai::{self, MockOpenAI};
use craftwave_engine::agent::RunManager;
use craftwave_engine::backend::ask::AskAnswer;
use craftwave_engine::backend::frames::{ChatFrame, EndReason, SequencedFrame, SequencedSink};
use craftwave_engine::app_state::AppState;
use craftwave_lib::host::commands::chat::{resolve_ask_core, start_chat_core};
use craftwave_engine::providers::types::{ApiFormat, AppConfig, ProviderConfig, ProviderModel};

mod common;

/// 帧收集器（与 protocol_dialects_e2e 同款，跨测试二进制不共享）。
#[derive(Clone, Default)]
struct SharedSink(Arc<std::sync::Mutex<Vec<SequencedFrame>>>);

impl SequencedSink for SharedSink {
    fn send_seq(&mut self, frame: SequencedFrame) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(frame);
    }
}

impl SharedSink {
    fn frames(&self) -> Vec<ChatFrame> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|f| f.frame.clone())
            .collect()
    }

    async fn wait_ended(&self, timeout: Duration) -> ChatFrame {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(last) = self.frames().last()
                && matches!(last, ChatFrame::RunEnded { .. })
            {
                return last.clone();
            }
            assert!(Instant::now() < deadline, "等待 runEnded 超时");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// 供应商夹具：`sk-` 前缀假密钥（脱敏断言的探针——绝不能出现在日志）。
fn provider_fixture(base_url: &str) -> ProviderConfig {
    ProviderConfig {
        id: "prov-1".to_owned(),
        name: "Mock 供应商".to_owned(),
        api_format: ApiFormat::OpenaiChat,
        base_url: Some(base_url.to_owned()),
        api_key: Some("sk-test1234567890abcdef".into()),
        models: vec![ProviderModel {
            id: "model-1".to_owned(),
            model: "gpt-mock".to_owned(),
            max_tokens: 32768,
            context_window: 128000,
            reasoning_effort: None,
            vision: None,
        }],
        headers: None,
        local: false,
    }
}

/// 应用夹具：日志级别可调（verbose = debug）。
fn app_fixture(base_url: &str, log_level: &str) -> (AppState, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let events = Arc::new(craftwave_engine::util::events::RecordingEventSink::new());
    let app = AppState::new_in(dir.path().to_path_buf(), events);
    let mut config = AppConfig {
        providers: vec![provider_fixture(base_url)],
        active_model_id: Some("model-1".to_owned()),
        ..AppConfig::default()
    };
    config.log.level = serde_json::from_value(serde_json::json!(log_level)).expect("日志级别");
    app.config.save(config).expect("save config");
    let root = dir.path().to_path_buf();
    // tempdir drop 会删目录——用 leak 保住（测试进程生命周期内可接受，
    // protocol_dialects_e2e 同款）
    std::mem::forget(dir);
    (app, root)
}

fn user_entry(text: &str) -> craftwave_engine::storage::MessageEntry {
    craftwave_engine::storage::MessageEntry {
        id: format!("u-{}", text.len()),
        parent_id: None,
        role: craftwave_engine::storage::MessageRole::User,
        parts: vec![craftwave_engine::storage::WirePart::Text {
            text: text.to_owned(),
        }],
        created_at: 1,
        run_id: None,
        status: None,
    }
}

fn run_config() -> craftwave_lib::host::commands::chat::RunConfig {
    craftwave_lib::host::commands::chat::RunConfig {
        model_id: None,
        thinking: None,
        tier: craftwave_engine::safety::tier::PermissionTier::FullAccess,
        max_turns: 4,
        steer: false,
    }
}

/// 第一轮脚本：先给一句带假密钥样式的正文（脱敏探针），再发起 ask 工具
/// 调用（问卷走 ask 协议两端口：ask:opened → resolve_ask → ask:closed）。
/// 第二轮（带工具结果回喂）走 mock 默认答复脚本。
fn turn1_with_ask() -> Vec<(&'static str, String)> {
    vec![
        mock_openai::content_delta("请确认 Bearer sk-test1234567890abcdef 是否可用，我先问你。"),
        mock_openai::tool_open(0, "call-ask-1", "ask"),
        mock_openai::tool_args_delta(
            0,
            r#"{"title":"确认","questions":[{"id":"q1","text":"用哪个值？","allowFreeText":true}]}"#,
        ),
        mock_openai::finish_chunk("tool_calls"),
        mock_openai::usage_event(),
        mock_openai::done_marker(),
    ]
}

/// 会话日志文件全文（jsonl 原文）。文件未产生（run 任务尚未首写）时
/// 返回空串——轮询场景由调用方超时兜底。
fn log_text(root: &std::path::Path, session_id: &str) -> String {
    std::fs::read_to_string(
        root.join("logs")
            .join("sessions")
            .join(format!("{session_id}.jsonl")),
    )
    .unwrap_or_default()
}

/// ---- 1. verbose 全链：run/llm/tool/approval 行齐备 + 全链脱敏 ----
#[tokio::test]
async fn verbose_run_logs_llm_tool_approval_and_redacts_keys() {
    let mock = MockOpenAI::start_with_turn1(turn1_with_ask()).await;
    let (app, root) = app_fixture(&mock.base_url, "debug");
    let runs = RunManager::new();
    let session_id = "sess-verbose".to_owned();
    let sink = SharedSink::default();
    let result = start_chat_core(
        &app,
        &runs,
        session_id.clone(),
        vec![user_entry("请确认密钥后继续")],
        run_config(),
        Box::new(sink.clone()),
        false,
    )
    .await
    .expect("start_chat_core 应成功");

    // 等待问卷打开（轮询日志里的 approval opened 行拿 askId——它带
    // callId 锚 = askId），然后作答回喂
    let ask_id = {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let text = log_text(&root, &session_id);
            if let Some(line) = text
                .lines()
                .find(|l| l.contains(r#""kind":"approval""#) && l.contains("等待用户回答问卷"))
                && let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
                && let Some(ask_id) = value.get("callId").and_then(|v| v.as_str())
            {
                break ask_id.to_owned();
            }
            assert!(Instant::now() < deadline, "等待问卷打开行超时");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    let mut answers = std::collections::BTreeMap::new();
    answers.insert("q1".to_owned(), "A".to_owned());
    resolve_ask_core(&runs, &ask_id, AskAnswer::Question { answers })
        .await
        .expect("resolve_ask 应命中 run 槽");

    let ended = sink.wait_ended(Duration::from_secs(20)).await;
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
    let _ = result;

    let text = std::fs::read_to_string(
        root.join("logs")
            .join("sessions")
            .join(format!("{session_id}.jsonl")),
    )
    .expect("run 收口后会话日志文件应存在");
    let lines: Vec<&str> = text.lines().collect();

    // ① kind=run 两行（开始/完成），完成行带终态与耗时
    assert!(
        lines
            .iter()
            .any(|l| l.contains(r#""kind":"run""#) && l.contains("run 开始")),
        "应有 run 开始行：{text}"
    );
    let done_line = lines
        .iter()
        .find(|l| l.contains(r#""kind":"run""#) && l.contains("run 完成"))
        .expect("应有 run 完成行");
    assert!(
        done_line.contains(r#""reason":"done""#),
        "完成行带终态：{done_line}"
    );
    assert!(
        done_line.contains(r#""durationMs""#),
        "完成行带耗时：{done_line}"
    );

    // ② kind=llm：Info 元数据行 + Debug 请求/响应原文行
    assert!(
        lines
            .iter()
            .any(|l| l.contains(r#""kind":"llm""#) && l.contains("模型请求完成")),
        "应有 llm 元数据行：{text}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains(r#""kind":"llm""#) && l.contains("模型请求原文")),
        "verbose 应有请求原文行：{text}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains(r#""kind":"llm""#) && l.contains("模型响应原文")),
        "verbose 应有响应原文行：{text}"
    );
    let request_line = lines
        .iter()
        .find(|l| l.contains("模型请求原文"))
        .expect("请求原文行");
    assert!(
        request_line.contains("chat_history") && request_line.contains("ask"),
        "请求原文应含 history 与工具调用（脱敏后仍可诊断）：{request_line}"
    );

    // ③ kind=tool：ask 工具结果行；kind=approval：ask 两端口行
    assert!(
        lines
            .iter()
            .any(|l| l.contains(r#""kind":"tool""#) && l.contains(r#""name":"ask""#)),
        "应有 ask 工具结果行：{text}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains(r#""kind":"approval""#) && l.contains("等待用户回答问卷")),
        "应有问卷打开行：{text}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains(r#""kind":"approval""#) && l.contains("问卷已回答")),
        "应有问卷收口行：{text}"
    );

    // ④ 红线：全文无 sk- 明文密钥（请求原文里的探针与 Provider
    //    配置的 api_key 都不得出现）
    assert!(
        !text.contains("sk-test1234567890abcdef"),
        "日志不得出现 sk- 密钥明文：{text}"
    );
    assert!(!text.contains("sk-abcdef"), "日志不得出现其他 sk- 形态明文");
    assert!(
        text.contains("sk-***") || text.contains("Bearer ***"),
        "探针密钥应被替换为脱敏占位：{text}"
    );

    // ⑤ 删除会话 → 本会话日志不存在（轮转副本同面）
    craftwave_lib::host::commands::session::delete_session_core(&app, &runs, &session_id)
        .expect("删除应成功");
    assert!(
        !root
            .join("logs")
            .join("sessions")
            .join(format!("{session_id}.jsonl"))
            .exists(),
        "删除会话后本会话日志应不存在"
    );
}

/// ---- 2. Info 级：新日志不含请求/响应消息体行（verbose 关）----
#[tokio::test]
async fn info_level_omits_request_response_bodies() {
    let mock = MockOpenAI::start().await;
    let (app, root) = app_fixture(&mock.base_url, "info");
    let runs = RunManager::new();
    let session_id = "sess-info".to_owned();
    let sink = SharedSink::default();
    start_chat_core(
        &app,
        &runs,
        session_id.clone(),
        vec![user_entry("帮我读配置文件")],
        run_config(),
        Box::new(sink.clone()),
        false,
    )
    .await
    .expect("start_chat_core 应成功");
    let ended = sink.wait_ended(Duration::from_secs(20)).await;
    assert!(
        matches!(
            &ended,
            ChatFrame::RunEnded {
                reason: EndReason::Done,
                ..
            }
        ),
        "run 应自然收口"
    );

    let text = log_text(&root, &session_id);
    // llm 元数据行在（Info），消息体行不在（无「请求原文」「响应原文」）
    assert!(
        text.contains(r#""kind":"llm""#) && text.contains("模型请求完成"),
        "Info 级应有 llm 元数据行：{text}"
    );
    assert!(
        !text.contains("请求原文"),
        "Info 级不应有请求原文行：{text}"
    );
    assert!(
        !text.contains("响应原文"),
        "Info 级不应有响应原文行：{text}"
    );
    // 消息正文不入日志（Info 级无任何载荷行；元数据行不含内容）
    assert!(
        !text.contains("配置文件共 12 行"),
        "模型回复正文不应出现在 Info 级日志：{text}"
    );
    // run/tool 行照记
    assert!(text.contains(r#""kind":"run""#), "run 行照记");
    assert!(text.contains(r#""kind":"tool""#), "tool 行照记");
}
