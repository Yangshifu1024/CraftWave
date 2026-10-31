//! 消息流底座测试（gpui-rewrite 2.4 验收，`#[gpui_kit::test]` 独立文件）。
//!
//! 覆盖 rendering.md 的本批验收点：
//! 1. 行分发四形态各渲一帧不 panic + 过滤器正反例（纪律二）；
//! 2. 展开状态两张表跨重渲染记忆 + splice(ix..ix+1,1) 重测被触发（纪律一）；
//! 3. 新消息滚动跟随、上滚暂停、回底续跟（选择拖到视口边缘的自动滚动
//!    由 gpui-base 的选择机制派发同型 ScrollEvent 驱动——这里用合成
//!    ScrollEvent 钉住列表对该事件通路的响应）；
//! 4. 流式行：出现 / 内容增长重测 / 带工具渲染 / 收口；
//! 5. apply_snapshot 指针对账：同历史快照不 reset（滚动状态不炸），
//!    新历史整体替换（展开表清空）；
//! 6. 正文换装 `TextView::markdown`（2.C-3）后：跨消息（跨 TextView）的
//!    文本选择自然生效（2.C-1 钉住的选择通路的收口验证）。

use gpui_kit::{
    Context, Entity, Modifiers, MouseButton, ScrollDelta, ScrollEvent, TestAppContext,
    VisualTestContext, point, px,
};

use craftwave_engine::snapshot::StreamPart;
use craftwave_engine::storage::MessageRole;

use crate::MainWindowView;
use crate::theme::{self, CraftwaveTheme};

use super::fixture;
use super::state::ConversationState;

/// 会话状态的可变操作入口（VisualTestContext 的 update 是窗口绑定的，
/// App 级操作经由它闭包里的 `&mut App` 完成——测试内统一的写法）。
fn mutate(
    chat: &Entity<ConversationState>,
    visual: &mut VisualTestContext,
    f: impl FnOnce(&mut ConversationState, &mut Context<ConversationState>),
) {
    visual.update(|_window, cx| {
        chat.update(cx, |chat, cx| f(chat, cx));
    });
}

/// 会话状态的只读断言入口。
fn observe<R>(
    chat: &Entity<ConversationState>,
    visual: &mut VisualTestContext,
    f: impl FnOnce(&ConversationState, &gpui_kit::App) -> R,
) -> R {
    visual.update(|_window, cx| f(chat.read(cx), cx))
}

/// 贴底判定：Bottom 对齐的 list 在底部时滚动锚为空（或指向行尾），此时
/// `logical_scroll_top().item_ix == row_count`。不用 `is_scrolled_to_end()`：
/// 惰性测量下视口外行高未知，它会返回 None（gpui 语义：总高未知即 None）。
fn at_bottom(chat: &ConversationState) -> bool {
    chat.list.logical_scroll_top().item_ix == chat.row_count()
}

/// 测试装配：与 main() 的启动序一致（init → mount → apply），再开一个
/// 中栏挂真 chat 装置的主窗。install 默认装演示历史，测试按需 set_history。
/// 根视图包 base `Root`（与生产 open_window 同款包装：窗口文本选择的
/// 全局协议、tooltip/浮层挂载点都在 Root 上——跨 TextView 选择测试依赖）。
fn chat_window(
    cx: &mut TestAppContext,
) -> (Entity<ConversationState>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        CraftwaveTheme::mount(cx);
        theme::apply(cx);
    });
    // install 的返回值经 Rc<RefCell> 带出闭包（闭包只调用一次）
    let holder: std::rc::Rc<std::cell::RefCell<Option<Entity<ConversationState>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = holder.clone();
    let (_root, visual) = cx.add_window_view(|window, cx| {
        use gpui_kit::AppContext as _;
        let chat = crate::chat::install(window, cx, None);
        *slot.borrow_mut() = Some(chat.clone());
        // Root::new 收 Entity<AnyView>：先建内层视图实体再包 Root
        let inner = cx.new(|_| MainWindowView {
            zoom_seen: 0.0,
            chat,
            sidebar: None,
        });
        gpui_kit::base::Root::new(inner, window, cx)
    });
    let chat = holder.borrow().clone().expect("install 在开窗闭包内执行");
    (chat, visual)
}

// ======================================================================
// 1. 行分发四形态 + 过滤器
// ======================================================================

/// 过滤器纯函数面：日志型正反例逐条判定（纪律二口径）。
#[gpui_kit::test]
fn filter_detects_log_messages_only(_cx: &mut TestAppContext) {
    // 正例：Assistant 只有 thinking（或空）→ 日志型
    assert!(ConversationState::is_log_entry(&fixture::log_only_assistant("只有思考")));
    assert!(ConversationState::is_log_entry(&fixture::empty_assistant()));
    // 反例：有正文 / 有工具 / 非 Assistant 都不算日志
    assert!(!ConversationState::is_log_entry(&fixture::assistant_msg("有正文")));
    assert!(!ConversationState::is_log_entry(&fixture::assistant_tool_only()));
    assert!(!ConversationState::is_log_entry(&fixture::user_msg("用户消息")));
    assert!(!ConversationState::is_log_entry(&fixture::system_msg("系统")));
    // 流式行同口径：只有 Reasoning 的流式态是日志型
    let mut thinking_only = fixture::streaming_message(6);
    thinking_only.parts = vec![StreamPart::Reasoning {
        text: "只想不说".to_owned(),
    }];
    assert!(ConversationState::is_log_streaming(&thinking_only));
    let mut with_text = thinking_only;
    with_text.parts.push(StreamPart::Text {
        text: "有正文了".to_owned(),
    });
    assert!(!ConversationState::is_log_streaming(&with_text));
}

/// 四形态各渲一帧不 panic；过滤器行为落在行上：日志行被滤、行号映射
/// 不变（纪律二：源数据保留，只不渲染）。
#[gpui_kit::test]
fn four_forms_render_and_log_row_is_filtered(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    // 换上覆盖四形态 + 过滤器正反例的历史（install 默认装的是演示历史）
    mutate(&chat, visual, |chat, cx| {
        chat.set_history(fixture::four_forms_history(), cx);
    });
    // 一帧全量 layout/prepaint/paint——形态构建错误会直接 panic
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        // 行数 = 条目数（流式缺席）；日志行（下标 1）被滤但占位保留
        assert_eq!(chat.row_count(), 6);
        assert!(chat.row_filtered(1), "只有 thinking 的 Assistant 应被滤");
        assert!(!chat.row_filtered(2), "三槽位 Assistant 正常渲染");
        assert!(!chat.row_filtered(0));
        assert!(!chat.row_filtered(3));
        assert!(!chat.row_filtered(5), "带图 User 正常渲染");
    });
    // 回声式追加一条 User，行数 +1 且映射仍按下标；渲染一帧（新增行走
    // User 气泡路径）
    mutate(&chat, visual, |chat, cx| {
        chat.append_message(fixture::user_msg("追加一条"), cx);
        assert_eq!(chat.row_count(), 7);
        assert!(chat.row_filtered(1));
    });
    visual.run_until_parked();
}

// ======================================================================
// 2. 展开状态（两张表）+ splice 重测
// ======================================================================

/// 展开状态跨重渲染记忆 + 每次开合触发 splice(ix..ix+1,1) 重测；
/// thinking 与工具簇两张表互不干扰（纪律一：折叠不嵌套、无自动收起）。
#[gpui_kit::test]
fn expansion_state_memories_across_rerenders_and_retests(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    mutate(&chat, visual, |chat, cx| {
        chat.set_history(fixture::four_forms_history(), cx);
    });
    visual.run_until_parked();
    let row = 2; // assistant_full：thinking + 正文 + 工具簇都在这一行
    observe(&chat, visual, |chat, _cx| {
        assert!(!chat.thinking_expanded(row));
        assert!(!chat.tools_expanded(row));
        assert_eq!(chat.splice_retest_count, 0);
    });
    // 真实点击折叠头（像素级）：thinking 展开 + 重测被触发
    let head = visual
        .debug_bounds("thinking-head-2")
        .expect("thinking 折叠头已渲染且在视口内");
    visual.simulate_click(head.center(), Default::default());
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(chat.thinking_expanded(row), "点击后应展开");
        assert!(!chat.tools_expanded(row), "两张表互不干扰");
        assert_eq!(chat.splice_retest_count, 1, "展开应触发 splice 重测");
        assert_eq!(chat.list.item_count(), 6, "splice 不改行数");
    });
    // 再制造一次重渲染（行数变化）：展开必须记忆（无自动收起）
    mutate(&chat, visual, |chat, cx| {
        chat.append_message(fixture::user_msg("触发一次重渲染"), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(chat.thinking_expanded(row), "跨重渲染记忆");
        assert_eq!(chat.splice_retest_count, 1, "重渲染本身不再触发重测");
    });
    // 工具簇开合：第二张表 + 各自走重测路径
    mutate(&chat, visual, |chat, cx| chat.toggle_tools(row, cx));
    visual.run_until_parked();
    mutate(&chat, visual, |chat, cx| {
        assert!(chat.tools_expanded(row));
        assert!(chat.thinking_expanded(row), "thinking 保持不动");
        assert_eq!(chat.splice_retest_count, 2);
        // 再折叠一次：计数继续走重测路径
        chat.toggle_tools(row, cx);
        assert!(!chat.tools_expanded(row));
        assert_eq!(chat.splice_retest_count, 3);
    });
    visual.run_until_parked();
}

// ======================================================================
// 3. 滚动跟随 / 上滚暂停 / 回底续跟 / 合成 ScrollEvent 通路
// ======================================================================

/// 新消息到达贴底跟随（Tail）；上滚暂停后追加不跟随；滚回底部自动续跟；
/// 列表响应 gpui-base 选择机制用于「拖到视口边缘自动滚动」的同型
/// ScrollEvent（AutoScroll::compute_delta 的产物就是这类事件）。
#[gpui_kit::test]
fn follow_tail_pauses_on_scroll_up_and_resumes_at_bottom(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    mutate(&chat, visual, |chat, cx| {
        chat.set_history(fixture::long_history(60), cx);
    });
    visual.run_until_parked();
    // 初始：Bottom 对齐 + Tail 跟随 → 贴底
    observe(&chat, visual, |chat, _cx| {
        assert!(at_bottom(chat), "初始应贴底");
        assert!(chat.list.is_following_tail());
    });
    // 列表视口中心（合成滚轮事件的落点）
    let viewport_center = observe(&chat, visual, |chat, _cx| {
        chat.list.viewport_bounds().center()
    });
    // 追加新消息 → 自动跟随到新底部
    mutate(&chat, visual, |chat, cx| {
        chat.append_message(fixture::user_msg("新消息一"), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(at_bottom(chat), "跟随中追加新消息应贴底");
    });
    // 上滚（正 delta = 向上）：Tail 暂停，视口停住
    visual.simulate_event(ScrollEvent {
        position: viewport_center,
        delta: ScrollDelta::Pixels(point(px(0.), px(120.))),
        ..Default::default()
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(!chat.list.is_following_tail(), "上滚应暂停跟随");
        assert!(!at_bottom(chat));
    });
    // 暂停中追加 → 不跟随（新消息不把视口拽走）
    mutate(&chat, visual, |chat, cx| {
        chat.append_message(fixture::user_msg("上滚期间到达的消息"), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(!chat.list.is_following_tail());
        assert!(!at_bottom(chat), "上滚暂停后新消息不应把视口拽到底部");
    });
    // 一次性滚回底部：gpui 内建「回底自动续跟」
    visual.simulate_event(ScrollEvent {
        position: viewport_center,
        delta: ScrollDelta::Pixels(point(px(0.), px(-1_000_000.))),
        ..Default::default()
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(chat.list.is_following_tail(), "滚回底部应自动续跟");
        assert!(at_bottom(chat));
    });
    // 续跟后再追加 → 贴底
    mutate(&chat, visual, |chat, cx| {
        chat.append_message(fixture::user_msg("续跟后的新消息"), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(at_bottom(chat));
    });
}

// ======================================================================
// 4. 流式行
// ======================================================================

/// 流式行生命周期：出现（尾部插一行）→ 内容增长（remeasure 重测路径）→
/// 带工具渲染 → 日志型流式态被滤 → 收口（尾部删一行 + 展开记忆清除）。
#[gpui_kit::test]
fn streaming_row_lifecycle(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    let history_len = observe(&chat, visual, |chat, _cx| chat.messages().len());
    // 出现：行数 +1，且 Tail 跟随贴底
    mutate(&chat, visual, |chat, cx| {
        chat.set_streaming(Some(fixture::streaming_message(12)), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert_eq!(chat.row_count(), history_len + 1);
        assert!(at_bottom(chat), "流式行出现即贴底");
        assert!(!chat.row_filtered(history_len), "有正文的流式行不被滤");
    });
    // 内容增长：走 remeasure_items 重测（不动行数）
    mutate(&chat, visual, |chat, cx| {
        chat.set_streaming(Some(fixture::streaming_message(30)), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert_eq!(chat.row_count(), history_len + 1, "内容增长不改行数");
        assert_eq!(chat.remeasure_count, 1, "流式更新应触发 remeasure");
    });
    // 带工具的流式行渲染一帧（工具簇槽位在流式行也要能走通）
    mutate(&chat, visual, |chat, cx| {
        chat.set_streaming(Some(fixture::streaming_with_tools()), cx);
    });
    visual.run_until_parked();
    // 终态帧（runEnded 已到、收口前的最后一次替换）走同一条更新路径
    mutate(&chat, visual, |chat, cx| {
        chat.set_streaming(Some(fixture::ended_streaming_message()), cx);
    });
    visual.run_until_parked();
    // 只有 thinking 的流式态 → 行被滤（纪律二对最后一行同样生效）
    mutate(&chat, visual, |chat, cx| {
        let mut log = fixture::streaming_message(6);
        log.parts = vec![StreamPart::Reasoning {
            text: "流式里只想不说".to_owned(),
        }];
        chat.set_streaming(Some(log), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(chat.row_filtered(history_len), "日志型流式行应被滤");
    });
    // 收口：尾部删一行；该行的展开记忆一并清除
    mutate(&chat, visual, |chat, cx| {
        chat.toggle_thinking(history_len, cx);
        assert!(chat.thinking_expanded(history_len));
        chat.set_streaming(None, cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert_eq!(chat.row_count(), history_len);
        assert!(!chat.thinking_expanded(history_len), "收口后展开记忆清除");
    });
}

// ======================================================================
// 5. apply_snapshot 指针对账
// ======================================================================

/// 同一历史 Rc 的重复快照零成本通过（不 reset、展开记忆保持）；
/// 新 Rc（内容相同的重建历史）触发整体替换 + 展开表清空。
#[gpui_kit::test]
fn apply_snapshot_routes_by_history_identity(cx: &mut TestAppContext) {
    let (chat, visual) = chat_window(cx);
    let history = fixture::four_forms_history();
    mutate(&chat, visual, |chat, cx| {
        chat.apply_snapshot(history.clone(), None, cx);
    });
    visual.run_until_parked();
    // 展开一行，作为「是否被 reset 清场」的探针
    mutate(&chat, visual, |chat, cx| chat.toggle_thinking(2, cx));
    observe(&chat, visual, |chat, _cx| {
        assert!(chat.thinking_expanded(2));
    });
    // 同一 Rc + 新流式态：历史不 reset，展开记忆保持
    mutate(&chat, visual, |chat, cx| {
        chat.apply_snapshot(history.clone(), Some(fixture::streaming_message(8)), cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(chat.thinking_expanded(2), "同历史快照不应清展开表");
        assert_eq!(chat.row_count(), 7, "历史 6 行 + 流式 1 行");
    });
    // 内容相同的**新** Rc：指针不同 → 整体替换，展开表清场
    mutate(&chat, visual, |chat, cx| {
        chat.apply_snapshot(fixture::four_forms_history(), None, cx);
    });
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert!(!chat.thinking_expanded(2), "换历史应清展开表");
        assert_eq!(chat.row_count(), 6);
    });
}

/// 角色面回归：fixture 装配的角色序列与行分发路由一致（User/Assistant/
/// System 各归各的形态），保证假数据装置本身没装反。
#[gpui_kit::test]
fn fixture_history_roles_match_dispatch(_cx: &mut TestAppContext) {
    let history = fixture::four_forms_history();
    assert_eq!(history[0].role, MessageRole::User);
    assert_eq!(history[1].role, MessageRole::Assistant);
    assert_eq!(history[2].role, MessageRole::Assistant);
    assert_eq!(history[3].role, MessageRole::System);
    assert_eq!(history[4].role, MessageRole::Assistant);
    assert_eq!(history[5].role, MessageRole::User);
}

// ======================================================================
// 6. 正文 Markdown 换装后的选择通路（2.C-3 收口）
// ======================================================================

/// 正文换成 `TextView::markdown + .selectable(true)` 后，跨消息（跨两个
/// TextView）的拖拽选择自然生效——2.C-1 钉住的选择/滚动通路在本批换装下
/// 的收口验证（rendering.md「选择与滚动」：全文可选、跨消息选择）。
#[gpui_kit::test]
fn markdown_bodies_support_cross_view_selection(cx: &mut TestAppContext) {
    use gpui_kit::base::TextSelection;

    let (chat, visual) = chat_window(cx);
    // 两条纯正文 Assistant：正文 TextView 直接可见、不折不滤
    mutate(&chat, visual, |chat, cx| {
        chat.set_history(
            std::rc::Rc::new(vec![
                fixture::assistant_msg("第一条正文阿尔法"),
                fixture::assistant_msg("第二条正文贝塔"),
            ]),
            cx,
        );
    });
    visual.run_until_parked();
    // 两条正文的像素位置（debug_selector 钩子，窗口绝对坐标）
    let first = visual
        .debug_bounds("body-0")
        .expect("第一条正文已渲染且在视口内");
    let second = visual
        .debug_bounds("body-1")
        .expect("第二条正文已渲染且在视口内");
    // 从第一条正文左缘行盒中部拖到第二条正文中心：选区跨过两个 TextView
    let start = point(first.left() + px(2.), first.center().y);
    visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let end = second.center();
    visual.simulate_mouse_move(end, Some(MouseButton::Left), Modifiers::default());
    visual.update(|window, cx| {
        let _ = window.draw(cx);
    });
    visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
    visual.update(|window, cx| {
        let _ = window.draw(cx);
    });
    let selected = visual.update(TextSelection::selected_text);
    assert!(
        selected.contains("第一条"),
        "选区应覆盖第一条正文：{selected:?}"
    );
    assert!(
        selected.contains("第二条"),
        "选区应跨到第二条正文（跨 TextView 选择）：{selected:?}"
    );
}
