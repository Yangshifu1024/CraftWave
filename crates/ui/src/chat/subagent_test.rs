//! 子代理（gpui-rewrite 3-B：3.6 验收，`#[gpui_kit::test]` 独立文件）。
//!
//! 三组：
//! 1. **帧面呈现**：SubStarted/SubFrame/SubReport/SubDone 注入聚合器 →
//!    流式行工具簇的子代理摘要行（状态徽标 / turn / 跳转 / 运行中停止）；
//! 2. **事件面 + 跳转往返**：sub:* 事件累积（含 error 文本）→ 打开详情
//!    视图（真历史装载：tempdir 构造 engine 目录契约）→ 列表行切换 →
//!    返回 → 再开（往返）；
//! 3. **stop 链路**：注册真 SubHandle → 摘要行停止钮 → `stop_subagent`
//!    被调 → 取消令牌置位；已移除子代理幂等不 panic。

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{AppContext as _, Context, Entity, TestAppContext, VisualTestContext};

use craftwave_engine::app_state::AppState;
use craftwave_engine::api::session::{self, CreateSessionArgs, RenameSessionArgs};
use craftwave_engine::backend::frames::{ChatFrame, SequencedFrame};
use craftwave_engine::snapshot::{FrameAggregator, StreamPart};
use craftwave_engine::util::events::keys;

use crate::MainWindowView;
use crate::engine::MailEventSink;
use crate::theme::{self, CraftwaveTheme};

use super::service::SessionService;
use super::state::ConversationState;

// ======================================================================
// 装置
// ======================================================================

fn fixture_app() -> (Arc<AppState>, Arc<MailEventSink>, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    let events = Arc::new(MailEventSink::new());
    let state = AppState::assemble(root.clone(), events.clone(), None);
    (state, events, root)
}

fn spawn_runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"))
}

/// 回声渲染窗口（无 service——纯帧面/状态驱动的渲染断言）。
fn chat_window(cx: &mut TestAppContext) -> (Entity<ConversationState>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        CraftwaveTheme::mount(cx);
        theme::apply(cx);
    });
    let (view, visual) = cx.add_window_view(|_window, cx| MainWindowView {
        zoom_seen: 0.0,
        chat: cx.new(|_| ConversationState::new()),
        sidebar: None,
        palette: None,
    });
    let chat = visual.update(|_window, cx| view.read(cx).chat.clone());
    (chat, visual)
}

/// 带 service 的窗口六元组（装置返回体；type_complexity 收敛）。
type ServiceWindow<'a> = (
    Entity<ConversationState>,
    Entity<SessionService>,
    Arc<AppState>,
    Arc<MailEventSink>,
    std::path::PathBuf,
    &'a mut VisualTestContext,
);

/// 带 service 的窗口（事件面 / stop / 历史装载链路用）。
fn service_window(cx: &mut TestAppContext) -> ServiceWindow<'_> {
    let (state, events, root) = fixture_app();
    let runtime = spawn_runtime();
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
            palette: None,
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
    (chat, service, state, events, root, visual)
}

fn mutate(
    chat: &Entity<ConversationState>,
    visual: &mut VisualTestContext,
    f: impl FnOnce(&mut ConversationState, &mut Context<ConversationState>),
) {
    visual.update(|_window, cx| {
        chat.update(cx, |chat, cx| f(chat, cx));
    });
}

/// 构造一个带 subagent 工具调用的流式快照（子代理四帧注入聚合器后产出）。
fn subagent_streaming(
    aggregator: &FrameAggregator,
    outcome_done: bool,
) -> craftwave_engine::snapshot::StreamingMessage {
    let mut seq = 0u64;
    let mut push = |frame: ChatFrame| {
        aggregator.push(SequencedFrame {
            seq,
            ts: None,
            frame,
        });
        seq += 1;
    };
    push(ChatFrame::RunStarted {
        run_id: "run-1".to_owned(),
        model: "mock".to_owned(),
    });
    push(ChatFrame::TextDelta {
        turn: 1,
        delta: "派个研究员去查。".to_owned(),
    });
    push(ChatFrame::ToolStarted {
        call_id: "call-1".to_owned(),
        name: "subagent".to_owned(),
    });
    push(ChatFrame::SubStarted {
        call_id: "call-1".to_owned(),
        sub_id: "sub-1".to_owned(),
        role: "researcher".to_owned(),
    });
    push(ChatFrame::SubFrame {
        sub_id: "sub-1".to_owned(),
        frame: Box::new(ChatFrame::StepStarted { turn: 2 }),
    });
    if outcome_done {
        push(ChatFrame::SubReport {
            sub_id: "sub-1".to_owned(),
            report: "调研结论：一切正常。".to_owned(),
        });
        push(ChatFrame::SubDone {
            sub_id: "sub-1".to_owned(),
            outcome: craftwave_engine::backend::frames::SubOutcome::Report,
        });
        push(ChatFrame::ToolResult {
            call_id: "call-1".to_owned(),
            ok: true,
            ui_summary: "子代理 researcher 完成 · 汇报已返回".to_owned(),
            error: None,
        });
    }
    push(ChatFrame::RunEnded {
        reason: craftwave_engine::backend::frames::EndReason::Done,
        duration_ms: 10,
        tool_count: 1,
        turn: 1,
        error: None,
    });
    aggregator.snapshot_streaming().expect("有流式态")
}

// ======================================================================
// 1. 帧面呈现：摘要行 + 跳转 + 运行中停止
// ======================================================================

#[gpui_kit::test]
fn subagent_frames_surface_in_tool_cluster(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);

    // 运行中：SubStarted + SubFrame（无 SubDone）——折叠态先验头跳转钮
    //（折叠头的钮只在折叠态显示，Wake open_child_button 语义）
    let aggregator = Arc::new(FrameAggregator::new());
    let streaming = subagent_streaming(&aggregator, false);
    mutate(&chat, &mut *visual, |chat, cx| {
        chat.apply_snapshot(
            Rc::new(Vec::new()),
            Some(streaming.clone()),
            cx,
        );
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("tool-open-subagent-0").is_some(),
        "折叠头跳转钮应显示（恰好一派 + 折叠态）"
    );
    // 展开工具簇（点折叠头）→ 摘要行：状态徽标 + turn 计数 + 跳转 + 停止
    let head = visual
        .debug_bounds("tools-head-0")
        .expect("工具簇折叠头已渲染");
    visual.simulate_click(head.center(), Default::default());
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("subagent-line-0-0").is_some(),
        "委派调用应带子代理摘要行"
    );
    assert!(
        visual.debug_bounds("subagent-line-stop-0-0").is_some(),
        "运行中子代理应有停止钮（回声装置无 service，点击为无动作防御）"
    );

    // 终态：SubDone 后停止钮消失、状态徽标翻牌
    let aggregator = Arc::new(FrameAggregator::new());
    let streaming = subagent_streaming(&aggregator, true);
    mutate(&chat, &mut *visual, |chat, cx| {
        chat.apply_snapshot(Rc::new(Vec::new()), Some(streaming), cx);
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("subagent-line-stop-0-0").is_none(),
        "终态子代理不应再有停止钮"
    );
}

// ======================================================================
// 2. 事件面 + 详情视图跳转往返 + 真历史装载
// ======================================================================

/// 构造 engine 子代理历史目录契约：
/// `<data>/histories/subs/<parentRunId>/<subId>/{meta.json,history.jsonl}`。
fn write_sub_history(root: &std::path::Path, session_id: &str, sub_id: &str) {
    let dir = root
        .join("histories")
        .join("subs")
        .join("parent-run-1")
        .join(sub_id);
    std::fs::create_dir_all(&dir).expect("建历史目录");
    let meta = serde_json::json!({
        "version": 1,
        "sessionId": session_id,
        "subId": sub_id,
        "parentRunId": "parent-run-1",
        "role": "researcher",
        "task": "调研",
        "budgetSteps": 8,
        "startedAt": 1_700_000_000_000i64,
        "outcome": "report",
    });
    std::fs::write(dir.join("meta.json"), serde_json::to_vec_pretty(&meta).unwrap())
        .expect("写 meta");
    let history = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"type": "message", "turn": 1, "text": "查完了。", "reasoning": "想了想"}),
        serde_json::json!({"type": "tool", "turn": 1, "callId": "c1", "name": "grep", "argsJson": "{\"pattern\":\"x\"}", "ok": true, "summary": "命中 1 处"}),
        serde_json::json!({"type": "end", "outcome": "report", "report": "调研结论：一切正常。"}),
    );
    std::fs::write(dir.join("history.jsonl"), history).expect("写历史");
}

#[gpui_kit::test]
fn subagent_events_accumulate_and_jump_roundtrip_loads_history(cx: &mut TestAppContext) {
    let (chat, service, state, _events, root, visual) = service_window(cx);
    // 历史装载的 sessionId 参数源 = 打开的会话（生产语义：子代理属于打开
    // 的会话）；tempdir 会话与历史目录按同一 id 构造
    let session_id = {
        let meta = session::create_session(&state, CreateSessionArgs::default())
            .expect("create_session");
        session::rename_session(
            &state,
            RenameSessionArgs {
                session_id: meta.id.clone(),
                title: "子代理测试会话".to_owned(),
            },
        )
        .expect("rename_session");
        meta.id
    };
    {
        let open_service = service.clone();
        let open_sid = session_id.clone();
        visual.update(move |_window, cx| {
            open_service.update(cx, |service, cx| service.open_session(open_sid, cx));
        });
    }
    visual.run_until_parked();
    let sid_json = serde_json::json!(session_id);

    // 事件面累积：spawn → step → report → done（sessionId = 打开的会话）
    let feed = |service: &Entity<SessionService>, visual: &mut VisualTestContext, key: &str, payload: serde_json::Value| {
        visual.update(|_window, cx| {
            service.update(cx, |service, cx| service.handle_event(key, &payload, cx));
        });
    };
    feed(
        &service,
        &mut *visual,
        keys::SUB_SPAWN,
        serde_json::json!({ "sessionId": sid_json, "runId": "parent-run-1", "subId": "sub-1", "role": "researcher" }),
    );
    feed(
        &service,
        &mut *visual,
        keys::SUB_STEP,
        serde_json::json!({ "sessionId": sid_json, "runId": "parent-run-1", "subId": "sub-1", "turn": 3 }),
    );
    feed(
        &service,
        &mut *visual,
        keys::SUB_REPORT,
        serde_json::json!({ "sessionId": sid_json, "runId": "parent-run-1", "subId": "sub-1", "report": "调研结论：一切正常。" }),
    );
    feed(
        &service,
        &mut *visual,
        keys::SUB_DONE,
        serde_json::json!({ "sessionId": sid_json, "runId": "parent-run-1", "subId": "sub-1", "outcome": "report" }),
    );
    // error 事件独有文本
    feed(
        &service,
        &mut *visual,
        keys::SUB_ERROR,
        serde_json::json!({ "sessionId": sid_json, "runId": "parent-run-1", "subId": "sub-2", "code": "E_INTERNAL", "message": "驱动失败" }),
    );
    visual.run_until_parked();
    visual.update(|_window, cx| {
        let subs = chat.read(cx).known_subagents();
        assert_eq!(subs.len(), 2, "两个子代理（sub-1 正常收口 / sub-2 异常）");
        let sub1 = subs.iter().find(|s| s.sub_id == "sub-1").expect("sub-1 在");
        assert_eq!(sub1.turn, 3);
        assert!(sub1.report.contains("一切正常"));
        assert!(matches!(
            sub1.state,
            craftwave_engine::snapshot::SubRunState::Done(
                craftwave_engine::backend::frames::SubOutcome::Report
            )
        ));
        let sub2 = subs.iter().find(|s| s.sub_id == "sub-2").expect("sub-2 在");
        assert_eq!(sub2.error.as_deref(), Some("E_INTERNAL: 驱动失败"));
    });

    // 跳转往返 + 真历史装载（engine 目录契约）
    write_sub_history(&root, &session_id, "sub-1");
    let open = |chat: &Entity<ConversationState>, visual: &mut VisualTestContext, sub: &str| {
        mutate(chat, visual, |chat, cx| chat.open_sub_view(sub.to_owned(), cx));
        visual.run_until_parked();
    };
    open(&chat, &mut *visual, "sub-1");
    // 装载是后台任务（cx.spawn → background_spawn）：轮询到 Loaded
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        visual.run_until_parked();
        let loaded = visual.update(|_window, cx| {
            chat.read(cx)
                .sub_view
                .as_ref()
                .map(|view| {
                    matches!(
                        view.history,
                        super::subagent::SubHistoryState::Loaded(_)
                    )
                })
                .unwrap_or(false)
        });
        if loaded {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "子代理历史装载超时"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        visual.debug_bounds("subagent-view").is_some(),
        "详情视图覆盖层应渲染"
    );
    assert!(
        visual.debug_bounds("subagent-row-sub-1").is_some(),
        "列表行应渲染（本会话已知子代理）"
    );
    assert!(
        visual.debug_bounds("sub-entry-sub-1:a1").is_some(),
        "turn 条目应经 load_subagent_history 装载渲染"
    );
    assert!(
        visual.debug_bounds("sub-entry-sub-1:report").is_some(),
        "汇报条目应渲染"
    );

    // 返回 → 再开（往返）
    let back = visual
        .debug_bounds("sub-view-back")
        .expect("返回钮已渲染");
    visual.simulate_click(back.center(), Default::default());
    visual.run_until_parked();
    visual.update(|_window, cx| {
        assert!(chat.read(cx).sub_view.is_none(), "返回应关闭视图");
    });
    assert!(visual.debug_bounds("subagent-view").is_none());
    open(&chat, &mut *visual, "sub-1");
    assert!(visual.debug_bounds("subagent-view").is_some(), "再次跳转应重开");

    // 列表行切换详情目标（sub-1 → sub-2）
    let row2 = visual
        .debug_bounds("subagent-row-sub-2")
        .expect("sub-2 列表行已渲染");
    visual.simulate_click(row2.center(), Default::default());
    visual.run_until_parked();
    visual.update(|_window, cx| {
        assert_eq!(
            chat.read(cx).sub_view.as_ref().expect("视图在").sub_id,
            "sub-2",
            "列表行点击应切换详情目标"
        );
    });
}

// ======================================================================
// 3. stop 链路
// ======================================================================

#[gpui_kit::test]
fn stop_subagent_cancels_registered_handle(cx: &mut TestAppContext) {
    let (chat, service, state, _events, _root, visual) = service_window(cx);
    visual.run_until_parked();

    // 注册一个真子代理句柄（engine SubRegistry 契约字段）
    let handle = Arc::new(craftwave_engine::agent::sub_registry::SubHandle {
        sub_id: "sub-stop".to_owned(),
        role: "researcher".to_owned(),
        session_id: "s".to_owned(),
        parent_run_id: "parent-run-1".to_owned(),
        cancel: tokio_util::sync::CancellationToken::new(),
        ask: Arc::new(std::sync::Mutex::new(None)),
        started_at: 1_700_000_000_000i64,
    });
    state.subs.register(handle.clone());
    assert!(!handle.cancel.is_cancelled());

    // 事件面让它进入已知集合（摘要行/详情的数据源）+ 打开详情视图
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| {
            service.handle_event(
                keys::SUB_SPAWN,
                &serde_json::json!({ "sessionId": "", "runId": "p", "subId": "sub-stop", "role": "researcher" }),
                cx,
            );
            service.handle_event(
                keys::SUB_STEP,
                &serde_json::json!({ "sessionId": "", "runId": "p", "subId": "sub-stop", "turn": 1 }),
                cx,
            );
        });
    });
    visual.run_until_parked();
    mutate(&chat, &mut *visual, |chat, cx| {
        chat.open_sub_view("sub-stop".to_owned(), cx);
    });
    visual.run_until_parked();

    // 详情头停止钮 → stop_subagent 被调 → 令牌置位
    let stop = visual
        .debug_bounds("sub-stop-sub-stop")
        .expect("运行中详情头应有停止钮");
    visual.simulate_click(stop.center(), Default::default());
    visual.run_until_parked();
    assert!(
        handle.cancel.is_cancelled(),
        "停止钮应触发 engine stop_subagent（取消令牌置位）"
    );

    // 已移除/已结束的子代理：stop 幂等不 panic
    state.subs.remove("sub-stop");
    mutate(&chat, &mut *visual, |chat, cx| {
        super::subagent::stop_subagent(chat, "sub-stop", cx);
    });
    visual.run_until_parked();
}

/// B6 错误卡常驻面：流式行终态 Error 后落盘重载（流式行消失），
/// 错误卡独立存活（run_error 面不随 set_history 清）。
#[gpui_kit::test]
fn run_error_card_survives_history_reload(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    let aggregator = Arc::new(FrameAggregator::new());
    let mut seq = 0u64;
    let mut push = |frame: ChatFrame| {
        aggregator.push(SequencedFrame {
            seq,
            ts: None,
            frame,
        });
        seq += 1;
    };
    push(ChatFrame::RunStarted {
        run_id: "run-1".to_owned(),
        model: "mock".to_owned(),
    });
    push(ChatFrame::RunEnded {
        reason: craftwave_engine::backend::frames::EndReason::Error,
        duration_ms: 5,
        tool_count: 0,
        turn: 1,
        error: Some(craftwave_engine::backend::frames::FrameError::new(
            "E_INTERNAL",
            "驱动中断",
        )),
    });
    let streaming = aggregator.snapshot_streaming().expect("有流式态");
    assert!(streaming.ended.is_some());
    let _ = StreamPart::Text {
        text: String::new(),
    };
    mutate(&chat, &mut *visual, |chat, cx| {
        // 泵的终态观察路径同款：错误终态落常驻卡
        chat.apply_snapshot(Rc::new(Vec::new()), Some(streaming), cx);
        chat.set_run_error("E_INTERNAL".to_owned(), "驱动中断".to_owned(), cx);
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("run-error-card").is_some());
    // 落盘重载（历史接管、流式行清空）后错误卡仍在
    mutate(&chat, &mut *visual, |chat, cx| {
        chat.apply_snapshot(Rc::new(Vec::new()), None, cx);
    });
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("run-error-card").is_some(),
        "错误卡应独立于流式行存活（长报错始终可读可复制）"
    );
}
