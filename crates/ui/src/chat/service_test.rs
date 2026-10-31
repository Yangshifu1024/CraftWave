//! 引擎会话服务测试（gpui-rewrite 2.C-2b 验收，`#[gpui_kit::test]` 独立文件）。
//!
//! 三组：
//! 1. **管道单元**：假 sink 推一段帧序列 → FrameAggregator → 快照 →
//!    `ConversationState::apply_snapshot`（历史 + 流式 + 终态收口）；
//! 2. **集成**（tempdir 装配 + rig 剧本假模型，经 engine `test-utils`
//!    接缝 `start_chat_test` 注入 resolver——与 engine 自身 `run_test.rs`
//!    同一条接缝；壳外可用的 mock 通道只有它，`mock_chat` 已随阶段 1
//!    移除）：真实 start_chat 在 tokio 跑 → 帧经通道/泵回主线程 →
//!    节流快照上屏 → cancel_run → 终态快照收口 + 历史重载；
//! 3. **composer 集成**：Enter → 会话创建 + 发送链路被调（未配模型的
//!    错误路径：合成错误终态帧收口——编排与错误面都被钉住，不起
//!    provider）。

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};

use rig::completion::Usage;
use rig::test_utils::{MockCompletionModel, MockRuntime, MockScript, MockStreamEvent, MockTurn};

use craftwave_engine::api::run::{
    self, DEFAULT_MAX_TURNS, ModelResolver, ResolvedModel, RunConfig, StartChatArgs,
    SessionPendingAskArgs,
};
use craftwave_engine::api::session::{
    self, CreateSessionArgs, LoadSessionTreeArgs, RenameSessionArgs, SaveSessionTreeArgs,
};
use craftwave_engine::app_state::AppState;
use craftwave_engine::backend::frames::{ChatFrame, EndReason, SequencedFrame};
use craftwave_engine::safety::tier::PermissionTier;
use craftwave_engine::snapshot::{FrameAggregator, RunInfo, StreamPart, SNAPSHOT_INTERVAL};
use craftwave_engine::storage::{MessageEntry, MessageRole, WirePart};
use craftwave_engine::util::ids::new_uuid;

use crate::MainWindowView;
use crate::engine::{EngineBridge, FrameBridge, MailEventSink, UiMail, install_runtime};
use crate::theme::{self, CraftwaveTheme};

use super::service::SessionService;
use super::state::ConversationState;

/// 事件/帧等待超时（tokio 侧真实时间；run 是后台任务，测试轮询让出即推进）。
const WAIT: Duration = Duration::from_secs(10);

// ======================================================================
// 装置
// ======================================================================

/// 临时数据根装配（engine run_test.rs 的 fixture 同款：tempdir 用
/// `std::mem::forget` 持有到测试进程结束）。返回 (状态, 事件汇, 数据根)。
fn fixture_app() -> (Arc<AppState>, Arc<MailEventSink>, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    let events = Arc::new(MailEventSink::new());
    // AppState::assemble 产物即 Arc
    let state = AppState::assemble(root.clone(), events.clone(), None);
    (state, events, root)
}

/// 建一个已改名的会话（标题非默认——绕开自动起名任务，测试不消耗额外
/// 剧本）。返回会话 id。
fn named_session(state: &AppState) -> String {
    let meta =
        session::create_session(state, CreateSessionArgs::default()).expect("create_session");
    session::rename_session(
        state,
        RenameSessionArgs {
            session_id: meta.id.clone(),
            title: "测试会话（已命名）".to_owned(),
        },
    )
    .expect("rename_session");
    meta.id
}

/// user 消息条目（wire 校验要求末条为 user）。
fn user_msg(text: &str) -> MessageEntry {
    MessageEntry {
        id: new_uuid(),
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

/// 发送前树同步（与 `SessionService::persist_user` 同口径：load → 追加
/// user 挂活动叶 → save；新会话空树按空处理）。返回落盘后的 user 条目。
fn persist_user(state: &AppState, session_id: &str, mut entry: MessageEntry) -> MessageEntry {
    let tree = match session::load_session_tree(
        state,
        LoadSessionTreeArgs {
            session_id: session_id.to_owned(),
        },
    ) {
        Ok(tree) => tree,
        // 新会话首条消息：历史文件随首次 save 产生（load 报 E_NOT_FOUND）
        Err(err) if err.code == craftwave_engine::util::errors::E_NOT_FOUND => {
            craftwave_engine::storage::SessionTreeData {
                session_id: session_id.to_owned(),
                messages: Vec::new(),
                compactions: Vec::new(),
                active_leaf_id: None,
                revision: 0,
            }
        }
        Err(err) => panic!("load tree: {err:?}"),
    };
    entry.parent_id = tree.active_leaf_id.clone();
    let mut messages = tree.messages;
    messages.push(entry.clone());
    session::save_session_tree(
        state,
        SaveSessionTreeArgs {
            session_id: session_id.to_owned(),
            messages,
            active_leaf_id: Some(entry.id.clone()),
            base_revision: tree.revision,
        },
    )
    .expect("save tree");
    entry
}

/// 剧本 usage（记账口径，engine run_test 同款）。
fn scripted_usage() -> Usage {
    Usage::default().input_tokens(120u64).output_tokens(48u64)
}

/// 纯正文流式 turn（带 usage）。
fn stream_text_turn(text: &str) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::text(text),
        MockStreamEvent::final_response(scripted_usage()),
    ]
}

/// 工具调用流式 turn：正文 + write 调用 + 收口（ConfirmEach 档下 write
/// 会在执行前挂审批——取消场景的挂起窗口）。
fn stream_write_turn(path: &str, content: &str) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::text("我来创建文件。"),
        MockStreamEvent::tool_call(
            "call-1",
            "write",
            serde_json::json!({ "path": path, "content": content }),
        ),
        MockStreamEvent::final_response(scripted_usage()),
    ]
}

/// 主 run 的剧本模型。
fn stream_model(turns: Vec<Vec<MockStreamEvent>>) -> MockCompletionModel {
    MockCompletionModel::from_stream_turns(turns)
}

/// 占位摘要模型（场景不触发压缩，永不被消费；防御脚本错位）。
fn unused_model() -> MockCompletionModel {
    MockCompletionModel::from_turns([MockTurn::text("unused").with_usage(scripted_usage())])
}

/// 剧本假模型解析器（engine run_test.rs 的 mock_resolver 同款：每次解析
/// 弹出一个预置模型；解析次数超预期 panic——装配面漂移的显式信号）。
fn mock_resolver(models: Vec<MockCompletionModel>) -> ModelResolver<MockScript, MockRuntime> {
    use std::collections::VecDeque;
    use std::sync::Mutex;
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
            api_format: craftwave_engine::providers::types::ApiFormat::OpenaiChat,
            reasoning_effort: None,
            base_url: "http://mock.local".to_owned(),
        })
    })
}

/// 常驻 tokio 运行时（每测试一个多线程实例；`std::mem::forget` 持到测试
/// 进程结束——run 任务跨 await 点，句柄 drop 会取消未完成任务）。
fn spawn_runtime() -> Arc<tokio::runtime::Runtime> {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    Arc::new(rt)
}

/// 测试会话窗口：tempdir 装配 + conversation + SessionService（tokio
/// runtime 常驻）。返回 (会话实体, 服务实体, 状态, 调度器句柄, 视觉上下文)。
/// 调度器句柄供节流快照的 fake clock 推进（`advance_clock` 内部会同步
/// 跑任务，不能在 `visual.update` 的 App 借用内调用——用 executor 句柄
/// 在借用外推进）。
fn service_window(
    cx: &mut TestAppContext,
) -> (
    Entity<ConversationState>,
    Entity<SessionService>,
    Arc<AppState>,
    gpui_kit::BackgroundExecutor,
    &mut VisualTestContext,
) {
    let (state, events, _root) = fixture_app();
    let runtime = spawn_runtime();
    let executor = cx.executor();
    cx.update(|cx| {
        gpui_kit::init(cx);
        CraftwaveTheme::mount(cx);
        theme::apply(cx);
    });
    let state_for_view = state.clone();
    let events_for_view = events.clone();
    let (view, visual) = cx.add_window_view(move |_window, cx| {
        let conversation = cx.new(|_| ConversationState::new());
        let service = cx.new(|_| {
            SessionService::new(
                state_for_view.clone(),
                events_for_view.clone(),
                runtime,
                conversation.downgrade(),
            )
        });
        conversation.update(cx, |conversation, _| {
            conversation.service = Some(service.clone());
        });
        MainWindowView {
            zoom_seen: 0.0,
            chat: conversation,
        }
    });
    let chat = visual.update(|_window, cx| view.read(cx).chat.clone());
    let service = visual.update(|_window, cx| {
        view.read(cx)
            .chat
            .read(cx)
            .service
            .clone()
            .expect("service 已登记")
    });
    (chat, service, state, executor, visual)
}

/// 等待条件成立（轮询 + 推进 fake clock + 让出 gpui 调度器）。tokio 在
/// 真实线程推进（帧产出）；心跳循环的 timer 是 gpui fake clock——只有
/// advance 才到期，所以每轮推一个节流窗口（等价生产中的时间流逝）。
fn wait_for(
    executor: &gpui_kit::BackgroundExecutor,
    visual: &mut VisualTestContext,
    mut pred: impl FnMut(&mut VisualTestContext) -> bool,
    label: &str,
) {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        executor.advance_clock(SNAPSHOT_INTERVAL);
        visual.run_until_parked();
        if pred(visual) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "等待超时：{label}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 推进一个节流窗口并跑完 gpui 任务（快照上屏恰好一步）。fake clock 经
/// executor 句柄推进——`advance_clock` 内部会同步跑任务（borrow App），
/// 必须在 `visual.update` 的 App 借用之外调用。
fn tick_snapshot(executor: &gpui_kit::BackgroundExecutor, visual: &mut VisualTestContext) {
    executor.advance_clock(SNAPSHOT_INTERVAL);
    visual.run_until_parked();
}

/// 发起一次真实 run：起管道（busy + 心跳循环）→ 剧本 resolver 注入的
/// start_chat_test 跑 tokio，帧经 FrameBridge 回主线程。`args` 的
/// run_config.tier 决定档位（取消场景用 ConfirmEach 触发审批挂起）。
/// 集成场景 2/3 共用。
fn spawn_scripted_run(
    visual: &mut VisualTestContext,
    service: &Entity<SessionService>,
    state: &Arc<AppState>,
    runtime: &Arc<tokio::runtime::Runtime>,
    args: StartChatArgs,
    resolver: ModelResolver<MockScript, MockRuntime>,
) {
    let mailbox = visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.begin_pipeline(cx))
    });
    let state_for_run = state.clone();
    runtime.spawn(async move {
        let result = run::start_chat_test(
            &state_for_run,
            &state_for_run.runs,
            args,
            Box::new(FrameBridge::new(mailbox.clone())),
            false,
            resolver,
        )
        .await;
        let _ = mailbox.send(UiMail::Started(result.map(|r| r.run_id)));
    });
}

/// 集成场景的运行配置（档位入参；其余缺省——与 engine run_test 同款）。
fn run_config(tier: PermissionTier) -> RunConfig {
    RunConfig {
        model_id: None,
        thinking: None,
        tier,
        max_turns: DEFAULT_MAX_TURNS,
        steer: false,
    }
}

// ======================================================================
// 1. 管道单元：帧序列 → 聚合器 → 快照 → apply_snapshot
// ======================================================================

/// 假 sink 语义直推聚合器：RunStarted → 思考 → 正文 → 工具往返 →
/// runEnded。断言快照面（历史 + 流式 + 终态）与收口路径
/// （apply_snapshot(new_history, None) 后流式行由历史行接管）。
#[gpui_kit::test]
fn pipeline_delivers_frames_to_conversation(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        CraftwaveTheme::mount(cx);
        theme::apply(cx);
    });
    let (view, visual) = cx.add_window_view(|_window, cx| MainWindowView {
        zoom_seen: 0.0,
        chat: cx.new(|_| ConversationState::new()),
    });
    visual.run_until_parked();
    let chat = visual.update(|_window, cx| view.read(cx).chat.clone());

    // 假 sink 推帧序列（seq 严格递增）
    let aggregator = Arc::new(FrameAggregator::new());
    let history: Rc<Vec<Rc<MessageEntry>>> = Rc::new(vec![Rc::new(user_msg("你好"))]);
    let frames = vec![
        ChatFrame::RunStarted {
            run_id: "run-1".to_owned(),
            model: "mock".to_owned(),
        },
        ChatFrame::ReasoningDelta {
            turn: 1,
            delta: "想想。".to_owned(),
        },
        ChatFrame::TextDelta {
            turn: 1,
            delta: "你好，".to_owned(),
        },
        ChatFrame::TextDelta {
            turn: 1,
            delta: "我在。".to_owned(),
        },
        ChatFrame::ToolStarted {
            call_id: "call-1".to_owned(),
            name: "read".to_owned(),
        },
        ChatFrame::ToolResult {
            call_id: "call-1".to_owned(),
            ok: true,
            ui_summary: "已读取".to_owned(),
            error: None,
        },
        ChatFrame::RunEnded {
            reason: EndReason::Done,
            duration_ms: 42,
            tool_count: 1,
            turn: 1,
            error: None,
        },
    ];
    for (seq, frame) in frames.into_iter().enumerate() {
        aggregator.push(SequencedFrame {
            seq: seq as u64,
            ts: None,
            frame,
        });
    }

    // 快照面：终态已收在聚合器，产出的流式态含合并 parts + 终态
    let streaming = aggregator.snapshot_streaming().expect("有流式态");
    assert_eq!(
        streaming.run,
        Some(RunInfo {
            run_id: "run-1".to_owned(),
            model: "mock".to_owned(),
        })
    );
    assert!(streaming.ended.is_some(), "终态帧应已收在快照里");
    // 工具为界合批：思考段 → 正文段 → 工具卡
    assert_eq!(streaming.parts.len(), 3);
    assert!(matches!(&streaming.parts[0], StreamPart::Reasoning { .. }));
    assert!(matches!(
        &streaming.parts[1],
        StreamPart::Text { text } if text == "你好，我在。"
    ));
    assert!(matches!(
        &streaming.parts[2],
        StreamPart::ToolCall { call_id, .. } if call_id == "call-1"
    ));

    // 上屏：历史 1 行 + 流式 1 行
    visual.update(|_window, cx| {
        chat.update(cx, |chat, cx| {
            chat.apply_snapshot(history, Some(streaming), cx);
            assert_eq!(chat.row_count(), 2);
            assert!(chat.streaming().is_some_and(|s| s.ended.is_some()));
        });
    });
    visual.run_until_parked();

    // 收口：落盘重载后的新历史 + 流式清空（流式行由历史行接管）
    let reloaded: Rc<Vec<Rc<MessageEntry>>> = Rc::new(vec![
        Rc::new(user_msg("你好")),
        Rc::new(MessageEntry {
            id: "assistant-1".to_owned(),
            parent_id: None,
            role: MessageRole::Assistant,
            parts: vec![WirePart::Text {
                text: "你好，我在。".to_owned(),
            }],
            created_at: 1_700_000_000_001i64,
            run_id: Some("run-1".to_owned()),
            status: None,
        }),
    ]);
    visual.update(|_window, cx| {
        chat.update(cx, |chat, cx| {
            chat.apply_snapshot(reloaded, None, cx);
            assert_eq!(chat.row_count(), 2, "流式行由历史行接管");
            assert!(chat.streaming().is_none(), "流式态应清空");
            assert_eq!(chat.messages()[1].id, "assistant-1");
        });
    });
    visual.run_until_parked();
}

// ======================================================================
// 2. 集成：真实 start_chat → 流式快照 → 自然终态收口 + 历史重载
// ======================================================================

#[gpui_kit::test]
fn start_chat_streams_into_conversation_and_finalizes(cx: &mut TestAppContext) {
    let (chat, service, state, executor, visual) = service_window(cx);
    let session_id = named_session(&state);
    let user = persist_user(&state, &session_id, user_msg("帮我算 2+3"));
    // 装载会话（历史 1 条 user → 快照上屏）
    let sid = session_id.clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.open_session(sid, cx));
    });
    visual.run_until_parked();
    visual.update(|_window, cx| {
        assert_eq!(chat.read(cx).row_count(), 1, "user 条目应已上屏");
    });

    // 剧本：单 turn 纯正文（无工具 → run 自然 Done）；摘要占位
    let resolver = mock_resolver(vec![
        stream_model(vec![stream_text_turn("计算完成，结果是 5。")]),
        unused_model(),
    ]);
    let runtime = spawn_runtime();
    spawn_scripted_run(
        &mut *visual,
        &service,
        &state,
        &runtime,
        StartChatArgs {
            session_id: session_id.clone(),
            messages: vec![user],
            run_config: run_config(PermissionTier::FullAccess),
        },
        resolver,
    );

    // 流式帧到达聚合器（真实 tokio 时间）
    wait_for(
        &executor,
        visual,
        |visual| {
            visual.update(|_window, cx| {
                service
                    .read(cx)
                    .aggregator()
                    .snapshot_streaming()
                    .is_some_and(|streaming| !streaming.parts.is_empty())
            })
        },
        "流式帧到达聚合器",
    );
    // 快照上屏：流式行出现（正文增量）+ busy
    tick_snapshot(&executor, visual);
    visual.update(|_window, cx| {
        let streaming = chat.read(cx).streaming().expect("流式行应已上屏");
        assert!(streaming.parts.iter().any(|part| matches!(part, StreamPart::Text { .. })));
        assert!(service.read(cx).busy(), "run 进行中 busy 应为 true");
    });

    // 自然终态：runEnded(Done) 帧到达 → 终态快照已随心跳轮上屏
    // （drain 与 apply 在同一轮完成；wait_for 的 is_terminal 判定基于
    // 聚合器——快照面是易逝中间态，重载轮之后就没了）
    wait_for(
        &executor,
        visual,
        |visual| {
            visual.update(|_window, cx| service.read(cx).aggregator().is_terminal())
        },
        "自然终态帧到达",
    );
    // 终态快照断言：advance_clock 会连跑窗口内全部到期节拍——终态上屏
    // 与重载收口可能在同一次推进内完成，快照面是易逝中间态。在场则必须
    // 是 Done 终态；不在场即已收口（收口断言在下方）
    visual.update(|_window, cx| {
        if let Some(streaming) = chat.read(cx).streaming() {
            let ended = streaming.ended.as_ref().expect("终态帧在场必有 reason");
            assert_eq!(ended.reason, EndReason::Done);
        }
    });
    // 重载收口：历史接管（assistant 正文在史册）+ busy 复位
    // （已收口时本步空转，幂等）
    tick_snapshot(&executor, visual);
    visual.update(|_window, cx| {
        assert!(chat.read(cx).streaming().is_none(), "收口后流式行应清空");
        assert!(!service.read(cx).busy(), "收口后 busy 应复位");
        let messages = chat.read(cx).messages();
        assert!(
            messages.iter().any(|entry| {
                entry.role == MessageRole::Assistant
                    && entry.parts.iter().any(|part| matches!(
                        part,
                        WirePart::Text { text } if text.contains("计算完成")
                    ))
            }),
            "assistant 正文应经落盘重载进史册"
        );
    });
}

// ======================================================================
// 3. 集成：cancel_run → 终态快照收口（审批挂起窗口取消）
// ======================================================================

#[gpui_kit::test]
fn cancel_run_yields_cancelled_terminal(cx: &mut TestAppContext) {
    let (chat, service, state, executor, visual) = service_window(cx);
    let session_id = named_session(&state);
    let user = persist_user(&state, &session_id, user_msg("创建一个文件"));
    let sid = session_id.clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.open_session(sid, cx));
    });
    visual.run_until_parked();

    // 剧本：write 工具调用（ConfirmEach 档 → 执行前挂审批——取消的窗口）
    let resolver = mock_resolver(vec![
        stream_model(vec![stream_write_turn("hello.txt", "hi")]),
        unused_model(),
    ]);
    let runtime = spawn_runtime();
    spawn_scripted_run(
        &mut *visual,
        &service,
        &state,
        &runtime,
        StartChatArgs {
            session_id: session_id.clone(),
            messages: vec![user],
            run_config: run_config(PermissionTier::ConfirmEach),
        },
        resolver,
    );

    // 等审批挂起（引擎 ask 槽出现）
    let ask_state = state.clone();
    let ask_sid = session_id.clone();
    wait_for(
        &executor,
        visual,
        move |visual| {
            visual.update(|_window, _cx| {
                run::session_pending_ask(
                    &ask_state,
                    SessionPendingAskArgs {
                        session_id: ask_sid.clone(),
                    },
                )
                .expect("pending ask 查询成功")
                .is_some()
            })
        },
        "审批挂起",
    );

    // cancel_run：置位取消令牌（引擎补发 runEnded(cancelled)）
    let cancelled = run::cancel_run(&state, &session_id).expect("cancel_run 成功");
    assert!(cancelled, "取消应有活跃 run 被置位");

    // 终态帧到达 → 终态快照已随心跳轮上屏（reason = cancelled）
    wait_for(
        &executor,
        visual,
        |visual| {
            visual.update(|_window, cx| service.read(cx).aggregator().is_terminal())
        },
        "取消终态帧到达",
    );
    // 终态快照断言（同场景 2：快照面是易逝中间态）
    visual.update(|_window, cx| {
        if let Some(streaming) = chat.read(cx).streaming() {
            let ended = streaming.ended.as_ref().expect("终态帧在场必有 reason");
            assert_eq!(ended.reason, EndReason::Cancelled);
        }
    });
    // 重载收口：busy 复位 + 流式清空（已收口时本步空转，幂等）
    tick_snapshot(&executor, visual);
    visual.update(|_window, cx| {
        assert!(chat.read(cx).streaming().is_none(), "收口后流式行应清空");
        assert!(!service.read(cx).busy(), "收口后 busy 应复位");
    });
}

// ======================================================================
// 4. composer 集成：Enter → 会话创建 + 发送链路被调（错误终态收口）
// ======================================================================

#[gpui_kit::test]
fn composer_enter_creates_session_and_dispatches_send(cx: &mut TestAppContext) {
    // 未配模型的 tempdir 装配：send_message 走生产 start_chat → 模型解析
    // 失败 → Started(Err) → 合成错误终态帧收口。整条编排被钉住。
    let (state, events, _root) = fixture_app();
    // 调度器句柄（tick_snapshot 的 fake clock 推进在 App 借用外进行）
    let executor = cx.executor();
    cx.update(|cx| {
        install_runtime(cx);
        gpui_kit::init(cx);
        CraftwaveTheme::mount(cx);
        theme::apply(cx);
    });
    let bridge = EngineBridge {
        state: state.clone(),
        events: events.clone(),
    };
    let (view, visual) = cx.add_window_view(|window, cx| MainWindowView {
        zoom_seen: 0.0,
        chat: crate::chat::install(window, cx, Some(&bridge)),
    });
    visual.run_until_parked();
    let chat = visual.update(|_window, cx| view.read(cx).chat.clone());

    // 聚焦输入框 → 打字 → Enter（真通道：defer 后 send_message 编排）
    visual.update(|window, cx| {
        let input = chat
            .read(cx)
            .composer()
            .expect("install 已装配 composer")
            .read(cx)
            .input()
            .clone();
        input.update(cx, |input_state, cx| input_state.focus(window, cx));
    });
    visual.simulate_input("你好");
    visual.run_until_parked();
    visual.simulate_keystrokes("enter");

    // 会话已创建 + user 气泡上屏 + busy 置位
    let index_state = state.clone();
    wait_for(
        &executor,
        visual,
        |visual| {
            visual.update(|_window, cx| {
                !index_state.store.index().list().is_empty()
                    && chat.read(cx).row_count() == 1
                    && chat.read(cx).messages().first().is_some_and(|entry| {
                        entry.role == MessageRole::User
                            && entry.parts.iter().any(|part| {
                                matches!(part, WirePart::Text { text } if text == "你好")
                            })
                    })
            })
        },
        "会话创建 + user 上屏",
    );
    visual.update(|_window, cx| {
        let _service = cx
            .try_global::<crate::chat::EngineService>()
            .expect("install 应挂服务 Global")
            .0
            .clone();
        assert_eq!(
            state.store.index().list().len(),
            1,
            "应恰好创建一个会话"
        );
    });

    // 模型解析失败 → 合成错误终态帧 → 心跳轮上屏 → 重载收口。
    // 等待判定用聚合器（快照面的 ended 是易逝中间态，重载轮后即清空）
    let service_for_wait = visual.update(|_window, cx| {
        cx.try_global::<crate::chat::EngineService>()
            .expect("install 应挂服务 Global")
            .0
            .clone()
    });
    wait_for(
        &executor,
        visual,
        move |visual| {
            visual.update(|_window, cx| service_for_wait.read(cx).aggregator().is_terminal())
        },
        "错误终态帧到达",
    );
    // 终态快照断言（同场景 2：快照面是易逝中间态，命令失败应带错误载荷）
    visual.update(|_window, cx| {
        if let Some(streaming) = chat.read(cx).streaming() {
            let ended = streaming.ended.as_ref().expect("终态帧在场必有 reason");
            assert_eq!(ended.reason, EndReason::Error);
            assert!(ended.error.is_some(), "命令失败应带错误载荷");
        }
    });
    // 重载收口（已收口时本步空转，幂等）
    tick_snapshot(&executor, visual);
    visual.update(|_window, cx| {
        assert!(chat.read(cx).streaming().is_none(), "收口后流式行应清空");
        let service = cx
            .try_global::<crate::chat::EngineService>()
            .expect("服务在")
            .0
            .clone();
        assert!(!service.read(cx).busy(), "收口后 busy 应复位");
    });
}

// ======================================================================
// 5. 附件链路：attach_image_from_path（engine attach_image 落盘 + mime
//    回读）——文件选择器阶段 4 接 rfd 前的落盘通道验证
// ======================================================================

#[gpui_kit::test]
fn attach_image_from_path_lands_blob_and_returns_mime(cx: &mut TestAppContext) {
    let (_chat, service, state, _executor, visual) = service_window(cx);
    // 最小 PNG：文件头魔数（attach_image 按魔数嗅探，字节内容不影响落盘）
    let png: &[u8] = &[
        0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', // 签名
        0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D', b'R', // IHDR 长度 + 类型
    ];
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("shot.png");
    std::fs::write(&path, png).expect("写入测试图片");
    let path_string = path.to_string_lossy().into_owned();

    let attachment = visual
        .update(|_window, cx| {
            service.update(cx, |service, cx| {
                service.attach_image_from_path(&path_string, cx)
            })
        })
        .expect("attach_image_from_path 成功");
    assert_eq!(attachment.mime_type, "image/png", "mime 应为魔数嗅探结果");
    assert!(!attachment.blob_hash.is_empty(), "应返回内容寻址哈希");
    // 落盘可读：blob 已在会话 imgblob 目录
    let session_id = visual.update(|_window, cx| {
        service.read(cx).session_id().expect("附件落盘应已建会话").to_owned()
    });
    let (_bytes, mime) = craftwave_engine::storage::get_blob(
        state.store.root(),
        &session_id,
        &attachment.blob_hash,
    )
    .expect("blob 应已落盘");
    assert_eq!(mime, "image/png");
}
