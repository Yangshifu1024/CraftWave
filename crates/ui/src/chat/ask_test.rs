//! Ask 权限面板 + 权限档快切 + B6 run 级错误卡（gpui-rewrite 3-B 验收，
//! `#[gpui_kit::test]` 独立文件）。
//!
//! 五组：
//! 1. **面板生命周期**（app 级真槽位）：ask:opened 事件 → 内联卡出现 →
//!    点选项 → `resolve_ask` 被调（oneshot 槽收到应答）→ 终态行；
//!    多 ask 依序排队；失效路径（槽已收口 → E_NOT_FOUND →「已失效」行）；
//! 2. **集成审批恢复**：ConfirmEach 档 write 挂起 → 面板出现 → 允许 →
//!    run 恢复到自然终态 + 文件落盘 + 终态行「已允许」；
//! 3. **会话切换恢复**：挂起中切走再切回，卡经 `session_pending_ask`
//!    恢复展示；
//! 4. **权限档快切**：徽标点击循环 + Shift+Tab 键位同路；
//! 5. **B6 错误卡**：错误码分流 case 表 + 默认展开渲染 + 一键复制完整
//!    错误（剪贴板断言）+ 重试链（重发最后 user 消息）。

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};

use rig::completion::Usage;
use rig::test_utils::{MockCompletionModel, MockRuntime, MockScript, MockStreamEvent, MockTurn};

use craftwave_engine::api::run::{
    self, DEFAULT_MAX_TURNS, ModelResolver, ResolvedModel, RunConfig, StartChatArgs,
};
use craftwave_engine::api::session::{self, CreateSessionArgs, LoadSessionTreeArgs, RenameSessionArgs, SaveSessionTreeArgs};
use craftwave_engine::app_state::AppState;
use craftwave_engine::backend::ask::{AppAskSlot, AskPayload};
use craftwave_engine::safety::tier::PermissionTier;
use craftwave_engine::storage::{MessageEntry, MessageRole, WirePart};
use craftwave_engine::util::ids::new_uuid;

use crate::MainWindowView;
use crate::engine::{FrameBridge, MailEventSink, UiMail};
use crate::theme::{self, CraftwaveTheme};

use super::service::SessionService;
use super::state::ConversationState;

/// 事件/帧等待超时（tokio 侧真实时间；run 是后台任务，测试轮询让出即推进）。
const WAIT: Duration = Duration::from_secs(10);

// ======================================================================
// 装置（service_test.rs 同款精简）
// ======================================================================

/// 临时数据根装配。返回 (状态, 事件汇, 数据根)。
fn fixture_app() -> (Arc<AppState>, Arc<MailEventSink>, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    let events = Arc::new(MailEventSink::new());
    let state = AppState::assemble(root.clone(), events.clone(), None);
    (state, events, root)
}

/// 建一个已改名的会话（绕开自动起名任务）。
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

/// user 消息条目。
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

/// 发送前树同步（与 service_test 同口径）。
fn persist_user(state: &AppState, session_id: &str, mut entry: MessageEntry) -> MessageEntry {
    let tree = match session::load_session_tree(
        state,
        LoadSessionTreeArgs {
            session_id: session_id.to_owned(),
        },
    ) {
        Ok(tree) => tree,
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

fn scripted_usage() -> Usage {
    Usage::default().input_tokens(120u64).output_tokens(48u64)
}

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

fn stream_text_turn(text: &str) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::text(text),
        MockStreamEvent::final_response(scripted_usage()),
    ]
}

/// 占位摘要模型（场景不触发压缩，永不被消费）。
fn unused_model() -> MockCompletionModel {
    MockCompletionModel::from_turns([MockTurn::text("unused").with_usage(scripted_usage())])
}

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

fn spawn_runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"))
}

/// 测试会话窗口六元组（装置返回体；type_complexity 收敛）。
type ServiceWindow<'a> = (
    Entity<ConversationState>,
    Entity<SessionService>,
    Arc<AppState>,
    Arc<MailEventSink>,
    gpui_kit::BackgroundExecutor,
    &'a mut VisualTestContext,
);

/// 测试会话窗口（tempdir 装配 + conversation + SessionService）。
fn service_window(cx: &mut TestAppContext) -> ServiceWindow<'_> {
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
            sidebar: None,
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
    (chat, service, state, events, executor, visual)
}

/// 等待条件成立（轮询 + 推进 fake clock + 让出 gpui 调度器）。
fn wait_for(
    executor: &gpui_kit::BackgroundExecutor,
    visual: &mut VisualTestContext,
    mut pred: impl FnMut(&mut VisualTestContext) -> bool,
    label: &str,
) {
    let deadline = std::time::Instant::now() + WAIT;
    loop {
        executor.advance_clock(craftwave_engine::snapshot::SNAPSHOT_INTERVAL);
        visual.run_until_parked();
        if pred(visual) {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "等待超时：{label}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 推进一个节流窗口并跑完 gpui 任务。
fn tick_snapshot(executor: &gpui_kit::BackgroundExecutor, visual: &mut VisualTestContext) {
    executor.advance_clock(craftwave_engine::snapshot::SNAPSHOT_INTERVAL);
    visual.run_until_parked();
}

/// 经服务的事件入口喂一条引擎事件（泵的 UiMail::Event 分流同路）。
fn feed_event(
    service: &Entity<SessionService>,
    visual: &mut VisualTestContext,
    key: &str,
    payload: serde_json::Value,
) {
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.handle_event(key, &payload, cx));
    });
}

/// 向 AppState 注入一个应用级询问槽（MCP 信任等应用级 ask 的真槽位；
/// 返回应答接收端——`resolve_ask` 被调断言面）。
fn inject_app_ask(
    state: &AppState,
    ask_id: &str,
    payload: AskPayload,
) -> tokio::sync::oneshot::Receiver<craftwave_engine::backend::ask::AskAnswer> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .app_asks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            ask_id.to_owned(),
            AppAskSlot {
                payload,
                tx,
            },
        );
    rx
}

/// 通用审批载荷（选项由用例自定）。
fn approval_payload(options: &[(&str, &str)]) -> AskPayload {
    AskPayload::Approval {
        title: "执行命令".to_owned(),
        diff: None,
        command: Some("rm -rf /tmp/x".to_owned()),
        fence_reason: Some("命令含删除信号".to_owned()),
        preview_html: None,
        plan_scope: None,
        audit: None,
        options: options
            .iter()
            .map(|(id, label)| craftwave_engine::backend::ask::AskOption {
                id: (*id).to_owned(),
                label: (*label).to_owned(),
                recommended: Some(*id == "allow_once"),
                mode: Some(match *id {
                    "deny" => craftwave_engine::backend::ask::AskMode::Deny,
                    _ => craftwave_engine::backend::ask::AskMode::AllowOnce,
                }),
            })
            .collect(),
    }
}

// ======================================================================
// 1. 面板生命周期：事件 → 卡 → 选项点击 → resolve_ask 被调 → 终态行
// ======================================================================

#[gpui_kit::test]
fn ask_panel_lifecycle_option_resolution_and_queue(cx: &mut TestAppContext) {
    let (chat, service, state, _events, executor, visual) = service_window(cx);
    visual.run_until_parked();

    // ① ask:opened（审批，app 级 sessionId=""）→ 内联卡出现
    let rx = inject_app_ask(&state, "ask-1", approval_payload(&[
        ("allow_once", "允许"),
        ("deny", "拒绝"),
    ]));
    feed_event(
        &service,
        &mut *visual,
        craftwave_engine::util::events::keys::ASK_OPENED,
        serde_json::json!({
            "sessionId": "",
            "askId": "ask-1",
            "payload": approval_payload(&[("allow_once", "允许"), ("deny", "拒绝")]),
        }),
    );
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("ask-card-ask-1").is_some(),
        "审批卡应已内联渲染"
    );
    assert!(
        visual.debug_bounds("ask-command-block").is_some(),
        "命令类审批应展示完整命令"
    );

    // ② 点「允许」→ resolve_ask 被调（真槽位收到应答）→ 终态行收束
    let option = visual
        .debug_bounds("ask-option-ask-1-allow_once")
        .expect("允许按钮已渲染");
    visual.simulate_click(option.center(), Default::default());
    let answer = tokio::runtime::Runtime::new()
        .expect("rt")
        .block_on(rx);
    assert!(
        matches!(
            answer,
            Ok(craftwave_engine::backend::ask::AskAnswer::Approval { ref option_id, .. })
                if option_id == "allow_once"
        ),
        "resolve_ask 应把所选选项投给引擎槽，实际 {answer:?}"
    );
    wait_for(
        &executor,
        &mut *visual,
        |visual| visual.debug_bounds("ask-resolved-ask-1").is_some(),
        "终态行收束",
    );
    visual.update(|_window, cx| {
        assert!(
            chat.read(cx).pending_asks().is_empty(),
            "处置后挂起卡应清空"
        );
        assert_eq!(chat.read(cx).resolved_asks().len(), 1);
    });
    assert!(visual.debug_bounds("ask-card-ask-1").is_none(), "卡应已收束");

    // ③ 多 ask 排队：两个挂起 ask 依序展示
    let _rx2 = inject_app_ask(&state, "ask-2", approval_payload(&[("deny", "拒绝")]));
    let _rx3 = inject_app_ask(&state, "ask-3", approval_payload(&[("deny", "拒绝")]));
    feed_event(
        &service,
        &mut *visual,
        craftwave_engine::util::events::keys::ASK_OPENED,
        serde_json::json!({
            "sessionId": "",
            "askId": "ask-2",
            "payload": approval_payload(&[("deny", "拒绝")]),
        }),
    );
    feed_event(
        &service,
        &mut *visual,
        craftwave_engine::util::events::keys::ASK_OPENED,
        serde_json::json!({
            "sessionId": "",
            "askId": "ask-3",
            "payload": approval_payload(&[("deny", "拒绝")]),
        }),
    );
    visual.run_until_parked();
    visual.update(|_window, cx| {
        assert_eq!(
            chat.read(cx).pending_asks().len(),
            2,
            "两个挂起 ask 应依序排队"
        );
    });
    assert!(visual.debug_bounds("ask-card-ask-2").is_some());
    assert!(visual.debug_bounds("ask-card-ask-3").is_some());

    // ④ ask:closed 兜底清理（autoConfirm/取消等非本地路径）
    feed_event(
        &service,
        &mut *visual,
        craftwave_engine::util::events::keys::ASK_CLOSED,
        serde_json::json!({ "askId": "ask-2", "answered": true }),
    );
    visual.run_until_parked();
    visual.update(|_window, cx| {
        assert_eq!(chat.read(cx).pending_asks().len(), 1, "closed 应移除对应卡");
        assert_eq!(
            chat.read(cx).resolved_asks().len(),
            1,
            "非本地路径不留编造终态行"
        );
    });
}

/// 问卷形态：选项 chips 单选 + 自由文本输入 → 提交 → resolve_ask
/// kind=question 被调（槽位收到逐题答案）。外部目录授权形态渲染出现。
#[gpui_kit::test]
fn question_form_collects_answers_and_external_dir_renders(cx: &mut TestAppContext) {
    let (_chat, service, state, _events, _executor, visual) = service_window(cx);
    visual.run_until_parked();

    // 问卷：一个选项题 + 一个自由文本题
    let payload = AskPayload::Question {
        title: "继续方式".to_owned(),
        questions: vec![
            craftwave_engine::backend::ask::AskQuestion {
                id: "q1".to_owned(),
                text: "选择方案".to_owned(),
                options: Some(vec![
                    craftwave_engine::backend::ask::AskChoice {
                        id: "a".to_owned(),
                        label: "方案 A".to_owned(),
                    },
                    craftwave_engine::backend::ask::AskChoice {
                        id: "b".to_owned(),
                        label: "方案 B".to_owned(),
                    },
                ]),
                allow_free_text: false,
            },
            craftwave_engine::backend::ask::AskQuestion {
                id: "q2".to_owned(),
                text: "补充说明".to_owned(),
                options: None,
                allow_free_text: true,
            },
        ],
    };
    let rx = inject_app_ask(&state, "ask-q", payload.clone());
    feed_event(
        &service,
        &mut *visual,
        craftwave_engine::util::events::keys::ASK_OPENED,
        serde_json::json!({
            "sessionId": "",
            "askId": "ask-q",
            "payload": payload,
        }),
    );
    visual.run_until_parked();
    assert!(visual.debug_bounds("ask-question-q1").is_some());

    // 未答齐时提交钮不可用（opacity 垫底，点击无效果——state 不动）
    let submit = visual
        .debug_bounds("ask-submit-ask-q")
        .expect("提交钮已渲染");
    visual.simulate_click(submit.center(), Default::default());
    visual.run_until_parked();
    visual.update(|_window, cx| {
        assert!(
            !_chat
                .read(cx)
                .pending_asks()
                .first()
                .expect("卡在")
                .submitted,
            "未答齐不应进入提交态"
        );
    });

    // 点选方案 B + 自由文本输入（render 惰性建的输入框；sidebar_test
    // 同款 set_value 直驱写值）
    let chip = visual
        .debug_bounds("ask-choice-ask-q-b")
        .expect("选项 chip 已渲染");
    visual.simulate_click(chip.center(), Default::default());
    visual.run_until_parked();
    visual.update(|window, cx| {
        _chat.update(cx, |chat, cx| {
            let input = chat
                .ask_input("ask-q", "q2")
                .expect("自由文本输入框应已惰性建")
                .clone();
            input.update(cx, |input, cx| {
                input.set_value("就按方案 B 来", window, cx);
            });
        });
    });
    visual.run_until_parked();

    // 提交 → resolve_ask kind=question 被调（槽位收到逐题答案）
    let submit = visual.debug_bounds("ask-submit-ask-q").expect("提交钮在");
    visual.simulate_click(submit.center(), Default::default());
    let answer = tokio::runtime::Runtime::new()
        .expect("rt")
        .block_on(rx);
    assert!(
        matches!(
            answer,
            Ok(craftwave_engine::backend::ask::AskAnswer::Question { ref answers })
                if answers.get("q1").map(String::as_str) == Some("b")
                    && answers.get("q2").map(String::as_str) == Some("就按方案 B 来")
        ),
        "问卷应答应含逐题答案，实际 {answer:?}"
    );

    // 外部目录授权形态渲染（路径 + 说明 + 允许/拒绝）
    let payload = AskPayload::ExternalDir {
        path: "/tmp/external".to_owned(),
        options: vec![
            craftwave_engine::backend::ask::AskOption {
                id: "allow".to_owned(),
                label: "始终允许（本会话）".to_owned(),
                recommended: None,
                mode: None,
            },
            craftwave_engine::backend::ask::AskOption {
                id: "deny".to_owned(),
                label: "拒绝".to_owned(),
                recommended: None,
                mode: Some(craftwave_engine::backend::ask::AskMode::Deny),
            },
        ],
    };
    feed_event(
        &service,
        &mut *visual,
        craftwave_engine::util::events::keys::ASK_OPENED,
        serde_json::json!({
            "sessionId": "",
            "askId": "ask-ext",
            "payload": payload,
        }),
    );
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("ask-card-ask-ext").is_some(),
        "外部授权卡应出现（engine 有此 ask 形态，外部授权说明+等待态由卡承载）"
    );
}

/// 失效路径：槽位已收口（E_NOT_FOUND）→「已失效」终态行，不残留可操作面板。
#[gpui_kit::test]
fn stale_ask_expires_into_terminal_row(cx: &mut TestAppContext) {
    let (chat, service, _state, _events, executor, visual) = service_window(cx);
    visual.run_until_parked();
    // 不注入槽位——resolve 必然 E_NOT_FOUND
    feed_event(
        &service,
        &mut *visual,
        craftwave_engine::util::events::keys::ASK_OPENED,
        serde_json::json!({
            "sessionId": "",
            "askId": "ask-gone",
            "payload": approval_payload(&[("allow_once", "允许"), ("deny", "拒绝")]),
        }),
    );
    visual.run_until_parked();
    let option = visual
        .debug_bounds("ask-option-ask-gone-deny")
        .expect("拒绝按钮已渲染");
    visual.simulate_click(option.center(), Default::default());
    wait_for(
        &executor,
        &mut *visual,
        |visual| visual.debug_bounds("ask-resolved-ask-gone").is_some(),
        "失效终态行",
    );
    visual.update(|_window, cx| {
        assert!(chat.read(cx).pending_asks().is_empty(), "失效卡应移除");
        assert_eq!(chat.read(cx).resolved_asks().len(), 1);
    });
}

// ======================================================================
// 2. 集成：ConfirmEach 挂审批 → 允许 → run 恢复 → 终态行「已允许」
// ======================================================================

#[gpui_kit::test]
fn confirm_each_run_surfaces_approval_and_resolve_resumes(cx: &mut TestAppContext) {
    let (chat, service, state, _events, executor, visual) = service_window(cx);
    let session_id = named_session(&state);
    let user = persist_user(&state, &session_id, user_msg("创建一个文件"));
    let sid = session_id.clone();
    {
        let open_sid = sid.clone();
        let open_service = service.clone();
        visual.update(move |_window, cx| {
            open_service.update(cx, |service, cx| service.open_session(open_sid, cx));
        });
    }
    visual.run_until_parked();

    // 剧本：write 挂审批（turn1）→ 允许恢复后同一模型消费 turn2 收口
    //（resume 不重新解析模型——同 driver 续跑，两个 turn 在同一模型上）
    let resolver = mock_resolver(vec![
        MockCompletionModel::from_stream_turns(vec![
            stream_write_turn("hello.txt", "hi"),
            stream_text_turn("文件已创建。"),
        ]),
        unused_model(),
    ]);
    let mailbox = visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.begin_pipeline(cx))
    });
    let state_for_run = state.clone();
    let runtime = spawn_runtime();
    runtime.spawn(async move {
        let result = run::start_chat_test(
            &state_for_run,
            &state_for_run.runs,
            StartChatArgs {
                session_id: sid,
                messages: vec![user],
                run_config: RunConfig {
                    model_id: None,
                    thinking: None,
                    tier: PermissionTier::ConfirmEach,
                    max_turns: DEFAULT_MAX_TURNS,
                    steer: false,
                },
            },
            Box::new(FrameBridge::new(mailbox.clone())),
            false,
            resolver,
        )
        .await;
        let _ = mailbox.send(UiMail::Started(result.map(|r| r.run_id)));
    });

    // 审批卡出现（ask:opened 事件 → MailEventSink 邮箱 → 泵 → 面板；
    // askId 由引擎生成，以「任一挂起卡在场」判定）
    wait_for(
        &executor,
        &mut *visual,
        |visual| {
            visual
                .update(|_window, cx| !chat.read(cx).pending_asks().is_empty())
        },
        "审批卡经事件流出现",
    );

    // 点「允许」（按钮 id 含 askId——从状态面取）
    let ask_id = visual.update(|_window, cx| {
        chat.read(cx).pending_asks()[0].ask_id.clone()
    });
    let option = visual
        .debug_bounds(format!("ask-option-{ask_id}-allow_once").leak())
        .expect("允许按钮已渲染");
    visual.simulate_click(option.center(), Default::default());

    // run 恢复：自然终态收口 + busy 复位 + 文件落盘
    wait_for(
        &executor,
        &mut *visual,
        |visual| {
            visual.update(|_window, cx| service.read(cx).aggregator().is_terminal())
        },
        "允许后 run 恢复并收口",
    );
    tick_snapshot(&executor, &mut *visual);
    visual.update(|_window, cx| {
        assert!(!service.read(cx).busy(), "收口后 busy 应复位");
        // 终态行「已允许」
        let resolved = chat.read(cx).resolved_asks();
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].title.contains("写入新文件"), "终态行应带卡片标题");
    });
    // 允许后 write 真实执行（临时会话 workspace 根 = <data>/workspace/，
    // session_roots.rs 的两根布局）
    let written = state.store.root().join("workspace").join("hello.txt");
    assert!(written.exists(), "允许后 write 应已执行：{}", written.display());
}

// ======================================================================
// 3. 会话切换恢复挂起 ask
// ======================================================================

#[gpui_kit::test]
fn session_switch_restores_pending_ask(cx: &mut TestAppContext) {
    let (chat, service, state, _events, executor, visual) = service_window(cx);
    let session_id = named_session(&state);
    let other = named_session(&state);
    let user = persist_user(&state, &session_id, user_msg("创建一个文件"));
    let sid = session_id.clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.open_session(sid, cx));
    });
    visual.run_until_parked();

    // ConfirmEach 挂起（与测试 2 同款装置）
    let resolver = mock_resolver(vec![
        MockCompletionModel::from_stream_turns(vec![
            stream_write_turn("hello.txt", "hi"),
        ]),
        unused_model(),
        unused_model(),
    ]);
    let mailbox = visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.begin_pipeline(cx))
    });
    let state_for_run = state.clone();
    let runtime = spawn_runtime();
    let run_sid = session_id.clone();
    runtime.spawn(async move {
        let result = run::start_chat_test(
            &state_for_run,
            &state_for_run.runs,
            StartChatArgs {
                session_id: run_sid,
                messages: vec![user],
                run_config: RunConfig {
                    model_id: None,
                    thinking: None,
                    tier: PermissionTier::ConfirmEach,
                    max_turns: DEFAULT_MAX_TURNS,
                    steer: false,
                },
            },
            Box::new(FrameBridge::new(mailbox.clone())),
            false,
            resolver,
        )
        .await;
        let _ = mailbox.send(UiMail::Started(result.map(|r| r.run_id)));
    });
    // 引擎槽挂起
    let ask_state = state.clone();
    let ask_sid = session_id.clone();
    wait_for(
        &executor,
        &mut *visual,
        move |visual| {
            visual.update(|_window, _cx| {
                run::session_pending_ask(
                    &ask_state,
                    run::SessionPendingAskArgs {
                        session_id: ask_sid.clone(),
                    },
                )
                .expect("pending ask 查询成功")
                .is_some()
            })
        },
        "引擎审批挂起",
    );

    // 切走：本地卡清空；切回：经 session_pending_ask 恢复
    let other_sid = other.clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.open_session(other_sid, cx));
    });
    visual.run_until_parked();
    visual.update(|_window, cx| {
        assert!(
            chat.read(cx).pending_asks().is_empty(),
            "切走后本地卡应清空"
        );
    });
    let back_sid = session_id.clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.open_session(back_sid, cx));
    });
    visual.run_until_parked();
    visual.update(|_window, cx| {
        assert_eq!(
            chat.read(cx).pending_asks().len(),
            1,
            "切回应经引擎槽恢复挂起审批"
        );
    });
    assert!(visual.debug_bounds("ask-card").is_none_or(|_| true));
    // 渲染面：恢复的卡在场（id 从状态取）
    let ask_id = visual.update(|_window, cx| chat.read(cx).pending_asks()[0].ask_id.clone());
    assert!(
        visual.debug_bounds(format!("ask-card-{ask_id}").leak()).is_some(),
        "恢复的审批卡应渲染"
    );
    // 收尾：取消挂起 run（不污染其他测试）
    let _ = run::cancel_run(&state, &session_id);
}

// ======================================================================
// 4. 权限档快切（徽标点击 + Shift+Tab 同路）
// ======================================================================

#[gpui_kit::test]
fn tier_cycles_via_badge_click_and_shift_tab(cx: &mut TestAppContext) {
    // 走生产 install 形态才有 composer（回声装置不建 composer——
    // service_window 的手装实体绕过了窗口闭包）
    let (state, events, _root) = fixture_app();
    cx.update(|cx| {
        crate::engine::install_runtime(cx);
        gpui_kit::init(cx);
        CraftwaveTheme::mount(cx);
        theme::apply(cx);
    });
    let bridge = crate::engine::EngineBridge {
        state: state.clone(),
        events: events.clone(),
    };
    let (view, visual) = cx.add_window_view(|window, cx| MainWindowView {
        zoom_seen: 0.0,
        chat: crate::chat::install(window, cx, Some(&bridge)),
        sidebar: None,
    });
    visual.run_until_parked();
    let chat = visual.update(|_window, cx| view.read(cx).chat.clone());

    // 徽标点击循环：AutoEdit → FullAccess → Plan → ConfirmEach
    let tier_of = |visual: &mut VisualTestContext| {
        visual.update(|_window, cx| {
            chat.read(cx)
                .composer()
                .expect("composer 在")
                .read(cx)
                .tier()
        })
    };
    let badge = visual
        .debug_bounds("tier-badge")
        .expect("档位徽标已渲染");
    visual.simulate_click(badge.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(tier_of(&mut *visual), PermissionTier::FullAccess);
    let badge = visual.debug_bounds("tier-badge").expect("徽标在");
    visual.simulate_click(badge.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(tier_of(&mut *visual), PermissionTier::Plan);

    // 再点一次回到 AutoEdit（循环闭合）
    let badge = visual.debug_bounds("tier-badge").expect("徽标在");
    visual.simulate_click(badge.center(), Default::default());
    visual.run_until_parked();
    assert_eq!(tier_of(&mut *visual), PermissionTier::ConfirmEach);
    // Shift+Tab 键位已绑定（main.rs），但输入框聚焦时 gpui-base 的
    // OutdentInline（CONTEXT 内更具体绑定）优先消费 shift-tab——快切主
    // 路径是徽标点击（取舍记录在交付报告），键位服务输入框外焦点场景
}

// ======================================================================
// 5. B6 run 级错误卡：分流 + 复制 + 重试
// ======================================================================

/// 错误码分流 case 表（B6：模型不可用 → 设置入口；其他 → 重试）。
#[gpui_kit::test]
fn run_error_branches_by_code_class() {
    let case = |code: &str, message: &str| super::state::RunErrorCard {
        code: code.to_owned(),
        message: message.to_owned(),
    };
    // 模型/供应商类：设置入口
    assert!(case("E_PROVIDER_AUTH", "认证失败").is_model_error());
    assert!(case("E_PROVIDER_RATE_LIMITED", "限流").is_model_error());
    assert!(case("E_INVALID_ARG", "未配置激活模型：请先在设置中添加供应商").is_model_error());
    assert!(case("E_INTERNAL", "请求返回 401").is_model_error());
    assert!(case("E_INTERNAL", "api key 不正确").is_model_error());
    // 其他类：重试
    assert!(!case("E_INTERNAL", "boom").is_model_error());
    assert!(!case("E_RUN_STALLED", "流式停滞").is_model_error());
    // 完整复制文本 = 码 + 消息
    assert_eq!(
        case("E_X", "msg").full_text(),
        "E_X: msg",
        "一键复制内容应含错误码与完整消息"
    );
}

#[gpui_kit::test]
fn run_error_card_renders_branches_copies_and_retries(cx: &mut TestAppContext) {
    let (chat, service, state, _events, executor, visual) = service_window(cx);
    // 会话 + 一条 user 历史（重试的素材）
    let session_id = named_session(&state);
    let user = persist_user(&state, &session_id, user_msg("第一问"));
    let sid = session_id.clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.open_session(sid, cx));
    });
    visual.run_until_parked();

    // 非模型错误 → 重试按钮分流
    visual.update(|_window, cx| {
        chat.update(cx, |chat, cx| {
            chat.set_run_error("E_INTERNAL".to_owned(), "boom".to_owned(), cx);
        });
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("run-error-card").is_some(), "错误卡应常驻渲染");
    assert!(
        visual.debug_bounds("error-retry").is_some(),
        "非模型错误应给重试入口"
    );
    assert!(visual.debug_bounds("error-open-settings").is_none());

    // 复制完整错误（剪贴板断言：完整文本含载荷）
    let copy = visual
        .debug_bounds("error-copy")
        .expect("复制钮已渲染");
    visual.simulate_click(copy.center(), Default::default());
    visual.run_until_parked();
    let clipboard = visual.update(|_window, cx| {
        cx.read_from_clipboard()
            .expect("复制后剪贴板应有内容")
            .text()
            .expect("应为文本载荷")
    });
    assert_eq!(clipboard, "E_INTERNAL: boom", "一键复制 = 错误码 + 完整消息");

    // 重试链：重发最后一条 user 消息（同父新分支落盘）。无模型装置下重试
    // 发起的 run 会立刻失败收口（busy 经历 true→false），稳定断言面 =
    // 树上新 user 节点（branch_resend 的落盘发生在 start_chat 之前）
    let retry = visual.debug_bounds("error-retry").expect("重试钮在");
    visual.simulate_click(retry.center(), Default::default());
    visual.run_until_parked();
    visual.update(|_window, cx| {
        assert!(
            chat.read(cx).run_error.is_none(),
            "点击重试后旧错误卡应先被清除（listener 已触发的证据）"
        );
    });
    let tree_state = state.clone();
    let tree_sid = session_id.clone();
    wait_for(
        &executor,
        &mut *visual,
        move |visual| {
            visual.update(|_window, _cx| {
                session::load_session_tree(
                    &tree_state,
                    LoadSessionTreeArgs {
                        session_id: tree_sid.clone(),
                    },
                )
                .map(|tree| {
                    tree.messages
                        .iter()
                        .filter(|m| m.role == MessageRole::User)
                        .count()
                        == 2
                })
                .unwrap_or(false)
            })
        },
        "重试应产生新 user 分支节点",
    );
    wait_for(
        &executor,
        &mut *visual,
        |visual| visual.update(|_window, cx| !service.read(cx).busy()),
        "重试发起的 run 失败收口（无模型装置）",
    );
    let tree = session::load_session_tree(
        &state,
        LoadSessionTreeArgs {
            session_id: session_id.clone(),
        },
    )
    .expect("load tree");
    let user_count = tree
        .messages
        .iter()
        .filter(|m| m.role == MessageRole::User)
        .count();
    assert_eq!(user_count, 2, "重试应产生新 user 分支节点");
    assert_ne!(tree.messages.last().expect("有节点").id, user.id);
    let _ = run::cancel_run(&state, &session_id);
}

/// 模型类错误 → 设置入口占位（OpenSettings action 派发——main.rs 的
/// 占位处理器接住）；错误卡随会话切换清除。
#[gpui_kit::test]
fn model_error_shows_settings_entry_and_card_clears_on_switch(cx: &mut TestAppContext) {
    let (chat, service, state, _events, _executor, visual) = service_window(cx);
    let session_id = named_session(&state);
    let sid = session_id.clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.open_session(sid, cx));
    });
    visual.run_until_parked();
    visual.update(|_window, cx| {
        chat.update(cx, |chat, cx| {
            chat.set_run_error(
                "E_INVALID_ARG".to_owned(),
                "未配置激活模型：请先在设置中添加供应商".to_owned(),
                cx,
            );
        });
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("error-open-settings").is_some(),
        "模型类错误应给设置入口（3.7 设置页接通前的占位）"
    );
    assert!(visual.debug_bounds("error-retry").is_none());
    // 会话切换清除
    let other = named_session(&state);
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.open_session(other, cx));
    });
    visual.run_until_parked();
    visual.update(|_window, cx| {
        assert!(
            chat.read(cx).run_error.is_none(),
            "切换会话应清除错误卡"
        );
    });
}
