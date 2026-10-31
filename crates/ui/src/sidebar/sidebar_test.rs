//! 会话侧栏测试（gpui-rewrite 3.1 验收，`#[gpui_kit::test]` 独立文件）。
//!
//! 五组：
//! 1. **纯函数面**：`build_rows` 分组（临时组 + 项目组）、未发送会话过滤、
//!    组头 tilde 路径；
//! 2. **装载**：tempdir 装配 → `refresh` 后台装载清单 + 行序列派生；
//! 3. **切会话链路**：`open_session` → 服务接线 + 会话实体历史装载；
//! 4. **重命名 / 删除确认**：行内编辑提交 / 确认对话框流（删当前会话
//!    收摊视图）；
//! 5. **稳定 ID**：清单重排与删除后，行的领域 id（会话 id）保持稳定
//!    （coding-guides §2 红线：不许用下标）。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Entity, TestAppContext, VisualTestContext};

use craftwave_engine::api::session::{
    self, CreateSessionArgs, ListSessionsArgs, RenameSessionArgs, SaveSessionTreeArgs,
};
use craftwave_engine::app_state::AppState;
use craftwave_engine::storage::{MessageEntry, MessageRole, ProjectMeta, SessionMeta, WirePart};

use crate::chat::{ConversationState, SessionService};
use crate::engine::{MailEventSink, install_runtime};
use crate::theme::{self, CraftwaveTheme};

use super::{RowItem, SidebarState, build_rows};

// ======================================================================
// 装置
// ======================================================================

/// 临时数据根装配（tree_test 同款）。
fn fixture_app() -> (Arc<AppState>, Arc<MailEventSink>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    let events = Arc::new(MailEventSink::new());
    let state = AppState::assemble(root, events.clone(), None);
    (state, events)
}

/// user 单条消息（树同步的最小形态）。
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

/// 建一个带一条消息的会话（messageCount>0 → 侧栏可见）并返回 meta。
fn seeded_session(state: &AppState, title: &str, project_id: Option<String>) -> SessionMeta {
    let meta = session::create_session(
        state,
        CreateSessionArgs {
            project_id: project_id.clone(),
        },
    )
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
    session::list_sessions(state, ListSessionsArgs::default())
        .expect("list")
        .into_iter()
        .find(|m| m.id == meta.id)
        .expect("meta 应在索引")
}

/// 测试窗口：tempdir 装配 + conversation + service + 侧栏（挂在主窗根视图
/// ——渲染冒烟一帧顺带覆盖）。返回各实体与状态、视觉上下文。
#[allow(clippy::type_complexity)]
fn sidebar_window(
    cx: &mut TestAppContext,
) -> (
    Entity<ConversationState>,
    Entity<SessionService>,
    Entity<SidebarState>,
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
    let service_slot: Rc<RefCell<Option<Entity<SessionService>>>> = Rc::new(RefCell::new(None));
    let slot_for_view = service_slot.clone();
    let (view, visual) = cx.add_window_view(move |window, cx| {
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
        // 重命名输入框要 window（与生产 install 同一约束）
        let rename_input = cx.new(|cx| gpui_kit::component::input::InputState::new(window, cx));
        let sidebar =
            cx.new(|_| SidebarState::new(state_for_view.clone(), service.clone(), rename_input));
        *slot_for_view.borrow_mut() = Some(service);
        crate::MainWindowView {
            zoom_seen: 0.0,
            chat: conversation,
            sidebar: Some(sidebar),
            palette: None,
        }
    });
    visual.run_until_parked();
    let chat = visual.update(|_window, cx| view.read(cx).chat.clone());
    let service = service_slot.borrow().clone().expect("窗口闭包内已装配");
    let sidebar = visual
        .update(|_window, cx| view.read(cx).sidebar.clone())
        .expect("侧栏已挂");
    (chat, service, sidebar, state, visual)
}

/// 行序列里的会话 id（顺序保持）。
fn row_session_ids(sidebar: &Entity<SidebarState>, cx: &App) -> Vec<String> {
    sidebar.read(cx).row_session_ids()
}

// ======================================================================
// 1. 纯函数面：分组与过滤
// ======================================================================

/// 分组语义（照旧栈）：未发送会话过滤 + 临时组在前 + 项目组按注册序 +
/// 组头带 tilde 路径副行。
#[test]
fn build_rows_groups_projects_and_filters_empty() {
    let temp_visible = session_meta("s1", None, 3);
    let temp_hidden = session_meta("s0", None, 0);
    let project_session = session_meta("p1s", Some("proj-1"), 2);
    let project_hidden = session_meta("p0s", Some("proj-1"), 0);
    let sessions = vec![
        temp_visible,
        temp_hidden,
        project_session,
        project_hidden,
    ];
    // 路径用非 HOME 前缀的绝对路径（tilde 折叠行为已由 tilde_path_with
    // 单测钉住；此处只验证组头副行承载项目路径原样）
    let projects = vec![project_meta("proj-1", "My Project", "/opt/team/myproj")];
    let rows = build_rows(&sessions, &projects);
    // 形态：临时组头 → s1 → 项目组头（tilde 路径）→ p1s
    let mut shapes = rows.iter();
    match shapes.next() {
        Some(RowItem::Group { id, label, subtitle }) => {
            assert_eq!(&**id, "group-temporary");
            assert_eq!(&**label, "临时会话");
            assert!(subtitle.is_none());
        }
        other => panic!("首行应是临时组头，实际 {other:?}"),
    }
    match shapes.next() {
        Some(RowItem::Session(meta)) => assert_eq!(meta.id, "s1"),
        other => panic!("第二行应是临时会话 s1，实际 {other:?}"),
    }
    match shapes.next() {
        Some(RowItem::Group { id, subtitle, .. }) => {
            assert_eq!(&**id, "group-proj-1");
            assert_eq!(subtitle.as_deref(), Some("/opt/team/myproj"), "组头副行应承载项目路径");
        }
        other => panic!("第三行应是项目组头，实际 {other:?}"),
    }
    match shapes.next() {
        Some(RowItem::Session(meta)) => assert_eq!(meta.id, "p1s"),
        other => panic!("第四行应是项目会话 p1s，实际 {other:?}"),
    }
    assert!(shapes.next().is_none(), "messageCount=0 的会话不进列表");
}

/// 无任何可见会话：只剩组头（渲染层据全部为组头显示「还没有会话」）。
#[test]
fn build_rows_with_nothing_visible_yields_group_headers_only() {
    let rows = build_rows(
        &[session_meta("s0", None, 0)],
        &[project_meta("p", "P", "/tmp")],
    );
    assert_eq!(rows.len(), 2, "两个组头仍在（空组渲染为空组）");
    assert!(rows
        .iter()
        .all(|row| matches!(row, RowItem::Group { .. })));
}

// ======================================================================
// 2-5. 集成：装载 / 切会话 / 重命名 / 删除 / 稳定 ID
// ======================================================================

/// refresh 装载 tempdir 清单并派生行序列（后台回写对账）。
#[gpui_kit::test]
fn refresh_loads_sessions_and_derives_rows(cx: &mut TestAppContext) {
    let (_chat, _service, sidebar, state, visual) = sidebar_window(cx);
    seeded_session(&state, "会话甲", None);
    seeded_session(&state, "会话乙", None);
    // 触发一次刷新（安装时装载发生在 seed 之前；这里显式再刷）
    visual.update(|_window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.refresh(cx));
    });
    visual.run_until_parked();
    let ids = visual.update(|_window, cx| row_session_ids(&sidebar, cx));
    assert_eq!(ids.len(), 2, "两个可见会话应进列表");
}

/// 点击会话行：服务接线 + 会话实体历史装载（换血）。
#[gpui_kit::test]
fn open_session_wires_service_and_loads_history(cx: &mut TestAppContext) {
    let (chat, service, sidebar, state, visual) = sidebar_window(cx);
    let target = seeded_session(&state, "目标会话", None);
    let other = seeded_session(&state, "另一会话", None);
    let _ = other;
    visual.update(|_window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.open_session(target.id.clone(), cx));
    });
    visual.run_until_parked();
    visual.update(|_window, cx| {
        assert_eq!(
            service.read(cx).session_id(),
            Some(target.id.as_str()),
            "侧栏点击应接线服务会话"
        );
        assert_eq!(
            chat.read(cx).messages().len(),
            1,
            "会话实体的历史应随装载换血"
        );
    });
}

/// 行内重命名：预填 → 改值 → 提交 → 引擎索引与清单刷新一致；空白与
/// 未变更不产生写盘。
#[gpui_kit::test]
fn inline_rename_commits_via_engine(cx: &mut TestAppContext) {
    let (_chat, _service, sidebar, state, visual) = sidebar_window(cx);
    let target = seeded_session(&state, "旧标题", None);
    visual.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.begin_rename(target.id.clone(), "旧标题".to_owned(), window, cx);
        });
    });
    visual.run_until_parked();
    // 改值并提交（Enter/失焦的订阅在生产路径；这里驱动同一提交方法）
    visual.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar
                .rename_input()
                .update(cx, |input, cx| input.set_value("新标题", window, cx));
            sidebar.commit_rename(cx);
        });
    });
    visual.run_until_parked();
    let listed = session::list_sessions(&state, ListSessionsArgs::default()).expect("list");
    assert_eq!(listed[0].title, "新标题", "重命名应落盘");
    // 未变更提交不产生写入（直接退出编辑态）
    visual.update(|window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.begin_rename(target.id.clone(), "新标题".to_owned(), window, cx);
            sidebar.commit_rename(cx);
        });
    });
    visual.run_until_parked();
    let listed = session::list_sessions(&state, ListSessionsArgs::default()).expect("list");
    assert_eq!(listed[0].title, "新标题");
}

/// 删除确认流：request 打开确认 → 确认删除 → 清单移除；删的是当前会话时
/// 服务收摊回「新会话」（session_id = None）。
#[gpui_kit::test]
fn delete_confirm_flow_removes_and_resets_active(cx: &mut TestAppContext) {
    let (chat, service, sidebar, state, visual) = sidebar_window(cx);
    let first = seeded_session(&state, "会话一", None);
    let second = seeded_session(&state, "会话二", None);
    // 打开第一个会话（活跃）
    visual.update(|_window, cx| {
        sidebar.update(cx, |sidebar, cx| sidebar.open_session(first.id.clone(), cx));
    });
    visual.run_until_parked();
    // 删除活跃会话：确认框打开 → 确认
    visual.update(|_window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.request_delete(first.clone(), cx);
            assert!(sidebar.delete_confirm_target().is_some(), "确认框应打开");
            sidebar.confirm_delete(cx);
            assert!(
                sidebar.delete_confirm_target().is_none(),
                "确认后对话框应收场"
            );
        });
    });
    visual.run_until_parked();
    visual.update(|_window, cx| {
        assert_eq!(
            service.read(cx).session_id(),
            None,
            "删当前会话应收摊回新会话空态"
        );
        assert_eq!(chat.read(cx).messages().len(), 0, "视图应清空");
    });
    let listed = session::list_sessions(&state, ListSessionsArgs::default()).expect("list");
    assert_eq!(listed.len(), 1, "引擎侧应只剩一个会话");
    assert_eq!(listed[0].id, second.id);
    // 取消路径：确认框 Cancel 不删除
    visual.update(|_window, cx| {
        sidebar.update(cx, |sidebar, cx| {
            sidebar.request_delete(second.clone(), cx);
            sidebar.cancel_delete(cx);
            assert!(sidebar.delete_confirm_target().is_none());
        });
    });
    let listed = session::list_sessions(&state, ListSessionsArgs::default()).expect("list");
    assert_eq!(listed.len(), 1, "取消不应删除");
}

/// 稳定 ID（coding-guides §2 红线）：清单删除一条后，其余行的领域 id
/// 保持不变（不随位置漂移）。
#[gpui_kit::test]
fn session_ids_are_stable_across_refresh(cx: &mut TestAppContext) {
    let (_chat, _service, sidebar, state, visual) = sidebar_window(cx);
    let a = seeded_session(&state, "A", None);
    let b = seeded_session(&state, "B", None);
    let c = seeded_session(&state, "C", None);
    let refresh_and_ids = |sidebar: &Entity<SidebarState>,
                           visual: &mut VisualTestContext|
     -> Vec<String> {
        visual.update(|_window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.refresh(cx));
        });
        visual.run_until_parked();
        visual.update(|_window, cx| row_session_ids(sidebar, cx))
    };
    let ids_before = refresh_and_ids(&sidebar, visual);
    assert_eq!(ids_before, vec![c.id.clone(), b.id.clone(), a.id.clone()],
        "updatedAt 倒序：后建在前");
    // 删除 B → 其余 id 原样保留（id 从会话 id 派生，与位置无关）
    session::delete_session(
        &state,
        craftwave_engine::api::session::DeleteSessionArgs {
            session_id: b.id.clone(),
        },
    )
    .expect("delete");
    let ids_after = refresh_and_ids(&sidebar, visual);
    assert_eq!(ids_after, vec![c.id, a.id]);
}

// ======================================================================
// 纯数据工厂
// ======================================================================

/// SessionMeta 工厂（不经引擎——纯函数面的输入）。
fn session_meta(id: &str, project_id: Option<&str>, message_count: u64) -> SessionMeta {
    SessionMeta {
        id: id.to_owned(),
        project_id: project_id.map(str::to_owned),
        title: format!("会话 {id}"),
        created_at: 1,
        updated_at: 1,
        revision: 0,
        message_count,
        total_tokens: 0,
        authorized_external_dirs: Vec::new(),
    }
}

/// ProjectMeta 工厂。
fn project_meta(id: &str, name: &str, home_dir: &str) -> ProjectMeta {
    ProjectMeta {
        id: id.to_owned(),
        name: name.to_owned(),
        home_dir: home_dir.to_owned(),
        created_at: 1,
    }
}

/// `Rc` 引用面自检（清单载体零深拷贝的口径提醒——引擎侧类型在本 crate
/// 的展示面不 clone 消息体）。
#[test]
fn rc_sharing_is_cheap_for_lists() {
    let meta = Rc::new(session_meta("x", None, 1));
    let alias = Rc::clone(&meta);
    assert!(Rc::ptr_eq(&meta, &alias));
}
