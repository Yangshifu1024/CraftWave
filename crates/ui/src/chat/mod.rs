//! 对话流（gpui-rewrite 阶段 2.C-1）：消息流底座 + Composer + 假数据装置。
//!
//! 本批把 main.rs 三栏的中栏占位换成真消息流（spec 2.4 / 2.10 / 2.12）。
//! 四个部分：[`ConversationState`] 是会话状态实体（历史
//! `Rc<Vec<Rc<MessageEntry>>>` + 可选流式末条 + 展开状态两张表 +
//! ListState），同时是中栏视图——gpui 的 list 元素长在它的元素树里，
//! `cx.notify` 必须落在它身上才能重排行（spike 单实体模式）；
//! [`ComposerState`] 负责输入 + @ 补全 + 附件（发送本批是回声式，真
//! start_chat 在 2.C-2）；`fixture` 是假数据工厂 + @ 补全 stub 源，测试
//! 与演示装配共用；`row` 是行分发四形态的元素构建。
//!
//! 真帧流管道（engine start_chat → FrameAggregator → 64ms 快照 →
//! [`ConversationState::apply_snapshot`]）是 2.C-2；本批全部由假数据驱动，
//! 但写入路径已经收敛到 apply_snapshot / append_message / set_streaming
//! 三个口，届时只是换数据来源。
//!
//! 装配入口 [`install`]：在开窗闭包里调用（TextareaState 与 subscribe_in
//! 都需要 `&mut Window`），返回挂进中栏的会话实体。

mod composer;
mod fixture;
mod row;
mod state;

#[cfg(test)]
#[path = "conversation_test.rs"]
mod conversation_test;

#[cfg(test)]
#[path = "composer_test.rs"]
mod composer_test;

use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{App, Entity, Window, div, px};

use crate::ui_zoom;

pub use composer::ComposerState;
pub use fixture::FakeCompletionSource;
pub use state::ConversationState;

/// 中栏装配：建会话实体 → 装演示历史 → 建 Composer（窗口闭包内）→
/// 订阅输入事件（Enter 发送 / @ 补全刷新）。返回挂进中栏的实体。
///
/// 演示历史是 2.12 假数据装置的一部分（真会话历史装载随 3.1 多会话切片；
/// @ 补全与附件的真引擎接线在 2.C-2）。
pub fn install(window: &mut Window, cx: &mut App) -> Entity<ConversationState> {
    let conversation = cx.new(|_| ConversationState::new());
    conversation.update(cx, |conversation, cx| {
        let history = fixture::demo_history();
        conversation.apply_snapshot(history, None, cx);
    });
    // Composer：输入框在窗口闭包里建（TextareaState::new 要 &mut Window）；
    // 补全源本批用假 stub（真实现接 engine search_workspace_paths 在 2.C-2）
    let composer = cx.new(|cx| {
        ComposerState::new(Rc::new(FakeCompletionSource::default()), window, cx)
    });
    let input = composer.read(cx).input().clone();
    conversation.update(cx, |conversation, cx| {
        conversation.composer = Some(composer);
        // 五参闭包（spike 模式）：回调里拿得到 window，set_value 才能清空输入
        cx.subscribe_in(
            &input,
            window,
            |conversation: &mut ConversationState,
             _input: &Entity<TextareaState>,
             event: &InputEvent,
             window: &mut Window,
             cx: &mut Context<ConversationState>| {
                // Enter 分流：弹层关着的普通 Enter 是「发送」——在会话自己
                // 的 Context 里追加（gpui 禁止嵌套 update 同一实体，composer
                // 只产出待发消息）；其余事件全部交给 composer 处理
                let send_now = matches!(event, InputEvent::PressEnter { shift: false, .. })
                    && conversation
                        .composer
                        .as_ref()
                        .is_some_and(|composer| !composer.read(cx).completion_open());
                if send_now {
                    let Some(entry) = conversation
                        .composer
                        .as_ref()
                        .and_then(|composer| {
                            composer.update(cx, |composer, cx| {
                                composer.take_outgoing(window, cx)
                            })
                        })
                    else {
                        return; // 空白输入：不空发
                    };
                    conversation.append_message(entry, cx);
                    return;
                }
                if let Some(composer) = &conversation.composer {
                    composer.update(cx, |composer, cx| {
                        composer::on_input_event(composer, event, window, cx)
                    });
                }
            },
        )
        .detach();
    });
    conversation
}

/// 中栏视图（Render on ConversationState）：消息列表（flex_1，list 容器
/// 必须 size_full——高度 0 坑）+ Composer（flex_shrink_0）。
impl Render for ConversationState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 缩放换档随动（spec 2.3 钩子）：字号档变了，行高缓存整体失效重测
        if ui_zoom::changed(&mut self.zoom_seen, cx) {
            self.remeasure_all(cx);
        }
        let list_state = self.list.clone();
        let entity = cx.entity();
        // 消息流：不等高惰性测量（Bottom 对齐 + 512 overscan），Tail 跟随在
        // ListState 内建（新消息贴底 / 上滚暂停 / 回底续跟）
        let mut column = div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div().flex_1().min_h_0().child(
                    gpui_kit::list(list_state, move |ix, _window, cx| {
                        entity.update(cx, |conversation, cx| conversation.render_row(ix, cx))
                    })
                    .size_full(),
                ),
            );
        if let Some(composer) = &self.composer {
            column = column.child(
                div()
                    .flex_shrink_0()
                    .px(px(crate::theme::SPACE_5))
                    .pb(px(crate::theme::SPACE_5))
                    .child(composer.clone()),
            );
        }
        column
    }
}
