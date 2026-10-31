//! run 装配树端到端测试（挂载于 `api::run`，仅 cargo test 时编译）。
//!
//! 装置：rig 剧本假模型（`MockCompletionModel`，流式 turn 驱动 `drive_run`
//! 全链）+ 录音事件汇 + 帧收集器 + 临时数据根。模型解析经
//! [`super::model::ModelResolver`] 接缝注入剧本（生产路径的 AppConfig
//! 查找是唯一被替换的面，装配与 run 循环零 mock）。
//!
//! 场景：start_chat 端到端（工具往返 → 帧序列 → 终态事件 → 树持久化 →
//! 记账落库）、cancel、ask 权限中断（允许/拒绝双路径）、压缩、followUp
//! 恢复投递、装配函数冒烟。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rig::completion::Usage;
use rig::test_utils::{MockCompletionModel, MockRuntime, MockScript, MockStreamEvent, MockTurn};

use super::model::{ModelResolver, ResolvedModel};
use super::{DEFAULT_MAX_TURNS, RunConfig, StartChatArgs};
use crate::agent::RunManager;
use crate::api::session::DEFAULT_SESSION_TITLE;
use crate::app_state::AppState;
use crate::backend::frames::{ChatFrame, EndReason, SequencedFrame, SequencedSink};
use crate::safety::tier::PermissionTier;
use crate::storage::{MessageEntry, MessageRole, SessionMeta, WirePart};
use crate::util::errors::IpcError;
use crate::util::events::{RecordingEventSink, keys};

/// 事件/终态等待超时（current_thread 调度器上的轮询上限；run 是后台任务，
/// 测试 future 让出即推进）。
const WAIT: Duration = Duration::from_secs(10);

/// 共享帧收集器：`SequencedSink` 的测试实现，句柄可从测试侧读帧序列
/// （`SeqCollectingSink` 是裸结构，Box 进枢纽后无法回读——本实现加一层
/// Arc 句柄）。
#[derive(Default, Clone)]
struct SharedSink(Arc<Mutex<Vec<SequencedFrame>>>);

impl SequencedSink for SharedSink {
    fn send_seq(&mut self, frame: SequencedFrame) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(frame);
    }
}

impl SharedSink {
    /// 帧快照（按到达序）。
    fn frames(&self) -> Vec<SequencedFrame> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// 测试夹具：临时数据根 + 录音事件汇 + 预置会话（标题非默认——绕开自动
/// 起名任务，测试不消费额外剧本）。返回 (app, events, 数据根)。
fn fixture(session_id: &str) -> (Arc<AppState>, Arc<RecordingEventSink>, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir); // 测试进程生命周期内持有（backend e2e 同款）
    let events = Arc::new(RecordingEventSink::new());
    let app = AppState::new_in(root.clone(), events.clone());
    let ts = 1_700_000_000_000i64;
    app.store
        .index()
        .upsert(SessionMeta {
            id: session_id.to_owned(),
            project_id: None,
            // 标题非默认：自动起名三不触发的「已改名」面（不消耗 mock 剧本）
            title: "测试会话（已命名）".to_owned(),
            created_at: ts,
            updated_at: ts,
            revision: 0,
            message_count: 0,
            total_tokens: 0,
            authorized_external_dirs: Vec::new(),
        })
        .expect("upsert index");
    (Arc::new(app), events, root)
}

/// 剧本假模型解析器：每次解析弹出一个预置模型（主 run / 摘要 / 标题 /
/// 递归投递各占一个脚本位）。解析次数超预期时 panic（装配面漂移的显式
/// 失败信号）。
fn mock_resolver(
    models: Vec<MockCompletionModel>,
) -> ModelResolver<MockScript, MockRuntime> {
    let queue = Mutex::new(VecDeque::from(models));
    Arc::new(move |_app: &AppState, _model_id: Option<&str>| {
        let driver = queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .expect("mock 模型脚本耗尽（装配解析次数超预期）");
        Ok(ResolvedModel {
            id: "mock-1".to_owned(),
            name: "gpt-mock".to_owned(),
            driver,
            context_window: 128_000,
            max_tokens: 32_768,
            vision: false,
            api_format: crate::providers::types::ApiFormat::OpenaiChat,
            reasoning_effort: None,
            base_url: "http://mock.local".to_owned(),
        })
    })
}

/// 剧本 usage（每轮 120 in / 48 out——记账断言的固定口径）。
fn scripted_usage() -> Usage {
    Usage::default().input_tokens(120u64).output_tokens(48u64)
}

/// 流式 turn：正文 + 工具调用 + 收口（带 usage）。
fn stream_turn_with_tool(
    call_id: &str,
    name: &str,
    args: serde_json::Value,
) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::text("我先执行一个工具。"),
        MockStreamEvent::tool_call(call_id, name, args),
        MockStreamEvent::final_response(scripted_usage()),
    ]
}

/// 流式 turn：纯正文收口（带 usage）。
fn stream_text_turn(text: &str) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::text(text),
        MockStreamEvent::final_response(scripted_usage()),
    ]
}

/// 主 run 的剧本模型（流式 turn 序列）。
fn stream_model(turns: Vec<Vec<MockStreamEvent>>) -> MockCompletionModel {
    MockCompletionModel::from_stream_turns(turns)
}

/// 辅助小调用的剧本模型（非流式 turn——压缩摘要走 complete_once）。
fn unary_model(text: &str) -> MockCompletionModel {
    MockCompletionModel::from_turns([MockTurn::text(text).with_usage(scripted_usage())])
}

/// 占位摘要模型（默认场景不触发压缩/标题，永不被消费；防御脚本错位）。
fn unused_model() -> MockCompletionModel {
    MockCompletionModel::from_turns([])
}

/// 用户消息条目（wire 校验要求末条为 user）。
fn user_msg(id: &str, text: &str) -> MessageEntry {
    MessageEntry {
        id: id.to_owned(),
        parent_id: None,
        role: MessageRole::User,
        parts: vec![WirePart::Text {
            text: text.to_owned(),
        }],
        created_at: 1_700_000_000_000i64,
        run_id: None,
        status: None,
    }
}

/// run 配置（档位入参；其余缺省）。
fn run_config(tier: PermissionTier) -> RunConfig {
    RunConfig {
        model_id: None,
        thinking: None,
        tier,
        max_turns: DEFAULT_MAX_TURNS,
        steer: false,
    }
}

/// 事件桥等待辅助（轮询录音汇，不阻塞 worker 线程；chat_test 同款）。
async fn wait_event(
    events: &RecordingEventSink,
    key: &str,
    timeout: Duration,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(payload) = events.payloads_of(key).last() {
            return payload.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "等待事件 {key} 超时"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// start_chat 装配入口的测试简写（经接缝注入剧本解析器）。
#[allow(clippy::too_many_arguments)]
async fn start(
    app: &Arc<AppState>,
    session_id: &str,
    messages: Vec<MessageEntry>,
    config: RunConfig,
    sink: SharedSink,
    models: Vec<MockCompletionModel>,
) -> Result<String, IpcError> {
    let args = StartChatArgs {
        session_id: session_id.to_owned(),
        messages,
        run_config: config,
    };
    let result = super::assemble::start_chat_assembled(
        app,
        &app.runs,
        args.session_id,
        args.messages,
        args.run_config,
        Box::new(sink),
        false,
        mock_resolver(models),
    )
    .await?;
    Ok(result.run_id)
}

// ---- 场景一：start_chat 端到端（工具往返 → 帧序列 → 终态 → 持久化 →
// 记账） ----

/// 全链剧本：turn1 正文+calculate 调用 → turn2 收尾正文（结果回喂后）。
#[tokio::test]
async fn start_chat_e2e_tool_roundtrip_frames_persistence_ledger() {
    let (app, events, _root) = fixture("sess-e2e");
    let sink = SharedSink::default();
    let models = vec![
        // ① 主 run：turn1 调 calculate(2+3)，turn2 汇报结果
        stream_model(vec![
            stream_turn_with_tool("call-1", "calculate", serde_json::json!({"expression": "2+3"})),
            stream_text_turn("计算完成，结果是 5。"),
        ]),
        // ② 摘要模型（未触发压缩，占位）
        unused_model(),
    ];
    let run_id = start(
        &app,
        "sess-e2e",
        vec![user_msg("u1", "帮我算 2+3")],
        run_config(PermissionTier::FullAccess),
        sink.clone(),
        models,
    )
    .await
    .expect("start_chat 装配成功");
    assert!(!run_id.is_empty());

    // 终态 run:done 事件（载荷 sessionId/runId）
    let done = wait_event(&events, keys::RUN_DONE, WAIT).await;
    assert_eq!(done["sessionId"], "sess-e2e");
    assert_eq!(done["runId"], run_id);
    assert!(done["durationMs"].is_u64());

    // 帧序列：seq 严格递增；关键帧按协议次序出现；末帧 runEnded(done)
    let frames = sink.frames();
    assert!(!frames.is_empty());
    assert!(
        frames.windows(2).all(|w| w[0].seq < w[1].seq),
        "帧序号必须严格递增"
    );
    let kinds: Vec<String> = frames
        .iter()
        .map(|f| serde_json::to_value(&f.frame).unwrap()["type"]
            .as_str()
            .unwrap_or_default()
            .to_owned())
        .collect();
    let kind = |name: &str| kinds.iter().position(|k| k == name);
    assert_eq!(kinds.first().map(String::as_str), Some("runStarted"));
    let step1 = kind("stepStarted").expect("缺 stepStarted");
    let tool_started = kind("toolStarted").expect("缺 toolStarted");
    let tool_result = kind("toolResult").expect("缺 toolResult");
    let usage_at = kind("usage").expect("缺 usage");
    let run_ended = kind("runEnded").expect("缺 runEnded");
    // 帧序：CallModel（stepStarted → 流内 toolStarted/toolArgs → usage
    // 收口）先于 CallTools（toolResult），末帧 runEnded
    assert!(step1 < tool_started && tool_started < usage_at && usage_at < tool_result);
    assert_eq!(kinds.last().map(String::as_str), Some("runEnded"));
    let ended_at = run_ended;
    if let ChatFrame::RunEnded {
        reason,
        tool_count,
        turn,
        error,
        ..
    } = &frames[ended_at].frame
    {
        assert_eq!(reason, &EndReason::Done);
        assert_eq!(*tool_count, 1, "工具调用计数 = 1");
        assert_eq!(*turn, 2, "两个模型轮次");
        assert!(error.is_none(), "自然终态无错误面");
    } else {
        panic!("末帧不是 runEnded");
    }
    // 工具往返：toolResult ok=true（calculate 真执行——2+3=5 进摘要）
    let mut tool_ok = false;
    for f in &frames {
        if let ChatFrame::ToolResult { ok, ui_summary, .. } = &f.frame {
            assert!(ui_summary.contains("2+3"), "结果摘要应含表达式：{ui_summary}");
            tool_ok = *ok;
        }
    }
    assert!(tool_ok, "calculate 应真实执行成功");
    // 正文增量可达（打字机面）
    assert!(kind("textDelta").is_some(), "应有正文增量帧");

    // 树持久化：user + assistant（含 toolCall/toolResult part）均入史册
    let tree = app
        .store
        .load_session_tree("sess-e2e")
        .expect("史册应可加载");
    assert!(tree.messages.iter().any(|m| m.id == "u1"), "用户消息入树");
    let assistant = tree
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::Assistant)
        .count();
    assert!(assistant >= 1, "assistant 条目应已落盘");
    assert!(
        tree.messages.iter().any(|m| {
            m.parts
                .iter()
                .any(|p| matches!(p, WirePart::ToolResult { ok: true, .. }))
        }),
        "工具结果 part 应在史册"
    );

    // 记账落库：主 run 明细 kind=main、按实际执行模型归属、两轮 usage 相加
    let detail = app.ledger.detail.load("sess-e2e");
    let run_entry = detail.runs.get(&run_id).expect("主 run 明细应存在");
    assert_eq!(run_entry.kind, "main");
    let model_entry = run_entry.by_model.get("mock-1").expect("模型归属明细");
    assert_eq!(model_entry.input, 240, "两轮 120 in 相加");
    assert_eq!(model_entry.output, 96, "两轮 48 out 相加");
    // tokens:update 全量校正事件照发（每轮一次）
    let tokens = events.payloads_of(keys::TOKENS_UPDATE);
    assert_eq!(tokens.len(), 2, "两轮各一个 tokens:update");
    assert_eq!(tokens[1]["usage"]["input"], 240);
    assert_eq!(tokens[1]["modelId"], "mock-1");

    // RunManager 摘除：终态后无活跃 run
    assert!(app.runs.get("sess-e2e").is_none(), "终态后 run 应摘除");
}

// ---- 场景二：cancel（运行中置位令牌 → run_cancelled 终态 + 树一致） ----

/// 剧本：turn1 调 ask 工具（问卷挂起等待应答）→ 测试在等待面置位取消。
#[tokio::test]
async fn cancel_during_ask_yields_cancelled_terminal() {
    let (app, events, _root) = fixture("sess-cancel");
    let sink = SharedSink::default();
    let models = vec![
        stream_model(vec![stream_turn_with_tool(
            "ask-1",
            "ask",
            serde_json::json!({
                "title": "确认",
                "questions": [{"id": "q1", "text": "继续吗？", "allowFreeText": true}]
            }),
        )]),
        unused_model(),
    ];
    start(
        &app,
        "sess-cancel",
        vec![user_msg("u1", "问我一个问题")],
        run_config(PermissionTier::FullAccess),
        sink.clone(),
        models,
    )
    .await
    .expect("start_chat 成功");

    // ask:opened 到达 → ask 槽在册（session_pending_ask 对账可见）
    let opened = wait_event(&events, keys::ASK_OPENED, WAIT).await;
    assert_eq!(opened["sessionId"], "sess-cancel");
    let ask_id = opened["askId"].as_str().expect("askId").to_owned();
    let pending = super::session_pending_ask(
        &app,
        super::SessionPendingAskArgs {
            session_id: "sess-cancel".to_owned(),
        },
    )
    .expect("查询成功");
    assert_eq!(pending.expect("未决询问应在册").ask_id, ask_id);

    // 取消：置位即返回 true
    assert!(super::cancel_run(&app, "sess-cancel").expect("取消成功"));

    // run_cancelled 终态 + ask:closed（answered=false，未回答即取消）
    let cancelled = wait_event(&events, keys::RUN_CANCELLED, WAIT).await;
    assert_eq!(cancelled["sessionId"], "sess-cancel");
    assert!(cancelled["runId"].is_string(), "终态载荷应带 runId");
    let closed = wait_event(&events, keys::ASK_CLOSED, WAIT).await;
    assert_eq!(closed["answered"], false, "取消路径未应答");

    // 帧面：末帧 runEnded(cancelled)；无 run:done
    let frames = sink.frames();
    let last = frames.last().expect("应有帧");
    if let ChatFrame::RunEnded { reason, error, .. } = &last.frame {
        assert_eq!(reason, &EndReason::Cancelled);
        assert!(error.is_none());
    } else {
        panic!("末帧应为 runEnded：{:?}", last.frame);
    }
    assert!(events.payloads_of(keys::RUN_DONE).is_empty());

    // 树状态一致：用户消息已落盘（输入保留），终态后 run 摘除、槽清空
    let tree = app.store.load_session_tree("sess-cancel").expect("史册可读");
    assert!(tree.messages.iter().any(|m| m.id == "u1"));
    assert!(app.runs.get("sess-cancel").is_none());
    assert!(
        super::session_pending_ask(
            &app,
            super::SessionPendingAskArgs {
                session_id: "sess-cancel".to_owned()
            },
        )
        .expect("查询成功")
        .is_none(),
        "取消后未决询问应清空"
    );
}

// ---- 场景三：ask 权限中断（ConfirmEach 档 write 挂起 → 处置续跑/取消） ----

/// 允许路径：write 挂起 → resolve_ask(allow_once) → 真实写盘 → run 完成。
#[tokio::test]
async fn ask_approval_allow_resumes_and_writes_file() {
    let (app, events, root) = fixture("sess-allow");
    let sink = SharedSink::default();
    let models = vec![
        stream_model(vec![
            stream_turn_with_tool(
                "w-1",
                "write",
                serde_json::json!({"path": "allow-e2e.txt", "content": "审批后写入"}),
            ),
            stream_text_turn("文件已写入。"),
        ]),
        unused_model(),
    ];
    start(
        &app,
        "sess-allow",
        vec![user_msg("u1", "写一个文件")],
        // ConfirmEach：FileWrite 需审批 → 挂起
        run_config(PermissionTier::ConfirmEach),
        sink.clone(),
        models,
    )
    .await
    .expect("start_chat 成功");

    // 审批卡开启（kind=approval，标题指向写入）
    let opened = wait_event(&events, keys::ASK_OPENED, WAIT).await;
    assert_eq!(opened["payload"]["kind"], serde_json::json!("approval"));
    assert!(opened["payload"]["options"].is_array());
    let ask_id = opened["askId"].as_str().expect("askId").to_owned();

    // 处置：允许（allow_once）
    super::resolve_ask(
        &app,
        super::ResolveAskArgs {
            ask_id,
            answer: crate::backend::ask::AskAnswer::Approval {
                option_id: "allow_once".to_owned(),
                edited_command: None,
            },
        },
    )
    .await
    .expect("应答投递成功");

    // run 按处置继续：写盘真实发生（workspace 根 = <data>/workspace/）
    wait_event(&events, keys::RUN_DONE, WAIT).await;
    let written = root.join("workspace").join("allow-e2e.txt");
    assert_eq!(
        std::fs::read_to_string(&written).expect("文件应已写入"),
        "审批后写入"
    );
    // 帧面：拒绝面缺席——toolResult ok=true、末帧 done
    let frames = sink.frames();
    if let ChatFrame::RunEnded { reason, .. } = &frames.last().expect("帧").frame {
        assert_eq!(reason, &EndReason::Done);
    }
    assert!(app.runs.get("sess-allow").is_none());
}

/// 拒绝路径：write 挂起 → resolve_ask(deny) → 拒绝结果帧 + 取消终态，
/// 文件不落盘。
#[tokio::test]
async fn ask_approval_deny_finishes_cancelled() {
    let (app, events, root) = fixture("sess-deny");
    let sink = SharedSink::default();
    let models = vec![
        stream_model(vec![stream_turn_with_tool(
            "w-1",
            "write",
            serde_json::json!({"path": "deny-e2e.txt", "content": "不应写入"}),
        )]),
        unused_model(),
    ];
    start(
        &app,
        "sess-deny",
        vec![user_msg("u1", "写一个文件")],
        run_config(PermissionTier::ConfirmEach),
        sink.clone(),
        models,
    )
    .await
    .expect("start_chat 成功");

    let opened = wait_event(&events, keys::ASK_OPENED, WAIT).await;
    let ask_id = opened["askId"].as_str().expect("askId").to_owned();
    super::resolve_ask(
        &app,
        super::ResolveAskArgs {
            ask_id,
            answer: crate::backend::ask::AskAnswer::Approval {
                option_id: "deny".to_owned(),
                edited_command: None,
            },
        },
    )
    .await
    .expect("应答投递成功");

    // 取消终态（deny 收场 = AwaitingApproval → Cancelled）
    wait_event(&events, keys::RUN_CANCELLED, WAIT).await;
    let frames = sink.frames();
    let mut denied_result = false;
    for f in &frames {
        if let ChatFrame::ToolResult { ok: false, error, .. } = &f.frame {
            assert_eq!(
                error.as_ref().map(|e| e.code.as_str()),
                Some("E_CANCELLED"),
                "拒绝结果帧错误码 = E_CANCELLED"
            );
            denied_result = true;
        }
    }
    assert!(denied_result, "应有拒绝补发结果帧");
    if let ChatFrame::RunEnded { reason, .. } = &frames.last().expect("帧").frame {
        assert_eq!(reason, &EndReason::Cancelled);
    }
    // 文件不落盘（挂起时批内零执行）
    assert!(!root.join("workspace").join("deny-e2e.txt").exists());
    assert!(app.runs.get("sess-deny").is_none());
}

// ---- 场景四：手动压缩（compact_session） ----

/// 预置史册三条消息 → 手动压缩：摘要模型执行、边界落盘、前后 token 返回。
#[tokio::test]
async fn compact_session_summarizes_and_persists_boundary() {
    let (app, events, _root) = fixture("sess-compact");
    // 预置史册：三条 user/assistant 交替（压缩范围需 ≥2 条）
    app.store
        .append_canonical(
            "sess-compact",
            &[
                crate::storage::StoredEntry::Message(user_msg("m1", "第一条消息")),
                crate::storage::StoredEntry::Message(MessageEntry {
                    id: "m2".to_owned(),
                    parent_id: Some("m1".to_owned()),
                    role: MessageRole::Assistant,
                    parts: vec![WirePart::Text {
                        text: "第一条回复".to_owned(),
                    }],
                    created_at: 1_700_000_000_001,
                    run_id: None,
                    status: None,
                }),
                crate::storage::StoredEntry::Message(MessageEntry {
                    id: "m3".to_owned(),
                    parent_id: Some("m2".to_owned()),
                    role: MessageRole::User,
                    parts: vec![WirePart::Text {
                        text: "第二条消息".to_owned(),
                    }],
                    created_at: 1_700_000_000_002,
                    run_id: None,
                    status: None,
                }),
            ],
        )
        .expect("史册写入");
    let models = vec![
        // compact_session 解析序：主模型（窗口面）→ 摘要模型（执行压缩）
        unused_model(),
        unary_model("用户在讨论两条测试消息。"),
    ];
    let result = super::compact_session_with(
        &app,
        super::CompactSessionArgs {
            session_id: "sess-compact".to_owned(),
        },
        mock_resolver(models),
    )
    .await
    .expect("压缩成功");
    let cut = result.cut_entry_id.expect("应有压缩边界");
    assert!(result.tokens_after < result.tokens_before, "压缩后占用应下降");
    // 边界落盘（史册 compaction 条目）
    let overview = app.store.compaction_overview("sess-compact").expect("概览");
    let (entry, _) = overview;
    assert_eq!(entry.expect("边界条目").cut_entry_id, cut);
    // 摘要用量照发 tokens:update（aux，runs 计数不增）
    wait_event(&events, keys::TOKENS_UPDATE, WAIT).await;
    let detail = app.ledger.detail.load("sess-compact");
    assert!(
        detail
            .runs
            .keys()
            .any(|rid| rid.ends_with(":compact") || rid.contains(":compact")),
        "应有 aux 压缩明细：{:?}",
        detail.runs.keys().collect::<Vec<_>>()
    );
}

// ---- 场景五：followUp 队列（失败暂停 → resume 投递新 run） ----

/// 队列失败暂停后 resume_followup_queue：解除暂停并投递队列头新开 run，
/// 新 run 走完整装配链到 run:done。
#[tokio::test]
async fn resume_followup_queue_delivers_head_as_new_run() {
    let (app, events, _root) = fixture("sess-queue");
    // 队列一条 + 置失败暂停态（模拟上一 run 失败收尾）
    let entry = super::followup_entry_from_text(
        "排队的问题",
        &run_config(PermissionTier::FullAccess),
        Vec::new(),
    );
    app.followups.push("sess-queue", entry);
    app.followups.set_paused("sess-queue", true);
    let models = vec![
        // 投递的新 run：一轮纯正文（主模型）+ 占位摘要
        stream_model(vec![stream_text_turn("已处理排队消息。")]),
        unused_model(),
    ];
    let result = super::resume_followup_queue_with(
        &app,
        super::ResumeFollowupQueueArgs {
            session_id: "sess-queue".to_owned(),
        },
        mock_resolver(models),
    )
    .await
    .expect("resume 成功");
    assert!(result.delivered, "队列非空且无活跃 run → 应立即投递");
    let new_run_id = result.run_id.expect("投递应返回新 runId");

    // 新 run 走全链到终态；排队文本入树（persist_user_fallback 兜底）
    let done = wait_event(&events, keys::RUN_DONE, WAIT).await;
    assert_eq!(done["runId"], new_run_id);
    let tree = app.store.load_session_tree("sess-queue").expect("史册");
    assert!(
        tree.messages
            .iter()
            .any(|m| m.role == MessageRole::User && m.parts.iter().any(|p| matches!(p, WirePart::Text { text } if text == "排队的问题"))),
        "投递的 user 条目应入树"
    );
    // 队列已空（run:queue 事件 size=0）
    let queue = events
        .payloads_of(keys::RUN_QUEUE)
        .last()
        .expect("应有队列事件")
        .clone();
    assert_eq!(queue["size"], 0);
    assert_eq!(queue["paused"], false);
    assert!(app.runs.get("sess-queue").is_none());
}

// ---- 场景六：装配函数冒烟（临时目录 → api 面就绪） ----

/// AppState::assemble：临时目录装配 → 列会话/创建/删除全通过 → 再装配
/// 同目录不炸（幂等启动）。
#[tokio::test]
async fn assemble_smoke_session_lifecycle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    let events = Arc::new(RecordingEventSink::new());
    let app = AppState::assemble(root.clone(), events.clone(), None);

    // api 面：创建 → 清单可见 → 删除
    let created = crate::api::session::create_session(
        &app,
        crate::api::session::CreateSessionArgs { project_id: None },
    )
    .expect("创建会话");
    assert_eq!(created.title, DEFAULT_SESSION_TITLE);
    let listed = crate::api::session::list_sessions(
        &app,
        crate::api::session::ListSessionsArgs { project_id: None },
    )
    .expect("列会话");
    assert!(listed.iter().any(|m| m.id == created.id));
    crate::api::session::delete_session(
        &app,
        crate::api::session::DeleteSessionArgs {
            session_id: created.id.clone(),
        },
    )
    .expect("删除会话");
    let listed = crate::api::session::list_sessions(
        &app,
        crate::api::session::ListSessionsArgs { project_id: None },
    )
    .expect("列会话");
    assert!(!listed.iter().any(|m| m.id == created.id));

    // 数据目录落位正确（logs/ 在装配根下；config.json 为惰性落盘——
    // 首次 save_config 才写文件，装配只持路径句柄）
    assert!(root.join("logs").is_dir(), "日志根应在装配根");

    // 启动序记账已写 app 行
    // （重复装配同目录：幂等——第二次启动不因既有文件失败）
    let _second = AppState::assemble(root, events.clone(), None);
}

/// RunManager 参数面冒烟：同一注册表句柄（app.runs 与独立 Clone 同源）。
#[tokio::test]
async fn run_manager_shared_handle_semantics() {
    let (app, _events, _root) = fixture("sess-runs");
    let runs: RunManager = app.runs.clone();
    let hub = Arc::new(crate::agent::FrameHub::new(Box::new(SharedSink::default())));
    let handle = runs
        .start("sess-runs", "run-x".to_owned(), 0, hub)
        .expect("注册成功");
    assert!(Arc::ptr_eq(&handle, &app.runs.get("sess-runs").expect("同源")));
    assert!(app.runs.cancel("sess-runs"));
    runs.remove("sess-runs", "run-x");
    assert!(app.runs.get("sess-runs").is_none());
}
