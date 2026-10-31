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

mod ask;
mod composer;
mod fixture;
mod images;
mod markdown_links;
mod row;
mod service;
mod state;
mod subagent;

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

#[cfg(test)]
#[path = "parts_test.rs"]
mod parts_test;

#[cfg(test)]
#[path = "tree_test.rs"]
mod tree_test;

#[cfg(test)]
#[path = "empty_test.rs"]
mod empty_test;

#[cfg(test)]
#[path = "ask_test.rs"]
mod ask_test;

#[cfg(test)]
#[path = "subagent_test.rs"]
mod subagent_test;

use std::rc::Rc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{App, Entity, FontWeight, Global, SharedString, Window, div, px};

use crate::i18n::t;
use crate::text_util::greeting;
use crate::theme::ActiveCraftwave as _;
use crate::ui_zoom;

pub use composer::ComposerState;
pub use fixture::FakeCompletionSource;
pub use service::{EngineCompletionSource, SessionService};
pub use state::ConversationState;

/// 引擎会话服务句柄（Global 强引用 = 服务寿命与 app 一致）：Composer
/// 停止钮 / 补全源 / 发送分流都从全局取（chat 域的服务，句柄住同模块）。
pub struct EngineService(pub Entity<SessionService>);

impl Global for EngineService {}

// 会话权限档快切 action（3.4）：Shift+Tab 循环（键位在 main.rs 绑定，
// 派发沿 focus path 冒泡到 ConversationState 处理——与 composer 徽标
// 点击同一条 cycle_tier 路，一个逻辑命令只建模一次）
gpui_kit::actions!(craftwave, [CyclePermissionTier]);

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
        // 图片装载面（2.9）：真通道接引擎存储装载器（会话 imgblob 读盘走
        // 后台线程池）；回声装置无装载面（图片按不可预览卡渲染，不假装有图）
        if let (Some(bridge), Some(service)) = (engine, service.as_ref()) {
            conversation.set_image_loader(std::rc::Rc::new(
                images::EngineImageLoader::new(bridge.state.clone(), service.downgrade()),
            ));
        }
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
    // 附件预览的图片装载面（2.9）：与消息流同一装载器（同款缩略图三态）；
    // 回声装置不装（stub 附件无 blob 可读，按不可预览卡渲染）
    if let (Some(bridge), Some(service)) = (engine, service.as_ref()) {
        let loader: std::rc::Rc<dyn images::ImageLoader> = std::rc::Rc::new(
            images::EngineImageLoader::new(bridge.state.clone(), service.downgrade()),
        );
        composer.update(cx, |composer, cx| composer.set_image_loader(loader, cx));
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
                    // 编辑重发待发态（3.3）：本次发送走 branch_resend 挂到
                    // 目标 parent 下产生新分支，随后清除待发态（一次一分支）
                    let resend_parent = conversation.resend_parent.take();
                    // 权限档快切（3.4）：发送配置从 composer 快切档取
                    let tier = conversation
                        .composer
                        .as_ref()
                        .map(|composer| composer.read(cx).tier())
                        .unwrap_or(craftwave_engine::safety::tier::PermissionTier::AutoEdit);
                    match &conversation.service {
                        // 真通道：服务的发送链要回写本会话实体
                        // （apply_snapshot），而当前正处于会话实体的
                        // update 栈内——gpui 禁止嵌套 update 同一实体，
                        // 用 App::defer 推迟到本轮 update 结束后执行
                        Some(service) => {
                            let service = service.clone();
                            cx.defer(move |cx| {
                                service.update(cx, |service, cx| match resend_parent {
                                    Some(parent) => {
                                        service.branch_resend(parent, text, attachments, cx);
                                    }
                                    None => {
                                        service.send_message(text, attachments, tier, cx);
                                    }
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
/// 必须 size_full——高度 0 坑）+ Composer（flex_shrink_0）+ zoom 覆盖层。
impl Render for ConversationState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 缩放换档随动（spec 2.3 钩子）：字号档变了，行高缓存整体失效重测
        if ui_zoom::changed(&mut self.zoom_seen, cx) {
            self.remeasure_all(cx);
        }
        // 阅读区实际宽度（工具簇参数摘要「宽度自适应格数」的反算基准，
        // Wake 7102-7104 手法）：视口减三栏骨架占用，封顶阅读轴。本帧内
        // 被 list 行回调读取（行渲染发生在本方法的元素布局阶段）；纯字段
        // 写入不 notify（宽度变化由窗口重排自然触发重渲，不构成反馈环）
        let rails = crate::SIDEBAR_PLACEHOLDER_WIDTH + crate::PANEL_PLACEHOLDER_WIDTH + 2.0;
        self.reader_width_seen = (window.viewport_size().width.as_f32() - rails)
            .clamp(220.0, crate::chat::state::READER_MAX_WIDTH);
        // zoom 自适应的可用视口（同帧 zoom 覆盖层消费；render_image_zoom
        // 拿不到 &mut Window，render 在此记录）
        self.viewport_seen = (
            window.viewport_size().width.as_f32(),
            window.viewport_size().height.as_f32(),
        );
        // 3.4 问卷自由文本输入框的惰性补建（InputState::new 要 window；
        // 有条件一次性 mutate，建满即收敛不 notify）
        self.ensure_ask_inputs(window, cx);
        let list_state = self.list.clone();
        let entity = cx.entity();
        // 消息流：不等高惰性测量（Bottom 对齐 + 512 overscan），Tail 跟随在
        // ListState 内建（新消息贴底 / 上滚暂停 / 回底续跟）；B1 空态：无行
        // 时问候语 + 建议 chips 整体替代列表（列表 0 行本来画不出东西）
        let stream_area: gpui_kit::AnyElement = if self.is_empty_view() {
            div()
                .flex_1()
                .min_h_0()
                .child(self.render_empty_state(cx))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .child(
                    gpui_kit::list(list_state, move |ix, _window, cx| {
                        entity.update(cx, |conversation, cx| conversation.render_row(ix, cx))
                    })
                    .size_full(),
                )
                .into_any_element()
        };
        let mut column = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            // Shift+Tab 快切权限档（3.4）：按键沿 focus path（composer 内
            // 输入框 → 本实体）冒泡到这里处理
            .on_action(cx.listener(|this: &mut ConversationState, _: &CyclePermissionTier, _, cx| {
                if let Some(composer) = &this.composer {
                    composer.update(cx, |composer, cx| composer.cycle_tier(cx));
                }
            }))
            .child(stream_area);
        // B6 run 级错误卡 + Ask 条带（3.4/3.6：消息流尾部、composer 之前的
        // 内联条带区——「出现在消息流当前 run 的位置」的形态取舍见 ask.rs
        // 模块注释）
        if let Some(error_card) = subagent::render_run_error(self, cx) {
            column = column.child(error_card);
        }
        if let Some(ask_bar) = ask::render_ask_bar(self, cx) {
            column = column.child(ask_bar);
        }
        if let Some(composer) = &self.composer {
            // 编辑重发提示条（3.3）：待发态可见（发送将产生新分支），点 ×
            // 取消回到常规发送
            let composer_area = div()
                .flex_shrink_0()
                .px(px(crate::theme::SPACE_5))
                .pb(px(crate::theme::SPACE_5))
                .when(self.resend_pending(), |area| {
                    area.child(self.render_resend_banner(cx))
                })
                .child(composer.clone());
            column = column.child(composer_area);
        }
        // 子代理详情视图（3.6）：开着时覆盖中栏（含 composer——详情是
        // 独立会话视角，返回入口收场）
        if let Some(sub_view) = subagent::render_sub_view(self, cx) {
            column = column.child(sub_view);
        }
        // zoom 覆盖层（2.9）：开着时叠在中栏上（inset_0 + occlude 挡住下层
        // 点击），背景点击 / 关闭钮收场
        if let Some(overlay) = self.render_image_zoom(cx) {
            column = column.child(overlay);
        }
        column
    }
}

impl ConversationState {
    /// 建议 chip 点击（B1）：文案填入 composer（不自动发送——发送由用户
    /// Enter 决定，与旧栈「点击填入 composer」语义一致）。
    pub(crate) fn fill_suggestion(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(composer) = &self.composer {
            composer.update(cx, |composer, cx| composer.set_text(text, window, cx));
        }
    }

    /// B1 空态：时段问候语（smart_time 同源的时段判定）+ 副句 + 建议
    /// chips；无会话时多一行引导文案（引导创建——发送链在首条消息时
    /// 自动建会话，输入即开始）。
    fn render_empty_state(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().colors;
        let zoom = cx.craftwave().zoom;
        let no_session = self
            .service
            .as_ref()
            .is_none_or(|service| service.read(cx).session_id().is_none());
        // 建议 chips 文案（i18n 词表；点击填 composer）
        let suggestions = [
            t("Explain this repository's structure"),
            t("Help me write a build script"),
            t("Review the recent changes"),
            t("Summarize today's meeting notes"),
        ];
        let mut stack = div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(crate::theme::SPACE_4))
            .child(
                div()
                    .text_size(px(crate::theme::font_size(crate::theme::FONT_UI_XL, zoom)))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(t(greeting())),
            )
            .child(
                div()
                    .text_size(px(crate::theme::font_size(crate::theme::FONT_UI_BASE, zoom)))
                    .text_color(theme.muted_foreground)
                    .child(t("What can I help you with?")),
            );
        if no_session {
            stack = stack.child(
                div()
                    .max_w(px(crate::chat::state::READER_MAX_WIDTH))
                    .px(px(crate::theme::SPACE_10))
                    .text_center()
                    .text_size(px(crate::theme::font_size(crate::theme::FONT_UI_SM, zoom)))
                    .text_color(cx.craftwave().colors.foreground_subtle)
                    .child(t(
                        "No conversation yet — type below to start a new session, or pick one from the sidebar.",
                    )),
            );
        }
        stack = stack.child(
            div()
                .flex()
                .flex_wrap()
                .justify_center()
                .gap(px(crate::theme::SPACE_3))
                .max_w(px(crate::chat::state::READER_MAX_WIDTH))
                .children(suggestions.map(|label| {
                    div()
                        .id(SharedString::from(label))
                        .cursor_pointer()
                        .px(px(crate::theme::SPACE_5))
                        .py(px(crate::theme::SPACE_2))
                        .rounded(px(crate::theme::rounded_lg(zoom)))
                        .border_1()
                        .border_color(theme.border)
                        .bg(cx.craftwave().colors.composer_surface)
                        .text_size(px(crate::theme::font_size(crate::theme::FONT_UI_SM, zoom)))
                        .text_color(theme.foreground)
                        .hover(|style| style.bg(theme.list_hover))
                        .debug_selector(move || format!("suggestion-chip-{}", label))
                        .on_click({
                            let entity = cx.entity();
                            move |_, window, cx| {
                                entity.update(cx, |conversation, cx| {
                                    conversation.fill_suggestion(label, window, cx);
                                });
                            }
                        })
                        .child(label)
                })),
        );
        stack.into_any_element()
    }

    /// 编辑重发提示条（3.3）：composer 上方一条说明（发送将产生新分支）
    /// + 取消钮。取消只清重发态，输入保留（用户可再手动清空）。
    fn render_resend_banner(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().colors;
        let zoom = cx.craftwave().zoom;
        let entity = cx.entity();
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(crate::theme::SPACE_3))
            .px(px(crate::theme::SPACE_4))
            .py(px(crate::theme::SPACE_2))
            .rounded(px(crate::theme::rounded_md(zoom)))
            .bg(theme.muted)
            .mb(px(crate::theme::SPACE_2))
            .debug_selector(|| "resend-banner".to_owned())
            .child(
                div()
                    .text_size(px(crate::theme::font_size(crate::theme::FONT_UI_SM, zoom)))
                    .text_color(theme.muted_foreground)
                    .child(t(
                        "Editing history message — sending creates a new branch",
                    )),
            )
            .child(
                div()
                    .id("resend-cancel")
                    .cursor_pointer()
                    .text_size(px(crate::theme::font_size(crate::theme::FONT_UI_SM, zoom)))
                    .text_color(theme.muted_foreground)
                    .hover(|style| style.text_color(theme.foreground))
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |conversation, cx| conversation.cancel_resend(cx));
                    })
                    .child(t("Cancel edit")),
            )
            .into_any_element()
    }

    /// zoom 覆盖层（Wake render_image_zoom 形态收窄）：Ready 态图按视口
    /// 自适应展示（不放大超过原尺寸），scrim 遮罩 + 关闭钮。zoom 目标不可
    /// 渲染（缓存被清等）时不画——防御路径，正常流不会走到。
    fn render_image_zoom(&self, cx: &Context<Self>) -> Option<gpui_kit::AnyElement> {
        let (ix, image_ix) = self.zoom_open()?;
        match self.image_state(ix, image_ix)? {
            images::ImageLoad::Ready { image, dims } => {
                let theme = cx.theme().colors;
                let scrim = cx.craftwave().colors.scrim;
                let zoom = cx.craftwave().zoom;
                let (width, height) = images::zoom_fit(*dims, images::zoom_avail(self.viewport_seen));
                let image = image.clone();
                Some(
                    div()
                        .id("image-zoom")
                        .debug_selector(|| "image-zoom".to_owned())
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(scrim)
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                            this.close_zoom(cx);
                        }))
                        .child(
                            div()
                                .id("image-zoom-content")
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(crate::theme::SPACE_3))
                                .on_click(|_, _, cx| cx.stop_propagation())
                                .child(
                                    gpui_kit::img(image)
                                        .id("image-zoom-figure")
                                        .w(width)
                                        .h(height)
                                        .rounded(px(crate::theme::rounded_lg(zoom))),
                                )
                                .child(
                                    div()
                                        .id("image-zoom-close")
                                        .px(px(crate::theme::SPACE_3))
                                        .py(px(crate::theme::SPACE_1))
                                        .rounded(px(crate::theme::rounded_md(zoom)))
                                        .cursor_pointer()
                                        .text_size(px(crate::theme::font_size(
                                            crate::theme::FONT_UI_SM,
                                            zoom,
                                        )))
                                        .text_color(theme.muted_foreground)
                                        .hover(|style| style.bg(theme.list_hover))
                                        .debug_selector(|| "image-zoom-close".to_owned())
                                        .on_click(
                                            cx.listener(|this: &mut Self, _, _, cx| {
                                                this.close_zoom(cx);
                                            }),
                                        )
                                        .child(crate::i18n::t("Close")),
                                ),
                        )
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }
}
