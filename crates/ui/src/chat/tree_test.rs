//! 会话树与分支测试（gpui-rewrite 3.3 验收，`#[gpui_kit::test]` 独立文件）。
//!
//! 三组：
//! 1. **纯函数面**：`build_branches`（兄弟分组表）/ `branch_neighbor`（相邻
//!    一支与到头边界）/ `deepest_leaf`（确定性叶）/ `active_path`（活动路径
//!    线性序——「消息流只渲活动路径」的钉子）；
//! 2. **服务集成**（tempdir 装配）：`open_session` 装载活动路径 + 分支表 →
//!    `switch_branch` 切活动叶 + 重载（落盘核对 + busy 拒绝）；
//! 3. **编辑重发**（3.3 分支入口）：`begin_resend` 待发态 → `branch_resend`
//!    产生同父新兄弟分支（树上落盘核对）；单分支的消息不产生分支控件数据
//!    （count ≤ 1 渲染层隐藏的判定面）。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};

use craftwave_engine::api::session::{
    self, CreateSessionArgs, LoadSessionTreeArgs, RenameSessionArgs, SaveSessionTreeArgs,
};
use craftwave_engine::app_state::AppState;
use craftwave_engine::storage::{MessageEntry, MessageRole, WirePart};

use crate::engine::MailEventSink;
use crate::theme::{self, CraftwaveTheme};

use super::service::{self, BranchDir, SessionService};
use super::state::ConversationState;

/// 事件/帧等待超时。
const WAIT: Duration = Duration::from_secs(10);

// ======================================================================
// 装置
// ======================================================================

/// 临时数据根装配（service_test 的 fixture 同款）。
fn fixture_app() -> (Arc<AppState>, Arc<MailEventSink>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    let events = Arc::new(MailEventSink::new());
    let state = AppState::assemble(root, events.clone(), None);
    (state, events)
}

/// 手工建一棵两分支树：
///
/// ```text
/// u1 ── a1          （分支 1：a1 为叶）
///   └── u2 ── a2    （分支 2：a2 为叶）
/// ```
/// 返回（会话 id, 各节点 id）。
fn seed_branchy_tree(state: &AppState) -> (String, HashMap<&'static str, String>) {
    let meta = session::create_session(state, CreateSessionArgs::default()).expect("create");
    let sid = meta.id;
    let entry = |id: &str, parent: Option<&str>, role: MessageRole, text: &str| MessageEntry {
        id: id.to_owned(),
        parent_id: parent.map(str::to_owned),
        role,
        parts: vec![WirePart::Text {
            text: text.to_owned(),
        }],
        created_at: 1_700_000_000_000,
        run_id: None,
        status: None,
    };
    let messages = vec![
        entry("u1", None, MessageRole::User, "第一问"),
        entry("a1", Some("u1"), MessageRole::Assistant, "回答甲"),
        entry("u2", Some("u1"), MessageRole::User, "第一问（重发）"),
        entry("a2", Some("u2"), MessageRole::Assistant, "回答乙"),
    ];
    session::save_session_tree(
        state,
        SaveSessionTreeArgs {
            session_id: sid.clone(),
            messages,
            active_leaf_id: Some("a1".to_owned()),
            base_revision: 0,
        },
    )
    .expect("save tree");
    let mut ids = HashMap::new();
    ids.insert("u1", "u1".to_owned());
    ids.insert("a1", "a1".to_owned());
    ids.insert("u2", "u2".to_owned());
    ids.insert("a2", "a2".to_owned());
    (sid, ids)
}

/// 测试会话窗口：tempdir 装配 + conversation + SessionService（service_test
/// 的 `service_window` 同款形态，tokio runtime 常驻）。调度器句柄供心跳
/// 循环的 fake clock 推进（与 service_test 同款：借用外推进）。
#[allow(clippy::type_complexity)]
fn tree_window(
    cx: &mut TestAppContext,
) -> (
    Entity<ConversationState>,
    Entity<SessionService>,
    Arc<AppState>,
    gpui_kit::BackgroundExecutor,
    &mut VisualTestContext,
) {
    let (state, events) = fixture_app();
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    let executor = cx.executor();
    cx.update(|cx| {
        gpui_kit::init(cx);
        CraftwaveTheme::mount(cx);
        theme::apply(cx);
    });
    let state_for_view = state.clone();
    let events_for_view = events.clone();
    let (view, visual) = cx.add_window_view(move |_window, cx| crate::MainWindowView {
        zoom_seen: 0.0,
        chat: cx.new(|_| ConversationState::new()),
        sidebar: None,
    });
    visual.run_until_parked();
    let chat = visual.update(|_window, cx| view.read(cx).chat.clone());
    let service = visual.update(|_window, cx| {
        let conversation = chat.clone();
        cx.new(|_| {
            SessionService::new(
                state_for_view.clone(),
                events_for_view.clone(),
                runtime,
                conversation.downgrade(),
            )
        })
    });
    visual.update(|_window, cx| {
        chat.update(cx, |chat, _| chat.service = Some(service.clone()));
    });
    (chat, service, state, executor, visual)
}

/// 轮询等待条件成立（run 是后台任务真实线程推进；心跳循环的 timer 是
/// gpui fake clock，每轮推一个节流窗口——service_test 的 wait_for 同款）。
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

// ======================================================================
// 1. 纯函数面
// ======================================================================

/// 分支表：同父兄弟成组、根消息（无父）同组；序 = 数组序。
#[test]
fn build_branches_groups_siblings_by_parent() {
    let msg = |id: &str, parent: Option<&str>| MessageEntry {
        id: id.to_owned(),
        parent_id: parent.map(str::to_owned),
        role: MessageRole::User,
        parts: vec![WirePart::Text { text: "x".into() }],
        created_at: 0,
        run_id: None,
        status: None,
    };
    let tree = vec![
        msg("root", None),
        msg("a", Some("root")),
        msg("b", Some("root")),
        msg("a1", Some("a")),
    ];
    let table = service::build_branches(&tree);
    // 根组：root 一枝（count 1）
    assert_eq!(
        table.get("root").copied(),
        Some(super::state::SiblingInfo { index: 0, count: 1 })
    );
    // root 的两个孩子：a 第 0/共 2，b 第 1/共 2
    assert_eq!(
        table.get("a").copied(),
        Some(super::state::SiblingInfo { index: 0, count: 2 })
    );
    assert_eq!(
        table.get("b").copied(),
        Some(super::state::SiblingInfo { index: 1, count: 2 })
    );
    // 单叶链：count 1（渲染层隐藏控件）
    assert_eq!(
        table.get("a1").copied(),
        Some(super::state::SiblingInfo { index: 0, count: 1 })
    );
}

/// 相邻一支：同组内前后移动；到头/消息不在树上返回 None。
#[test]
fn branch_neighbor_walks_siblings_and_stops_at_edges() {
    let msg = |id: &str, parent: Option<&str>| MessageEntry {
        id: id.to_owned(),
        parent_id: parent.map(str::to_owned),
        role: MessageRole::User,
        parts: vec![WirePart::Text { text: "x".into() }],
        created_at: 0,
        run_id: None,
        status: None,
    };
    let tree = vec![
        msg("root", None),
        msg("a", Some("root")),
        msg("b", Some("root")),
        msg("c", Some("root")),
    ];
    // 中间：前后都可走
    assert_eq!(
        service::branch_neighbor(&tree, "b", BranchDir::Prev),
        Some("a".to_owned())
    );
    assert_eq!(
        service::branch_neighbor(&tree, "b", BranchDir::Next),
        Some("c".to_owned())
    );
    // 到头
    assert_eq!(service::branch_neighbor(&tree, "a", BranchDir::Prev), None);
    assert_eq!(service::branch_neighbor(&tree, "c", BranchDir::Next), None);
    // 消息不在树上
    assert_eq!(service::branch_neighbor(&tree, "zz", BranchDir::Next), None);
}

/// 确定性叶：从起点沿「数组序最后的孩子」走到底。
#[test]
fn deepest_leaf_follows_last_children() {
    let msg = |id: &str, parent: Option<&str>| MessageEntry {
        id: id.to_owned(),
        parent_id: parent.map(str::to_owned),
        role: MessageRole::User,
        parts: vec![WirePart::Text { text: "x".into() }],
        created_at: 0,
        run_id: None,
        status: None,
    };
    // b 有两个孩子 b1/b2（b2 在数组序更后），b2 是叶
    let tree = vec![
        msg("root", None),
        msg("b", Some("root")),
        msg("b1", Some("b")),
        msg("b2", Some("b")),
    ];
    assert_eq!(service::deepest_leaf(&tree, "root"), "b2");
    // 起点即叶
    assert_eq!(service::deepest_leaf(&tree, "b1"), "b1");
}

/// 钉住：`active_path` 只取活动叶的祖先链（「消息流只渲活动路径」——
/// 3.3 定稿原则，无旧数据收养语义，只有本会话重载）。
#[test]
fn active_path_contains_only_leaf_chain() {
    let msg = |id: &str, parent: Option<&str>| MessageEntry {
        id: id.to_owned(),
        parent_id: parent.map(str::to_owned),
        role: MessageRole::User,
        parts: vec![WirePart::Text { text: "x".into() }],
        created_at: 0,
        run_id: None,
        status: None,
    };
    let tree = vec![
        msg("u1", None),
        msg("a1", Some("u1")),
        msg("u2", Some("u1")),
        msg("a2", Some("u2")),
    ];
    // 叶 = a1：路径 u1 → a1（不含 u2/a2 分支）
    let path = service::active_path(&tree, Some("a1"));
    let ids: Vec<&str> = path.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["u1", "a1"]);
    // 叶 = a2：路径 u1 → u2 → a2
    let path = service::active_path(&tree, Some("a2"));
    let ids: Vec<&str> = path.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["u1", "u2", "a2"]);
    // 叶 = None：空路径
    assert!(service::active_path(&tree, None).is_empty());
}

// ======================================================================
// 2. 服务集成：装载 → 切分支 → 重载
// ======================================================================

/// open_session 装载活动路径历史 + 分支表；switch_branch 切活动叶 + 重载
/// （ConversationState 换血 + 盘上 active_leaf 核对）。
#[gpui_kit::test]
fn switch_branch_swaps_active_leaf_and_reloads(cx: &mut TestAppContext) {
    let (chat, service, state, _executor, visual) = tree_window(cx);
    let (sid, ids) = seed_branchy_tree(&state);
    // 打开会话：活动叶 = a1 → 历史 [u1, a1]，分支表 a1/u2 各 count 2
    let sid_open = sid.clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.open_session(sid_open, cx));
    });
    visual.run_until_parked();
    visual.update(|_window, cx| {
        let messages = chat.read(cx).messages();
        let ids_seen: Vec<&str> = messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids_seen, vec!["u1", "a1"], "活动路径应只含分支 1");
        // 分支表已随装载推送
        let a1 = chat.read(cx).branch_of("a1").expect("分支表应含 a1");
        assert_eq!((a1.index, a1.count), (0, 2));
        let u2 = chat.read(cx).branch_of("u2").expect("分支表应含 u2");
        assert_eq!((u2.index, u2.count), (1, 2));
    });

    // 分支点在 u1 的孩子层：a1 与 u2 是同父兄弟。a1 的 Next（u2）成为
    // 活动叶：叶沿最后孩子走到底 = a2
    let a1 = ids["a1"].clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| {
            service.switch_branch(&a1, BranchDir::Next, cx)
        });
    });
    visual.run_until_parked();
    visual.update(|_window, cx| {
        let messages = chat.read(cx).messages();
        let ids_seen: Vec<&str> = messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids_seen, vec!["u1", "u2", "a2"], "切到分支 2 应重载历史");
    });
    // 落盘核对：active_leaf = a2（重 load 一致——「关键点落盘」）
    let tree = session::load_session_tree(
        &state,
        LoadSessionTreeArgs { session_id: sid },
    )
    .expect("reload tree");
    assert_eq!(tree.active_leaf_id.as_deref(), Some("a2"));
}

/// busy（run 进行中）时拒绝切分支（树归 run 落盘器持有）。
#[gpui_kit::test]
fn switch_branch_is_rejected_while_busy(cx: &mut TestAppContext) {
    let (_chat, service, state, _executor, visual) = tree_window(cx);
    let (sid, ids) = seed_branchy_tree(&state);
    let sid_open = sid.clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.open_session(sid_open, cx));
    });
    visual.run_until_parked();
    // 人为置 busy（对应「有 run 在跑」；真实路径由发送链置位——测试走
    // begin_pipeline 的测试面：起管道即置位 busy）
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| {
            let _mailbox = service.begin_pipeline(cx);
        });
    });
    let a1 = ids["a1"].clone();
    let sid_check = sid.clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| {
            service.switch_branch(&a1, BranchDir::Next, cx)
        });
    });
    visual.run_until_parked();
    // 盘上活动叶未动（切换被拒）
    let tree = session::load_session_tree(
        &state,
        LoadSessionTreeArgs {
            session_id: sid_check,
        },
    )
    .expect("reload tree");
    assert_eq!(tree.active_leaf_id.as_deref(), Some("a1"));
}

/// 无兄弟/到头的方向：切换为无动作（不落盘不重载）。
#[gpui_kit::test]
fn switch_branch_at_edge_is_noop(cx: &mut TestAppContext) {
    let (_chat, service, state, _executor, visual) = tree_window(cx);
    let (sid, ids) = seed_branchy_tree(&state);
    let sid_open = sid.clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.open_session(sid_open, cx));
    });
    visual.run_until_parked();
    // a1 是第 0 个兄弟：Prev 到头，无动作
    let a1 = ids["a1"].clone();
    let sid_check = sid.clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| {
            service.switch_branch(&a1, BranchDir::Prev, cx)
        });
    });
    visual.run_until_parked();
    let tree = session::load_session_tree(
        &state,
        LoadSessionTreeArgs {
            session_id: sid_check,
        },
    )
    .expect("reload tree");
    assert_eq!(tree.active_leaf_id.as_deref(), Some("a1"), "到头应无动作");
}

// ======================================================================
// 3. 编辑重发（3.3 分支入口）与单分支隐藏判定面
// ======================================================================

/// 编辑重发：待发态 + branch_resend 在目标 parent 下落盘新 user 兄弟节点
/// 并切活动叶（新分支与原消息同父——旧栈 ActionBarPrimitive.Edit 语义）。
#[gpui_kit::test]
fn branch_resend_creates_sibling_under_parent(cx: &mut TestAppContext) {
    let (chat, service, state, executor, visual) = tree_window(cx);
    let (sid, _ids) = seed_branchy_tree(&state);
    let sid_open = sid.clone();
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| service.open_session(sid_open, cx));
    });
    visual.run_until_parked();
    // 待发态：编辑 u1（根消息，parent None）
    visual.update(|_window, cx| {
        let snapshot = chat.read(cx).messages()[0].as_ref().clone();
        chat.update(cx, |chat, cx| chat.begin_resend(&snapshot, cx));
        assert!(chat.read(cx).resend_pending());
    });
    // 发送（走 branch_resend；tempdir 无模型 → start_chat 失败收口，
    // 但 user 节点已先行落盘——树同步在发送链最前）
    visual.update(|_window, cx| {
        service.update(cx, |service, cx| {
            // parent None：u1 是根消息，新分支同挂根
            service.branch_resend(None, "第一问（编辑重发）".to_owned(), Vec::new(), cx);
        });
    });
    wait_for(
        &executor,
        visual,
        |visual| visual.update(|_window, cx| !service.read(cx).busy()),
        "编辑重发收口",
    );
    // 盘上核对：新 user 节点挂在 u1 的 parent（None = 根）下，成为 u1 的
    // 兄弟；active_leaf 切到新节点
    let sid_check = sid.clone();
    let tree = session::load_session_tree(
        &state,
        LoadSessionTreeArgs {
            session_id: sid_check,
        },
    )
    .expect("reload tree");
    let u1 = tree.messages.iter().find(|m| m.id == "u1").expect("u1 在树");
    let new_siblings: Vec<&MessageEntry> = tree
        .messages
        .iter()
        .filter(|m| m.parent_id == u1.parent_id && m.id != "u1" && m.role == MessageRole::User)
        .collect();
    assert_eq!(new_siblings.len(), 1, "应有恰好一个新 user 兄弟节点");
    assert_eq!(
        new_siblings[0].parts.first(),
        Some(&WirePart::Text {
            text: "第一问（编辑重发）".to_owned()
        })
    );
    assert_eq!(
        tree.active_leaf_id.as_deref(),
        Some(new_siblings[0].id.as_str()),
        "活动叶应切到新分支"
    );
    // 待发态一次性：发送链 take 后（这里手动清）回常规
    visual.update(|_window, cx| {
        chat.update(cx, |chat, cx| chat.cancel_resend(cx));
        assert!(!chat.read(cx).resend_pending());
    });
}

/// 重命名挂进侧栏前的服务侧核对（rename 走 engine，无壳态分支语义）。
#[gpui_kit::test]
fn session_meta_roundtrip_for_sidebar(_cx: &mut TestAppContext) {
    let (state, _events) = fixture_app();
    let meta = session::create_session(&state, CreateSessionArgs::default()).expect("create");
    session::rename_session(
        &state,
        RenameSessionArgs {
            session_id: meta.id.clone(),
            title: "改名后的会话".to_owned(),
        },
    )
    .expect("rename");
    let listed = session::list_sessions(&state, Default::default()).expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "改名后的会话");
}
