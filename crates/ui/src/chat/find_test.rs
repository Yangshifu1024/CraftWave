//! 会话内查找测试（gpui-rewrite 3.8 验收，`#[gpui_kit::test]` 独立文件）。
//!
//! 六组：
//! 1. **纯函数面**：`find_ranges` 大小写不敏感/CJK/多命中/字节边界；
//!    `row_search_text` 覆盖正文/工具参数/思考文本、排除图片；
//! 2. **扫描**：命中行序 + 过滤行跳过（纪律二：源数据在但渲染被滤，
//!    查找不落空行）；
//! 3. **开合**：⌘F 打开（查找条入树）、Escape 关闭（命中/闪示清空）；
//! 4. **命中计数与导航循环**：n/m 计数、next/prev 循环、跳转行级定位
//!    （`logical_scroll_top` 对账）；
//! 5. **高亮存在性（渲染帧断言）**：查找条与闪示行的 debug_selector
//!    在帧上可查；TextView 行淡底降级路径（正文命中行挂 find 槽位）；
//! 6. **换历史清场**：set_history 后查找态整体作废（行号映射换了）。

use std::rc::Rc;

use gpui_kit::{Context, Entity, TestAppContext, VisualTestContext};

use craftwave_engine::storage::{MessageEntry, MessageRole, WirePart};

use crate::MainWindowView;
use crate::theme::{self, CraftwaveTheme};

use super::fixture;
use super::state::ConversationState;

use super::find::{find_ranges, row_search_text, run_find};

// ======================================================================
// 装置
// ======================================================================

/// 会话状态的可变操作入口（conversation_test 同款）。
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

/// 测试窗口：echo 装置（无引擎桥）+ 演示历史，随后按用例 set_history。
fn find_window(cx: &mut TestAppContext) -> (Entity<ConversationState>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        CraftwaveTheme::mount(cx);
        theme::apply(cx);
    });
    let holder: Rc<std::cell::RefCell<Option<Entity<ConversationState>>>> =
        Rc::new(std::cell::RefCell::new(None));
    let slot = holder.clone();
    let (_root, visual) = cx.add_window_view(|window, cx| {
        use gpui_kit::AppContext as _;
        let chat = crate::chat::install(window, cx, None);
        *slot.borrow_mut() = Some(chat.clone());
        let inner = cx.new(|_| MainWindowView {
            zoom_seen: 0.0,
            chat,
            sidebar: None,
            palette: None,
        });
        gpui_kit::base::Root::new(inner, window, cx)
    });
    let chat = holder.borrow().clone().expect("install 在开窗闭包内执行");
    visual.run_until_parked();
    (chat, visual)
}

/// open_find 需要 window（InputState::new）；mutate 的闭包拿不到——
/// 独立小入口直接从 visual.update 拿 window。
fn open_find(chat: &Entity<ConversationState>, visual: &mut VisualTestContext) {
    visual
        .update(|window, cx| chat.update(cx, |chat, cx| super::find::open_find(chat, window, cx)));
}

fn close_via_escape(chat: &Entity<ConversationState>, visual: &mut VisualTestContext) {
    visual
        .update(|window, cx| chat.update(cx, |chat, cx| super::find::on_escape(chat, window, cx)));
}

// ======================================================================
// 1. 纯函数面
// ======================================================================

/// find_ranges：大小写不敏感 + 全部命中 + 字节区间落在字符边界
/// （CJK 3 字节/字符的场景）。
#[test]
fn find_ranges_matches_case_insensitive_and_cjk() {
    let ranges = find_ranges("Error occurred; expect ERROR soon", "error");
    assert_eq!(ranges, vec![0..5, 23..28]);
    // CJK：无大小写可言，但字节区间必须按 3 字节步进
    let cjk = find_ranges("构建脚本在这里，再看构建两字", "构建");
    assert_eq!(cjk, vec![0..6, 30..36]);
    // 空查询与无命中
    assert!(find_ranges("anything", "").is_empty());
    assert!(find_ranges("", "x").is_empty());
    assert!(find_ranges("abc", "abcd").is_empty());
    // 重复命中不遗漏（相邻重叠不算——非重叠子串扫描）
    assert_eq!(find_ranges("abab", "ab"), vec![0..2, 2..4]);
}

/// row_search_text：正文 + 工具参数 + 思考文本 + 结果摘要在场，图片排除。
#[test]
fn row_search_text_covers_body_args_and_thinking() {
    let entry = MessageEntry {
        id: "m1".into(),
        parent_id: None,
        role: MessageRole::Assistant,
        parts: vec![
            WirePart::Reasoning {
                text: "thinking text".into(),
            },
            WirePart::Text {
                text: "body text".into(),
            },
            WirePart::ToolCall {
                call_id: "c1".into(),
                name: "grep".into(),
                args_json: r#"{"pattern":"NEEDLE"}"#.into(),
            },
            WirePart::ToolResult {
                call_id: "c1".into(),
                ok: true,
                summary: "result summary".into(),
            },
            WirePart::Image {
                blob_hash: "h".into(),
                mime_type: "image/png".into(),
            },
        ],
        created_at: 0,
        run_id: None,
        status: None,
    };
    let text = row_search_text(&entry);
    assert!(text.contains("body text"));
    assert!(text.contains("thinking text"));
    assert!(text.contains(r#""pattern":"NEEDLE""#));
    assert!(text.contains("result summary"));
    // 跨段不误拼：相邻段尾首字符不成词（换行分隔）
    assert!(text.contains('\n'));
}

// ======================================================================
// 2. 扫描：命中行序 + 过滤行跳过
// ======================================================================

/// 扫描走活动路径历史：日志行（Assistant 只有 thinking）被跳过——
/// 源数据在但渲染为零高空行，查找不得落进不可见行。
#[gpui_kit::test]
fn scan_skips_filtered_rows_and_hits_expected_rows(cx: &mut TestAppContext) {
    let (chat, visual) = find_window(cx);
    mutate(&chat, visual, |chat, cx| {
        let history = Rc::new(vec![
            fixture::assistant_msg("first alpha hit"),
            fixture::log_only_assistant("alpha in filtered row"),
            fixture::user_msg("second ALPHA hit"),
            fixture::assistant_tool_only(), // 工具参数里有 needle 才命中
        ]);
        chat.set_history(history, cx);
    });
    mutate(&chat, visual, |chat, _cx| {
        let hits = super::find::scan_hits(chat.messages(), "alpha");
        assert_eq!(hits, vec![0, 2], "过滤行 1 不参与，命中行升序");
    });
}

// ======================================================================
// 3. 开合
// ======================================================================

/// ⌘F 打开：查找条入树；Escape 关闭：命中与闪示清空、焦点交还 composer。
#[gpui_kit::test]
fn open_and_escape_close_find(cx: &mut TestAppContext) {
    let (chat, visual) = find_window(cx);
    open_find(&chat, visual);
    visual.run_until_parked();
    assert!(visual.debug_bounds("find-bar").is_some(), "查找条应已入树");
    // 关闭
    close_via_escape(&chat, visual);
    visual.run_until_parked();
    assert!(visual.debug_bounds("find-bar").is_none(), "Escape 后查找条离场");
    observe(&chat, visual, |chat, _| {
        assert!(chat.find.is_none());
        assert!(chat.find_flash.is_none());
    });
}

/// 换历史（会话切换 / 落盘重载共用 set_history）清空查找态：行号映射
/// 换了，旧命中只会误导导航。
#[gpui_kit::test]
fn history_replacement_clears_find(cx: &mut TestAppContext) {
    let (chat, visual) = find_window(cx);
    mutate(&chat, visual, |chat, cx| {
        chat.set_history(
            Rc::new(vec![fixture::assistant_msg("alpha page one")]),
            cx,
        );
    });
    open_find(&chat, visual);
    set_query_via_window(&chat, visual, "alpha");
    observe(&chat, visual, |chat, _| {
        assert!(!chat.find.as_ref().expect("已开").hits.is_empty());
    });
    mutate(&chat, visual, |chat, cx| {
        chat.set_history(Rc::new(vec![fixture::assistant_msg("fresh page")]), cx);
    });
    observe(&chat, visual, |chat, _| {
        assert!(chat.find.is_none(), "换历史应关闭查找");
        assert!(chat.find_flash.is_none());
    });
}

// ======================================================================
// 4. 命中计数与导航循环
// ======================================================================

/// n/m 计数 + next/prev 循环 + Item 级定位（logical_scroll_top 对账）。
#[gpui_kit::test]
fn navigation_cycles_and_counts(cx: &mut TestAppContext) {
    let (chat, visual) = find_window(cx);
    // 长历史让列表真正可滚（内容超出测试视口——Bottom 对齐下列表装得下
    // 全部内容时会收掉滚动锚点，「滚到行 0」无从谈起，锚点断言需真溢出）
    mutate(&chat, visual, |chat, cx| {
        let filler = "撑起滚动距离的正文行。".repeat(30);
        let mut history = vec![fixture::assistant_msg("alpha one")];
        for i in 0..14 {
            history.push(fixture::assistant_msg(&format!("filler {i} {filler}")));
        }
        history.push(fixture::assistant_msg("alpha two"));
        chat.set_history(Rc::new(history), cx);
    });
    open_find(&chat, visual);
    set_query_via_window(&chat, visual, "ALPHA");
    visual.update(|_, cx| eprintln!("after set_query: {:?}", chat.read(cx).list.logical_scroll_top()));
    visual.run_until_parked();
    visual.update(|_, cx| eprintln!("after parked: {:?}", chat.read(cx).list.logical_scroll_top()));
    observe(&chat, visual, |chat, _| {
        let find = chat.find.as_ref().expect("已开");
        assert_eq!(find.hits, vec![0, 15], "大小写不敏感命中");
        assert_eq!(find.current, 0, "首查落第一处");
    });
    // 闪示与滚动定位在第一处（行 0 落视口顶）
    observe(&chat, visual, |chat, _| {
        assert_eq!(chat.find_flash, Some(0));
        assert_eq!(
            chat.list.logical_scroll_top().item_ix,
            0,
            "Item 级定位（锚点落行 0）"
        );
    });
    // next → 行 15；再 next → 循环回行 0
    visual.update(|_window, cx| chat.update(cx, super::find::find_next));
    observe(&chat, visual, |chat, _| {
        let find = chat.find.as_ref().expect("已开");
        assert_eq!((find.current, find.hits[find.current]), (1, 15));
        assert_eq!(chat.find_flash, Some(15));
    });
    visual.update(|_window, cx| chat.update(cx, super::find::find_next));
    observe(&chat, visual, |chat, _| {
        let find = chat.find.as_ref().expect("已开");
        assert_eq!(find.current, 0, "循环回第一个");
    });
    // prev 从第一个循环回最后一个
    visual.update(|_window, cx| chat.update(cx, super::find::find_prev));
    observe(&chat, visual, |chat, _| {
        let find = chat.find.as_ref().expect("已开");
        assert_eq!(find.current, 1);
    });
}

// ======================================================================
// 5. 高亮存在性（渲染帧断言）
// ======================================================================

/// 命中高亮落在帧上：当前命中行有闪示条（find-flash-<row>）；TextView
/// 正文命中行（非当前）有行级淡底（降级形态——find 槽位驱动，帧上以
/// flash 条与行底两条路径分别可见性验证：闪示条是显式 selector，行底
/// 由 row_is_hit 的状态面断言）。
#[gpui_kit::test]
fn hit_highlights_render_on_frame(cx: &mut TestAppContext) {
    let (chat, visual) = find_window(cx);
    mutate(&chat, visual, |chat, cx| {
        let history = Rc::new(vec![
            fixture::assistant_msg("alpha one"),
            fixture::user_msg("alpha two"),
        ]);
        chat.set_history(history, cx);
    });
    open_find(&chat, visual);
    set_query_via_window(&chat, visual, "alpha");
    visual.run_until_parked();
    assert!(
        visual.debug_bounds("find-flash-0").is_some(),
        "当前命中行应有闪示条"
    );
    // 导航到行 1（user 气泡行）：闪示随行
    visual.update(|_window, cx| chat.update(cx, super::find::find_next));
    visual.run_until_parked();
    assert!(visual.debug_bounds("find-flash-1").is_some());
    assert!(visual.debug_bounds("find-flash-0").is_none(), "旧落点闪示离场");
    observe(&chat, visual, |chat, _| {
        assert!(
            super::find::row_is_hit(chat, 0),
            "非当前命中行保持行级淡底判定（TextView 行降级路径）"
        );
    });
}

/// reduce_motion：闪示静态呈现（元素在场）且不派清理任务。
#[gpui_kit::test]
fn reduce_motion_keeps_static_flash(cx: &mut TestAppContext) {
    let (chat, visual) = find_window(cx);
    visual.update(|_, cx| cx.set_reduce_motion(true));
    mutate(&chat, visual, |chat, cx| {
        chat.set_history(
            Rc::new(vec![fixture::assistant_msg("alpha one")]),
            cx,
        );
    });
    open_find(&chat, visual);
    set_query_via_window(&chat, visual, "alpha");
    visual.run_until_parked();
    assert!(visual.debug_bounds("find-flash-0").is_some(), "静态高亮在场");
    observe(&chat, visual, |chat, _| {
        assert!(
            chat.find_flash_task.is_none(),
            "reduce_motion 不派淡出清理任务"
        );
    });
}

// ======================================================================
// 装置补充：带窗口的查询词注入（set_value 要 &mut Window）
// ======================================================================

fn set_query_via_window(
    chat: &Entity<ConversationState>,
    visual: &mut VisualTestContext,
    query: &str,
) {
    visual.update(|window, cx| {
        chat.update(cx, |chat, cx| {
            let find = chat.find.as_ref().expect("查找条已开").input.clone();
            find.update(cx, |input, cx| input.set_value(query.to_owned(), window, cx));
            run_find(chat, cx);
            eprintln!("inside update after run_find: {:?}", chat.list.logical_scroll_top());
        });
    });
}
