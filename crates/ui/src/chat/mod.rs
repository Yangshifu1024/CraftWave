//! 对话流（gpui-rewrite 阶段 2.C-1 底座 + 2.C-2b 帧流接线）：消息流底座 +
//! Composer + 引擎会话服务 + 假数据装置。
//!
//! 四个部分：[`ConversationState`] 是会话状态实体（历史
//! `Rc<Vec<Rc<MessageEntry>>>` + 可选流式末条 + 展开状态两张表 +
//! ListState），同时是中栏视图——`apply_snapshot` 是唯一写入口；
//! [`ComposerState`] 负责输入 + @ 补全（2.C-2b 起异步真源）+ 附件 +
//! 停止钮；[`SessionService`]（service.rs）是引擎会话服务——发送链 /
//! 帧流管道（tokio 桥 → 泵 → 聚合器 → 64ms 节流 → apply_snapshot）/
//! 取消 / 续流的 owner；`fixture` 是测试装置工厂（回声条目 + 假补全源，
//! 生产装配不用）。
//!
//! ## 装配入口 [`install`]
//!
//! 在开窗闭包里调用（TextareaState 与 subscribe_in 都需要 `&mut Window`）。
//! `service` 参数二选一：
//! - `Some(service)`（生产）：真通道——发送走服务的 start_chat 装配链，
//!   快照由帧流驱动，历史从空开始；
//! - `None`（测试装置）：回声模式——发送直接追加一条演示条目，装载演示
//!   历史（2.C-1 行为原样保留，既有 UI 测试零退化）。

mod composer;
mod fixture;
mod markdown_links;
mod row;
mod service;
mod state;

#[cfg(test)]
#[path = "conversation_test.rs"]
mod conversation_test;

#[cfg(test)]
#[path = "markdown_links_test.rs"]
mod markdown_links_test;

#[cfg(test)]
#[path = "composer_test.rs"]
mod composer_test;

#[cfg(test)]
#[path = "service_test.rs"]
mod service_test;

use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{App, Entity, Global, Window, div, px};

use crate::ui_zoom;

pub use composer::ComposerState;
pub use fixture::FakeCompletionSource;
pub use service::{EngineCompletionSource, SessionService};
pub use state::ConversationState;

/// 引擎会话服务句柄（Global 强引用 = 服务寿命与 app 一致）：Composer
/// 停止钮 / 补全源 / 发送分流都从全局取（chat 域的服务，句柄住同模块）。
pub struct EngineService(pub Entity<SessionService>);

impl Global for EngineService {}

/// 中栏装配：建会话实体 →（真通道）建 / 复用会话服务并登记新视图 →
/// 建 Composer（窗口闭包内）→ 订阅输入事件（Enter 发送 / @ 补全刷新）。
/// 返回挂进中栏的实体。
///
/// `engine` 参数二选一：
/// - `Some(bridge)`（生产）：真通道——发送走服务的 start_chat 装配链，
///   快照由帧流驱动，历史从空开始；服务实体挂 `EngineService` Global
///   （应用级服务，窗口重开时复用并登记新视图）；
/// - `None`（测试装置）：回声模式——发送直接追加一条演示条目，装载演示
///   历史（2.C-1 行为原样保留，既有 UI 测试零退化）。
pub fn install(
    window: &mut Window,
    cx: &mut App,
    engine: Option<&crate::engine::EngineBridge>,
) -> Entity<ConversationState> {
    let conversation = cx.new(|_| ConversationState::new());
    // 真通道：服务实体（Global 已有则复用——窗口重开场景）登记新视图；
    // 回声：演示历史
    let service: Option<Entity<SessionService>> = engine.map(|bridge| {
        if let Some(existing) = cx.try_global::<EngineService>() {
            let service = existing.0.clone();
            let weak = conversation.downgrade();
            service.update(cx, |service, _| service.set_conversation(weak));
            service
        } else {
            let runtime = crate::engine::runtime(cx);
            let weak = conversation.downgrade();
            let service = cx.new(|_| {
                SessionService::new(bridge.state.clone(), bridge.events.clone(), runtime, weak)
            });
            cx.set_global(EngineService(service.clone()));
            service
        }
    });
    conversation.update(cx, |conversation, cx| {
        // 真通道：空历史（内容由服务快照驱动）；回声：演示历史
        let history: Rc<Vec<Rc<craftwave_engine::storage::MessageEntry>>> = if service.is_some()
        {
            Rc::new(Vec::new())
        } else {
            fixture::demo_history()
        };
        conversation.apply_snapshot(history, None, cx);
        conversation.service = service.clone();
    });
    // Composer：输入框在窗口闭包里建（TextareaState::new 要 &mut Window）；
    // 补全源：真通道 = engine 搜索（后台线程池），回声 = fixture 假源
    let source: Rc<dyn composer::CompletionSource> = match &service {
        Some(service) => Rc::new(EngineCompletionSource::new(service.downgrade())),
        None => Rc::new(FakeCompletionSource::default()),
    };
    let composer = cx.new(|cx| {
        ComposerState::new(source, window, cx)
    });
    if let Some(service) = &service {
        // 停止钮的取消目标（弱引用）
        composer.update(cx, |composer, cx| {
            composer.set_cancel_target(service.downgrade(), cx);
        });
    }
    let input = composer.read(cx).input().clone();
    conversation.update(cx, |conversation, cx| {
        conversation.composer = Some(composer);
        // 五参闭包（spike 模式）：回调里拿得到 window，set_value 才能清空输入
        let subscription = cx.subscribe_in(
            &input,
            window,
            |conversation: &mut ConversationState,
             _input: &Entity<TextareaState>,
             event: &InputEvent,
             window: &mut Window,
             cx: &mut Context<ConversationState>| {
                // Enter 分流：弹层关着的普通 Enter 是「发送」；其余事件
                // 全部交给 composer 处理
                let send_now = matches!(event, InputEvent::PressEnter { shift: false, .. })
                    && conversation
                        .composer
                        .as_ref()
                        .is_some_and(|composer| !composer.read(cx).completion_open());
                if send_now {
                    let Some((text, attachments)) = conversation
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
                    match &conversation.service {
                        // 真通道：服务的发送链要回写本会话实体
                        // （apply_snapshot），而当前正处于会话实体的
                        // update 栈内——gpui 禁止嵌套 update 同一实体，
                        // 用 App::defer 推迟到本轮 update 结束后执行
                        Some(service) => {
                            let service = service.clone();
                            cx.defer(move |cx| {
                                service.update(cx, |service, cx| {
                                    service.send_message(text, attachments, cx);
                                });
                            });
                        }
                        // 回声模式（测试装置）：条目重建后直接追加
                        None => {
                            conversation.append_message(fixture::echo_entry(&text, attachments), cx);
                        }
                    }
                    return;
                }
                if let Some(composer) = &conversation.composer {
                    composer.update(cx, |composer, cx| {
                        composer::on_input_event(composer, event, window, cx)
                    });
                }
            },
        );
        // 订阅随 owner（本实体）存活：存进 _subscriptions 保活，不用
        // detach（窗口重开重建实体时旧回调随旧实体一起释放，不累积）
        conversation._subscriptions.push(subscription);
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
