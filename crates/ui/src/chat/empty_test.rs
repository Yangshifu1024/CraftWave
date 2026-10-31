//! B1 空状态测试（阶段 3-A，`#[gpui_kit::test]` 独立文件）。
//!
//! 时段问候的分档判定在 `text_util_test.rs`（greeting_from 纯函数）；
//! 这里钉中栏空态的状态机面：
//! 1. **空态判定**：无历史 + 无流式行 = 空态（问候语 + 建议 chips 的
//!    显示面）；有消息或流式行时退出；
//! 2. **建议 chips**：点击把文案填入 composer（不自动发送——发送由
//!    Enter 决定，与旧栈「点击填入 composer」语义一致）；
//! 3. **编辑重发提示条的显隐**（3.3 交互面在空态之外的补钉）：
//!    begin_resend 打开 / cancel_resend 收场 / 发送分流的待发态消费。

use gpui_kit::{AppContext as _, TestAppContext};

use crate::theme::{self, CraftwaveTheme};

use super::state::ConversationState;

/// 回声测试装置窗口（conversation_test 同款：install(None) 带 composer）。
fn echo_window(cx: &mut TestAppContext) -> (gpui_kit::Entity<ConversationState>, &mut gpui_kit::VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        CraftwaveTheme::mount(cx);
        theme::apply(cx);
    });
    let holder: std::rc::Rc<std::cell::RefCell<Option<gpui_kit::Entity<ConversationState>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = holder.clone();
    let (_root, visual) = cx.add_window_view(|window, cx| {
        let chat = crate::chat::install(window, cx, None);
        *slot.borrow_mut() = Some(chat.clone());
        let inner = cx.new(|_| crate::MainWindowView {
            zoom_seen: 0.0,
            chat,
            sidebar: None,
            palette: None,
        });
        gpui_kit::base::Root::new(inner, window, cx)
    });
    visual.run_until_parked();
    let chat = holder.borrow().clone().expect("install 在开窗闭包内执行");
    (chat, visual)
}

/// 空态判定：0 行即空态；装载历史后退出、清空后回到空态。
/// （回声装置窗口自带演示历史——先清空再断言纯判定面。）
#[gpui_kit::test]
fn empty_view_tracks_row_count(cx: &mut TestAppContext) {
    let (chat, visual) = echo_window(cx);
    visual.update(|_window, cx| {
        chat.update(cx, |chat, cx| {
            chat.apply_snapshot(std::rc::Rc::new(Vec::new()), None, cx);
        });
        assert!(chat.read(cx).is_empty_view(), "空历史应是空态");
        // 装载一条历史 → 退出空态
        chat.update(cx, |chat, cx| {
            chat.apply_snapshot(
                std::rc::Rc::new(vec![crate::chat::fixture::user_msg("你好")]),
                None,
                cx,
            );
        });
        assert!(!chat.read(cx).is_empty_view());
        // 清空历史回空态
        chat.update(cx, |chat, cx| {
            chat.apply_snapshot(std::rc::Rc::new(Vec::new()), None, cx);
        });
        assert!(chat.read(cx).is_empty_view());
    });
}

/// 建议 chips 点击填 composer：文案入输入框、不自动发送、附件不受影响。
#[gpui_kit::test]
fn suggestion_chip_fills_composer_without_sending(cx: &mut TestAppContext) {
    let (chat, visual) = echo_window(cx);
    // 回声装置自带演示历史：先清空（空态是 chips 的显示前提）
    visual.update(|_window, cx| {
        chat.update(cx, |chat, cx| {
            chat.apply_snapshot(std::rc::Rc::new(Vec::new()), None, cx);
        });
    });
    visual.update(|window, cx| {
        chat.update(cx, |chat, cx| {
            chat.fill_suggestion("帮我写一个构建脚本", window, cx);
        });
    });
    visual.run_until_parked();
    visual.update(|_window, cx| {
        let composer = chat.read(cx).composer().expect("composer 已装配");
        let value = composer.read(cx).input().read(cx).value().to_owned();
        assert_eq!(value, "帮我写一个构建脚本", "chip 文案应填入输入框");
        // 未自动发送：消息流仍为空（空态保持，等待用户 Enter）
        assert!(chat.read(cx).is_empty_view());
    });
}

/// 编辑重发待发态的开关面（3.3 提示条的显示判定）。
#[gpui_kit::test]
fn resend_pending_toggles_and_clears_on_history_swap(cx: &mut TestAppContext) {
    let (chat, visual) = echo_window(cx);
    visual.update(|_window, cx| {
        chat.update(cx, |chat, cx| {
            chat.apply_snapshot(
                std::rc::Rc::new(vec![crate::chat::fixture::user_msg("原始问题")]),
                None,
                cx,
            );
        });
        let entry = chat.read(cx).messages()[0].as_ref().clone();
        chat.update(cx, |chat, cx| chat.begin_resend(&entry, cx));
        assert!(chat.read(cx).resend_pending());
        // 手动取消
        chat.update(cx, |chat, cx| chat.cancel_resend(cx));
        assert!(!chat.read(cx).resend_pending());
        // 再次发起后换历史（切换会话）→ 待发态随旧行作废（set_history 清）
        let entry = chat.read(cx).messages()[0].as_ref().clone();
        chat.update(cx, |chat, cx| chat.begin_resend(&entry, cx));
        assert!(chat.read(cx).resend_pending());
        chat.update(cx, |chat, cx| {
            chat.apply_snapshot(std::rc::Rc::new(Vec::new()), None, cx);
        });
        assert!(
            !chat.read(cx).resend_pending(),
            "换历史应作废未发出的编辑重发"
        );
    });
}
