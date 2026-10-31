//! ⌘K 命令面板测试（gpui-rewrite 3.2 验收，`#[gpui_kit::test]` 独立文件）。
//!
//! 六组：
//! 1. **开合**：toggle 开（面板入树 + 查询框聚焦）、再按关、Escape 关；
//! 2. **分组条目**：命令组三条恒在 + 空查询最近会话组（未发送会话过滤）；
//! 3. **键盘导航全模型**：上下键（循环）、Home/End 首末、Enter 确认；
//! 4. **命令执行**：确认「新会话」条目 dispatch 既有 action（一个逻辑
//!    命令只建模一次）+ 面板收场；
//! 5. **会话结果定位**：查询命中标题 → 确认 → 服务 open_session 装载历史；
//! 6. **防抖对账**：连发两查询，过期结果丢弃（代次守卫 + 任务取消双保险）。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::actions::{Cancel, SelectDown, SelectFirst, SelectLast, SelectUp};
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};

use craftwave_engine::api::session::{
    self, CreateSessionArgs, RenameSessionArgs, SaveSessionTreeArgs,
};
use craftwave_engine::app_state::AppState;
use craftwave_engine::storage::{MessageEntry, MessageRole, WirePart};

use crate::chat::{ConversationState, EngineService, SessionService};
use crate::engine::{MailEventSink, install_runtime};
use crate::theme::{self, CraftwaveTheme};
use crate::NewSession;

use super::PaletteState;

// ======================================================================
// 装置
// ======================================================================

/// 临时数据根装配（sidebar_test 同款）。
fn fixture_app() -> (Arc<AppState>, Arc<MailEventSink>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    let events = Arc::new(MailEventSink::new());
    let state = AppState::assemble(root, events.clone(), None);
    (state, events)
}

/// user 单消息（树同步最小形态）。
fn user_msg(text: &str) -> MessageEntry {
    MessageEntry {
        id: format!("m-{}", text),
        parent_id: None,
        role: MessageRole::User,
        parts: vec![WirePart::Text {
            text: text.to_owned(),
        }],
        created_at: 1_700_000_000_000,
        run_id: None,
        status: None,
    }
}

/// 建一个带一条消息的会话并改名（messageCount>0 → 可见可命中）。
fn seeded_session(state: &AppState, title: &str) -> String {
    let meta = session::create_session(state, CreateSessionArgs { project_id: None })
        .expect("create");
    session::rename_session(
        state,
        RenameSessionArgs {
            session_id: meta.id.clone(),
            title: title.to_owned(),
        },
    )
    .expect("rename");
    let entry = user_msg("首条消息（入列）");
    let entry_id = entry.id.clone();
    session::save_session_tree(
        state,
        SaveSessionTreeArgs {
            session_id: meta.id.clone(),
            messages: vec![entry],
            active_leaf_id: Some(entry_id),
            base_revision: 0,
        },
    )
    .expect("save");
    meta.id
}

/// 测试窗口：tempdir 装配 + conversation + service + 命令面板（挂主窗根
/// 视图——渲染冒烟顺带覆盖）。
type Slots = (
    Option<Entity<SessionService>>,
    Option<Entity<PaletteState>>,
    Option<Entity<ConversationState>>,
);

#[allow(clippy::type_complexity)]
fn palette_window(
    cx: &mut TestAppContext,
) -> (
    Entity<ConversationState>,
    Entity<SessionService>,
    Entity<PaletteState>,
    Arc<AppState>,
    &mut VisualTestContext,
) {
    let (state, events) = fixture_app();
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    cx.update(|cx| {
        install_runtime(cx);
        gpui_kit::init(cx);
        CraftwaveTheme::mount(cx);
        theme::apply(cx);
    });
    let state_for_view = state.clone();
    let events_for_view = events.clone();
    let slots: Rc<RefCell<Slots>> = Rc::new(RefCell::new((None, None, None)));
    let slots_for_view = slots.clone();
    let (view, visual) = cx.add_window_view(move |window, cx| {
        let conversation = cx.new(|_| ConversationState::new());
        slots_for_view.borrow_mut().2 = Some(conversation.clone());
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
        // 生产里 chat::install 挂 Global；测试手动挂（palette::install 取它）
        cx.set_global(EngineService(service.clone()));
        slots_for_view.borrow_mut().0 = Some(service.clone());
        let bridge = crate::engine::EngineBridge {
            state: state_for_view.clone(),
            events: events_for_view.clone(),
        };
        let palette = super::install(window, cx, &bridge);
        slots_for_view.borrow_mut().1 = Some(palette.clone());
        // Root 包装与生产 open_window 同款（浮层挂载点；conversation_test
        // 的 find 测试同款装置——无它浮层子树的绘制帧不落 debug_bounds）
        let inner = cx.new(|_|
            crate::MainWindowView {
                zoom_seen: 0.0,
                chat: conversation,
                sidebar: None,
                palette: Some(palette),
            });
        gpui_kit::base::Root::new(inner, window, cx)
    });
    let _view = view;
    visual.run_until_parked();
    let (service, palette, chat) = slots.borrow().clone();
    (
        chat.expect("窗口闭包内已装配"),
        service.expect("窗口闭包内已装配"),
        palette.expect("窗口闭包内已装配"),
        state,
        visual,
    )
}

/// 打开面板并等清单装载落地。
fn open_palette(palette: &Entity<PaletteState>, visual: &mut VisualTestContext) {
    visual
        .update(|window, cx| palette.update(cx, |palette, cx| palette.toggle(window, cx)));
    visual.run_until_parked();
}

// ======================================================================
// 1. 开合
// ======================================================================

/// toggle 开：浮层入树、面板开标记置位；再 toggle 关：浮层离场。
#[gpui_kit::test]
fn toggle_opens_and_closes(cx: &mut TestAppContext) {
    let (_chat, _service, palette, _state, visual) = palette_window(cx);
    assert!(visual.debug_bounds("command-palette").is_none(), "初始关着");
    open_palette(&palette, visual);
    assert!(visual.debug_bounds("command-palette").is_some(), "⌘K 后入树");
    assert!(
        visual.debug_bounds("command-palette-card").is_some(),
        "面板卡片入树"
    );
    visual.update(|window, cx| palette.update(cx, |palette, cx| palette.toggle(window, cx)));
    visual.run_until_parked();
    assert!(visual.debug_bounds("command-palette").is_none(), "再按 ⌘K 关");
}

/// Escape 关（空查询的 Cancel 冒泡到宿主容器收场——组件文档的分工）。
#[gpui_kit::test]
fn escape_closes_palette(cx: &mut TestAppContext) {
    let (_chat, _service, palette, _state, visual) = palette_window(cx);
    open_palette(&palette, visual);
    visual.update(|window, cx| {
        window.dispatch_action(Box::new(Cancel), cx);
    });
    visual.run_until_parked();
    visual.update(|_window, cx| assert!(!palette.read(cx).open, "Escape 后应关闭"));
}

// ======================================================================
// 2. 分组条目
// ======================================================================

/// 命令组三条恒在；空查询给最近会话（上限内）；未发送会话（count=0）
/// 不进最近面。
#[gpui_kit::test]
fn groups_show_commands_and_recent_sessions(cx: &mut TestAppContext) {
    let (_chat, _service, palette, state, visual) = palette_window(cx);
    let _a = seeded_session(&state, "needle session");
    let _b = seeded_session(&state, "another visible session");
    // 未发送会话（messageCount = 0）：不进最近面（侧栏 listed 同纪律）
    let _empty = session::create_session(&state, CreateSessionArgs { project_id: None })
        .expect("create");
    open_palette(&palette, visual);
    visual.update(|_window, cx| {
        let command = palette.read(cx).command.clone();
        let command = command.read(cx);
        // 命令 3 + 可见会话 2（空会话不进）
        assert_eq!(command.matched_count(), 3 + 2);
        assert_eq!(
            palette.read(cx).visible_sessions.len(),
            2,
            "最近面只列已发送会话"
        );
    });
}

// ======================================================================
// 3. 键盘导航全模型
// ======================================================================

/// 上下键移动选中（组件内建），Home/End 落首末（宿主容器接），全部走
/// 真实按键派发。命令组 section 0，会话组 section 1。
#[gpui_kit::test]
fn keyboard_navigation_full_model(cx: &mut TestAppContext) {
    let (_chat, _service, palette, state, visual) = palette_window(cx);
    seeded_session(&state, "target session");
    open_palette(&palette, visual);
    let selected = |visual: &mut VisualTestContext| {
        visual.update(|_window, cx| {
            palette
                .read(cx)
                .command
                .read(cx)
                .selected_index()
                .map(|index| (index.section, index.row))
        })
    };
    // 初始选中 = 命令组第一条
    assert_eq!(selected(visual), Some((0, 0)));
    // down ×2 → 命令组第三条；再 down → 跨进会话组第一条
    visual.update(|window, cx| window.dispatch_action(Box::new(SelectDown), cx));
    visual.update(|window, cx| window.dispatch_action(Box::new(SelectDown), cx));
    assert_eq!(selected(visual), Some((0, 2)));
    visual.update(|window, cx| window.dispatch_action(Box::new(SelectDown), cx));
    assert_eq!(selected(visual), Some((1, 0)), "跨组下行");
    // up 回命令组末条；End → 会话组末条；Home → 全局第一条
    visual.update(|window, cx| window.dispatch_action(Box::new(SelectUp), cx));
    assert_eq!(selected(visual), Some((0, 2)));
    visual.update(|window, cx| window.dispatch_action(Box::new(SelectLast), cx));
    assert_eq!(selected(visual), Some((1, 0)), "End 落会话组末条");
    visual.update(|window, cx| window.dispatch_action(Box::new(SelectFirst), cx));
    assert_eq!(selected(visual), Some((0, 0)), "Home 落命令组首条");
}

// ======================================================================
// 4. 命令执行
// ======================================================================

/// 确认「新会话」条目：组件 dispatch 既有 NewSession action（应用级
/// 处理器接住，无第二份逻辑），面板收场。
#[gpui_kit::test]
fn confirming_command_dispatches_action_and_closes(cx: &mut TestAppContext) {
    // 应用级处理器先于开窗注册（palette_window 独占借用 cx）
    let fired: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
    let fired_handler = fired.clone();
    cx.update(|cx| {
        cx.on_action(move |_: &NewSession, _| {
            *fired_handler.borrow_mut() += 1;
        });
    });
    let (_chat, _service, palette, _state, visual) = palette_window(cx);
    open_palette(&palette, visual);
    // 选中停在命令组第一条（新会话），Enter 确认
    visual.update(|window, cx| {
        window.dispatch_action(
            Box::new(gpui_kit::base::actions::Confirm { secondary: false }),
            cx,
        );
    });
    visual.run_until_parked();
    assert_eq!(*fired.borrow(), 1, "NewSession action 应被 dispatch");
    visual.update(|_window, cx| assert!(!palette.read(cx).open, "确认后面板收场"));
}

// ======================================================================
// 5. 会话结果定位
// ======================================================================

/// 查询命中标题 → 确认 → 服务 open_session 装载历史到会话实体。
#[gpui_kit::test]
fn confirming_session_result_opens_session(cx: &mut TestAppContext) {
    // 执行器句柄先取（palette_window 独占借用 cx 后再取会撞借用）
    let executor = cx.update(|cx| cx.background_executor().clone());
    let (chat, _service, palette, state, visual) = palette_window(cx);
    let target = seeded_session(&state, "rust lifetimes discussion");
    seeded_session(&state, "unrelated talk");
    open_palette(&palette, visual);
    // 输入查询（组件 on_query 回调 → 防抖后台过滤）
    visual.update(|window, cx| {
        palette.update(cx, |palette, cx| {
            palette.command.update(cx, |command, cx| {
                command.set_query("lifetimes", window, cx);
            });
        });
    });
    // 防抖定时走假时钟（测试执行器的 timer 不随 park 自动到点）
    executor.advance_clock(std::time::Duration::from_millis(200));
    visual.run_until_parked();
    visual.update(|_window, cx| {
        let palette = palette.read(cx);
        assert_eq!(palette.visible_sessions.len(), 1, "只有标题命中的会话");
        assert_eq!(palette.visible_sessions[0].id, target);
    });
    // 选中移动到会话组第一条，Enter 确认
    visual.update(|window, cx| window.dispatch_action(Box::new(SelectDown), cx));
    visual.update(|window, cx| window.dispatch_action(Box::new(SelectDown), cx));
    visual.update(|window, cx| window.dispatch_action(Box::new(SelectDown), cx));
    visual.update(|window, cx| {
        window.dispatch_action(
            Box::new(gpui_kit::base::actions::Confirm { secondary: false }),
            cx,
        );
    });
    visual.run_until_parked();
    visual.update(|_window, cx| {
        assert!(!palette.read(cx).open, "确认后面板收场");
        let conversation = chat.read(cx);
        assert_eq!(
            conversation.row_count(),
            1,
            "会话历史应装载（open_session 定位链路）"
        );
    });
}

// ======================================================================
// 6. 防抖对账
// ======================================================================

/// 连发两查询：第一个查询的定时被任务替换取消，代次守卫兜底——最终
/// 命中面只反映最后一次查询（过期回写丢弃）。
#[gpui_kit::test]
fn stale_query_results_are_dropped(cx: &mut TestAppContext) {
    // 执行器句柄先取（同上）
    let executor = cx.update(|cx| cx.background_executor().clone());
    let (_chat, _service, palette, state, visual) = palette_window(cx);
    seeded_session(&state, "alpha report");
    seeded_session(&state, "beta report");
    open_palette(&palette, visual);
    visual.update(|window, cx| {
        palette.update(cx, |palette, cx| {
            palette.command.update(cx, |command, cx| {
                command.set_query("alpha", window, cx);
            });
            // 同一轮 update 里再发一查（模拟连打）——alpha 的防抖任务
            // 立即被 drop（定时取消），代次 +1
            palette.command.update(cx, |command, cx| {
                command.set_query("beta", window, cx);
            });
        });
    });
    // 防抖定时走假时钟；连发两查询时首个任务已被 drop（定时取消），
    // 推进后只有 beta 的回写能落地
    executor.advance_clock(std::time::Duration::from_millis(200));
    visual.run_until_parked();
    visual.update(|_window, cx| {
        let palette = palette.read(cx);
        let titles: Vec<String> = palette
            .visible_sessions
            .iter()
            .map(|meta| meta.title.clone())
            .collect();
        assert_eq!(titles, vec!["beta report"], "过期（alpha）结果不得回写");
    });
}
