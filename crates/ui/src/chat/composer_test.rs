//! Composer 测试（gpui-rewrite 2.10 验收，`#[gpui_kit::test]` 独立文件）。
//!
//! 覆盖：Enter 发送清空 / Shift+Enter 换行不发送 / 空输入不空发 /
//! @ 触发补全弹层 + 点击条目插入 / Enter 接受高亮候选不发送 /
//! 附件入队与移除 + 发送清空 + 附件展开为 Image part。

use gpui_kit::{Entity, TestAppContext, VisualTestContext};

use craftwave_engine::followup::QueuedImageAttachment;
use craftwave_engine::storage::{MessageRole, WirePart};

use crate::MainWindowView;
use crate::theme::{self, CraftwaveTheme};

use super::composer::ComposerState;
use super::state::ConversationState;

/// 会话状态只读断言入口（App 级操作经 VisualTestContext 的窗口绑定
/// update 完成——与 conversation_test.rs 同款写法）。
fn observe<R>(
    chat: &Entity<ConversationState>,
    visual: &mut VisualTestContext,
    f: impl FnOnce(&ConversationState, &gpui_kit::App) -> R,
) -> R {
    visual.update(|_window, cx| f(chat.read(cx), cx))
}

/// Composer 的可变操作入口（composer 实体从会话状态取）。
fn with_composer<R>(
    chat: &Entity<ConversationState>,
    visual: &mut VisualTestContext,
    f: impl FnOnce(&mut ComposerState, &mut gpui_kit::Context<ComposerState>) -> R,
) -> R {
    visual.update(|_window, cx| {
        let composer = chat
            .read(cx)
            .composer()
            .expect("install 已装配 composer")
            .clone();
        composer.update(cx, f)
    })
}

/// Composer 只读断言入口（input 值、弹层状态、附件队列）。
fn composer_snapshot<R>(
    chat: &Entity<ConversationState>,
    visual: &mut VisualTestContext,
    f: impl FnOnce(&ComposerState, &gpui_kit::App) -> R,
) -> R {
    visual.update(|_window, cx| {
        let composer = chat
            .read(cx)
            .composer()
            .expect("install 已装配 composer")
            .clone();
        f(composer.read(cx), cx)
    })
}

/// 聚焦输入框（测试窗口无自动焦点；simulate_input 需要有焦点目标）。
fn focus_input(chat: &Entity<ConversationState>, visual: &mut VisualTestContext) {
    visual.update(|window, cx| {
        let input = chat
            .read(cx)
            .composer()
            .expect("install 已装配 composer")
            .read(cx)
            .input()
            .clone();
        input.update(cx, |state, cx| state.focus(window, cx));
    });
}

/// 测试装配（同 conversation_test.rs::chat_window）：boot → 开窗 → 拿
/// （会话，视觉上下文）。
fn composer_window(
    cx: &mut TestAppContext,
) -> (Entity<ConversationState>, &mut VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        CraftwaveTheme::mount(cx);
        theme::apply(cx);
    });
    let (view, visual) = cx.add_window_view(|window, cx| MainWindowView {
        zoom_seen: 0.0,
        chat: crate::chat::install(window, cx, None),
        sidebar: None,
        palette: None,
    });
    let chat = visual.update(|_window, cx| view.read(cx).chat.clone());
    (chat, visual)
}

// ======================================================================
// 发送：Enter / Shift+Enter / 空输入
// ======================================================================

/// Enter 发送：用户消息追加进会话（正文 + 附件展开为 Image part），
/// 输入框与附件队列清空。
#[gpui_kit::test]
fn enter_sends_and_clears(cx: &mut TestAppContext) {
    let (chat, visual) = composer_window(cx);
    let before = observe(&chat, visual, |chat, _cx| chat.row_count());
    // 入队一张附件再打字发送
    with_composer(&chat, visual, |composer, cx| {
        composer.add_attachment(
            QueuedImageAttachment {
                blob_hash: "attach-1".to_owned(),
                mime_type: "image/png".to_owned(),
            },
            cx,
        );
    });
    focus_input(&chat, visual);
    visual.simulate_input("带图消息");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    composer_snapshot(&chat, visual, |composer, cx| {
        assert_eq!(composer.input().read(cx).value().trim(), "", "发送后输入应清空");
        assert!(composer.attachments().is_empty(), "发送后附件应清空");
    });
    observe(&chat, visual, |chat, _cx| {
        assert_eq!(chat.row_count(), before + 1, "应追加一条消息");
        let last = chat.messages().last().expect("有新消息");
        assert_eq!(last.role, MessageRole::User);
        assert_eq!(last.parts.len(), 2, "正文 + 附件");
        assert!(matches!(&last.parts[0], WirePart::Text { text } if text == "带图消息"));
        assert!(matches!(
            &last.parts[1],
            WirePart::Image { blob_hash, mime_type }
                if blob_hash == "attach-1" && mime_type == "image/png"
        ));
    });
}

/// Shift+Enter：组件内建换行，不产生 PressEnter——不发送、不清空。
#[gpui_kit::test]
fn shift_enter_inserts_newline_without_sending(cx: &mut TestAppContext) {
    let (chat, visual) = composer_window(cx);
    let before = observe(&chat, visual, |chat, _cx| chat.row_count());
    focus_input(&chat, visual);
    visual.simulate_input("第一行");
    visual.simulate_keystrokes("shift-enter");
    visual.run_until_parked();
    composer_snapshot(&chat, visual, |composer, cx| {
        assert_eq!(
            composer.input().read(cx).value(),
            "第一行\n",
            "Shift+Enter 应换行"
        );
    });
    observe(&chat, visual, |chat, _cx| {
        assert_eq!(chat.row_count(), before, "换行不应发送");
    });
}

/// 空输入（与纯空白）Enter 不空发。
#[gpui_kit::test]
fn enter_with_blank_input_does_not_send(cx: &mut TestAppContext) {
    let (chat, visual) = composer_window(cx);
    let before = observe(&chat, visual, |chat, _cx| chat.row_count());
    focus_input(&chat, visual);
    visual.simulate_input("   ");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    observe(&chat, visual, |chat, _cx| {
        assert_eq!(chat.row_count(), before, "空白输入不应发送");
    });
}

// ======================================================================
// @ 补全
// ======================================================================

/// 输入 @ 触发弹层；点击候选条目把路径插回输入框；无命中的查询不开层。
#[gpui_kit::test]
fn at_opens_completion_and_clicking_inserts_path(cx: &mut TestAppContext) {
    let (chat, visual) = composer_window(cx);
    focus_input(&chat, visual);
    // 输入 "@ma"：Change 事件 → 假源过滤出 main.rs
    visual.simulate_input("@ma");
    visual.run_until_parked();
    composer_snapshot(&chat, visual, |composer, _cx| {
        assert!(composer.completion_open(), "输入 @ma 应打开补全弹层");
    });
    // 点击第一条候选（真点击，走 on_click 插入路径）
    let item = visual
        .debug_bounds("completion-item-0")
        .expect("补全候选已渲染");
    visual.simulate_click(item.center(), Default::default());
    visual.run_until_parked();
    composer_snapshot(&chat, visual, |composer, cx| {
        assert!(
            composer
                .input()
                .read(cx)
                .value()
                .contains("@crates/ui/src/main.rs "),
            "接受候选应插入完整路径 + 尾随空格"
        );
        assert!(!composer.completion_open(), "插入后弹层应关闭");
    });
    // 输入没有命中的查询词：候选为空不开层
    focus_input(&chat, visual);
    visual.simulate_input("@zzz-no-match");
    visual.run_until_parked();
    composer_snapshot(&chat, visual, |composer, _cx| {
        assert!(!composer.completion_open(), "无候选不开层");
    });
    // Escape 关层的数据面：close_completion（Escape 的键位接线随 2.C-2）
    with_composer(&chat, visual, |composer, cx| {
        composer.refresh_completion(cx);
    });
    focus_input(&chat, visual);
    visual.simulate_input("@ma");
    visual.run_until_parked();
    with_composer(&chat, visual, |composer, cx| {
        assert!(composer.completion_open());
        composer.close_completion(cx);
        assert!(!composer.completion_open());
    });
}

/// 弹层开着时 Enter 是「接受高亮候选」，不发送消息。
#[gpui_kit::test]
fn enter_accepts_completion_instead_of_sending(cx: &mut TestAppContext) {
    let (chat, visual) = composer_window(cx);
    let before = observe(&chat, visual, |chat, _cx| chat.row_count());
    focus_input(&chat, visual);
    visual.simulate_input("@snapshot");
    visual.run_until_parked();
    composer_snapshot(&chat, visual, |composer, _cx| {
        assert!(composer.completion_open());
        assert_eq!(composer.selected_completion(), 0, "默认高亮第一条");
    });
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    composer_snapshot(&chat, visual, |composer, cx| {
        assert!(
            composer
                .input()
                .read(cx)
                .value()
                .contains("@crates/engine/src/snapshot.rs "),
            "Enter 应接受高亮候选"
        );
        assert!(!composer.completion_open());
    });
    observe(&chat, visual, |chat, _cx| {
        assert_eq!(chat.row_count(), before, "接受候选不应发送");
    });
}

/// at_query 词扫描口径：@ 前扫描、空白断词（含换行）、无 @ 与光标在 @ 前
/// 都不算活查询词。下标按 UTF-8 字节（与输入框 selected_range 同口径）。
#[test]
fn at_query_scans_backwards_from_cursor() {
    use super::composer::at_query;
    assert_eq!(at_query("@ma", 3), Some((0, "ma")));
    // "看 @docs"：看=3 字节，@ 在字节 4，查询词 "docs"
    assert_eq!(at_query("看 @docs", 10), Some((4, "docs")));
    assert_eq!(at_query("@a b", 4), None, "空白断词");
    assert_eq!(at_query("plain", 5), None, "没有 @");
    // 光标紧贴 @ 后：空查询也是活查询（弹层给「全部候选」的前几条）
    assert_eq!(at_query("@ma", 1), Some((0, "")));
    // "a\n@x"：a=0 \n=1 @=2 x=3；光标 4（文末）取最近 @
    assert_eq!(at_query("a\n@x", 4), Some((2, "x")), "跨行取最近 @");
}

// ======================================================================
// 附件队列
// ======================================================================

/// 附件：入队 → 移除一枚 → 发送时剩余清空且不进消息。
#[gpui_kit::test]
fn attachment_queue_add_remove_and_clear_on_send(cx: &mut TestAppContext) {
    let (chat, visual) = composer_window(cx);
    with_composer(&chat, visual, |composer, cx| {
        composer.attach_image_stub(cx);
        composer.attach_image_stub(cx);
    });
    composer_snapshot(&chat, visual, |composer, _cx| {
        assert_eq!(composer.attachments().len(), 2, "按钮入队两枚演示附件");
    });
    with_composer(&chat, visual, |composer, cx| {
        composer.remove_attachment(0, cx);
    });
    composer_snapshot(&chat, visual, |composer, _cx| {
        assert_eq!(composer.attachments().len(), 1);
        assert_eq!(composer.attachments()[0].blob_hash, "demo-hash-2");
    });
    // 发送清空（正文照发，附件不再入消息）
    let before = observe(&chat, visual, |chat, _cx| chat.row_count());
    focus_input(&chat, visual);
    visual.simulate_input("无图消息");
    visual.simulate_keystrokes("enter");
    visual.run_until_parked();
    composer_snapshot(&chat, visual, |composer, _cx| {
        assert!(composer.attachments().is_empty(), "发送后附件清空");
    });
    observe(&chat, visual, |chat, _cx| {
        let last = chat.messages().last().expect("有新消息");
        // 剩下的那枚附件（demo-hash-2）随消息发出；移除掉的不在
        assert_eq!(last.parts.len(), 2, "正文 + 剩余附件");
        assert!(matches!(
            &last.parts[1],
            WirePart::Image { blob_hash, .. } if blob_hash == "demo-hash-2"
        ));
        assert_eq!(chat.row_count(), before + 1);
    });
}
