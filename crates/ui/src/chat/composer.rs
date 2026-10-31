//! Composer（gpui-rewrite 2.10 + 2.C-2b 真通道）：多行输入 + Enter 发送 +
//! @ 补全（异步真源）+ 图片附件 + 运行中停止钮。
//!
//! ## 组件面（spike 04 号票踩平的用法直接沿用）
//!
//! - gpui-component `Textarea`（**不是** `Input`——那是单行组件）：
//!   `auto_grow(1, 6)` 一行起步、最多六行滚动；`submit_on_enter(true)` 后
//!   Enter 走 `InputEvent::PressEnter`（Shift+Enter 组件内建换行）；
//! - `set_value` 需要 `&mut Window`：输入事件订阅用 `cx.subscribe_in`
//!   五参闭包（事件回调拿得到 window）——装配在 mod.rs 的 install 里。
//!
//! ## @ 补全（at-refs 平移 + 2.C-2b 异步真源）
//!
//! 输入 `@` 触发文件路径补全：光标前最近一个 `@` 到光标之间无空白即视为
//! 查询词。数据源经 [`CompletionSource`] 注入：生产 =
//! `service::EngineCompletionSource`（engine `search_workspace_paths` 跑
//! 后台线程池）；测试 = fixture 假源（同步快返回，同一 trait 面）。
//!
//! 结果绑**查询词**对账（异步拒绝过期）：发起时记住查询词，回写时查询词
//! 已变（用户继续输入触发了更新的查询 / 弹层已关）即丢弃。查询任务的
//! 句柄存 owner（[`ComposerState::completion_task`]），新一轮查询即
//! drop 旧任务。
//!
//! 弹层交互：点击条目插入 / Enter 接受高亮项；键盘上下导航与 Escape
//! 关闭随键位面切片再接（本批 up/down 键还归输入框光标移动）。
//!
//! ## 附件（队列类型用 engine 的 QueuedImageAttachment）
//!
//! 附件按钮本批入队的仍是演示占位（文件选择器阶段 4 接 rfd；真落盘
//! 通道 `SessionService::attach_image_from_path` 已链通，由测试驱动）；
//! 发送时附件展开为 WirePart::Image 并清空。
//!
//! ## 发送与停止（2.C-2b）
//!
//! `take_outgoing` 只产出（正文 + 附件）交回调用方——真通道由
//! mod.rs 的分流交给 [`super::service::SessionService`]（会话创建 /
//! 落盘 / start_chat 都在服务侧）；无服务的装配（测试装置）走回声。
//! 运行中（`busy`，由会话侧推送）显示停止钮，点击经取消目标
//! （服务弱引用）调 `cancel_run`——引擎补发终态帧，节流循环自然收口。

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{App, Entity, SharedString, Task, WeakEntity, Window, div, px};

use craftwave_engine::api::workspace::WorkspacePathHit;
use craftwave_engine::followup::QueuedImageAttachment;

use crate::i18n::t;
use crate::theme::{self, ActiveCraftwave as _};

use super::images::{self, ImageLoad, ImageLoader};
use super::service::SessionService;

/// @ 补全弹层宽度（基准像素，随缩放档缩放）：路径列 + 类型徽标的定稿宽度。
/// 编码指南 §3：非白名单 px 字面量迁移为 token 派生（模块常量 × zoom，
/// 与 theme::rounded_* 同构）。
const COMPLETION_WIDTH: f32 = 360.0;

/// @ 补全数据源接口：查询词 → 命中条目任务（engine `WorkspacePathHit`）。
/// 真实现（engine 搜索）跑后台线程池，假实现（fixture）同步快返回——
/// composer 只消费 Task，两种来源同一条回写路径。
pub trait CompletionSource {
    fn search(&self, query: &str, cx: &mut App) -> Task<Vec<WorkspacePathHit>>;
}

/// 打开的补全弹层状态（None = 关闭）。
pub struct Completion {
    /// `@` 在输入全文中的下标（接受时从这里替换）。
    pub at_ix: usize,
    /// 触发时的光标位置（查询词右界；接受时替换到这里的旧文）。
    pub cursor: usize,
    /// 候选条目。
    pub candidates: Vec<WorkspacePathHit>,
    /// 高亮项下标（键盘导航随键位面接；本批 Enter 接受它）。
    pub selected: usize,
}

/// Composer 状态：输入框 + 附件队列 + @ 补全（异步）+ 运行停止钮。
/// 发送链在 mod.rs 的订阅回调层分流（真服务 / 回声）——composer 只负责
/// 产出输入面数据（见 `take_outgoing` 的注释）。
pub struct ComposerState {
    /// 多行输入框（TextareaState 实体；渲染走 `Textarea::new`）。
    input: Entity<TextareaState>,
    /// 图片附件队列（engine 类型；发送时清空）。
    attachments: Vec<QueuedImageAttachment>,
    /// 打开的补全弹层（None = 关闭）。
    completion: Option<Completion>,
    /// @ 补全数据源（生产 = engine 异步搜索；测试 = fixture 假源）。
    source: Rc<dyn CompletionSource>,
    /// 在途补全查询的任务句柄（存 owner：新一轮查询 drop 旧任务即取消）。
    completion_task: Option<Task<()>>,
    /// 在途查询的查询词（结果对账基准：回写时已变即过期丢弃）。
    completion_query: Option<String>,
    /// 有进行中的 run（停止钮显示面；由会话侧经 [`Self::set_busy`] 推送）。
    busy: bool,
    /// 取消目标（服务弱引用；停止钮点击时升级调用 `cancel_run`）。
    cancel_target: Option<WeakEntity<SessionService>>,
    /// 附件图片装载面（2.9：预览条同款缩略图；None = 回声装置——stub
    /// 附件按不可预览卡渲染）。
    image_loader: Option<Rc<dyn ImageLoader>>,
    /// 附件图片三态缓存（键 = blobHash）。
    attachment_images: HashMap<String, ImageLoad>,
    /// 在途装载任务句柄（存 owner）。
    image_tasks: Vec<Task<()>>,
}

impl ComposerState {
    /// 构造：输入框需要在窗口闭包里建（`TextareaState::new` 要 &mut Window）。
    pub fn new(
        source: Rc<dyn CompletionSource>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true);
            state.set_placeholder(t("Reply or type @ for files…"), window, cx);
            state
        });
        Self {
            input,
            attachments: Vec::new(),
            completion: None,
            source,
            completion_task: None,
            completion_query: None,
            busy: false,
            cancel_target: None,
            image_loader: None,
            attachment_images: HashMap::new(),
            image_tasks: Vec::new(),
        }
    }

    /// 输入框实体（mod.rs 的 subscribe_in 装配用）。
    pub fn input(&self) -> &Entity<TextareaState> {
        &self.input
    }

    /// 附件队列（只读；测试断言用。生产面走预览条渲染，直接读字段）。
    #[cfg(test)]
    pub fn attachments(&self) -> &[QueuedImageAttachment] {
        &self.attachments
    }

    /// 补全弹层是否打开。
    pub fn completion_open(&self) -> bool {
        self.completion.is_some()
    }

    /// 当前高亮的补全候选下标（弹层关着时返回 0——调用方先看
    /// [`Self::completion_open`]）。
    pub fn selected_completion(&self) -> usize {
        self.completion.as_ref().map_or(0, |c| c.selected)
    }

    /// 外部填入文本（B1 建议 chips 点击 / 3.3 编辑重发预填）：整体替换
    /// 输入框内容并通知重渲。需要 `&mut Window`（TextareaState::set_value
    /// 的固有要求，见模块注释）。
    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input
            .update(cx, |state, cx| state.set_value(text, window, cx));
        cx.notify();
    }

    // ==================================================================
    // 发送（产出待发数据；真通道 = SessionService，回声 = mod.rs 分流）
    // ==================================================================

    /// 取出待发消息：正文 + 附件二元组，同时清空输入与附件队列。
    /// 空白输入返回 None（不空发）。
    ///
    /// 为什么不在这里追加进会话 / 调服务：Enter 事件走会话实体的订阅
    /// 回调，回调期间会话实体正在被 update——gpui 禁止嵌套 update 同一
    /// 实体，而服务的发送链要回写会话（apply_snapshot）。所以本方法只
    /// 产出数据；真通道的调用由 mod.rs 用 `App::defer` 推迟到本轮
    /// update 结束后（见 mod.rs 分流注释）。
    pub fn take_outgoing(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(String, Vec<QueuedImageAttachment>)> {
        let text = self.input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return None;
        }
        let attachments = std::mem::take(&mut self.attachments);
        self.completion = None;
        self.completion_task = None;
        self.completion_query = None;
        self.input
            .update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
        Some((text, attachments))
    }

    // ==================================================================
    // 运行停止（2.C-2b：busy 由会话侧推送；点击桥到服务 cancel_run）
    // ==================================================================

    /// 推送运行状态（会话实体在发送 / 收口时调用）：busy 变化才 notify，
    /// 同值的重复推送零成本。
    pub(crate) fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        if self.busy != busy {
            self.busy = busy;
            cx.notify();
        }
    }

    /// 注入取消目标（install 装配时调用；服务在 Global，寿命覆盖全部
    /// composer，这里只存弱引用防止反向持有）。
    pub(crate) fn set_cancel_target(
        &mut self,
        target: WeakEntity<SessionService>,
        cx: &mut Context<Self>,
    ) {
        self.cancel_target = Some(target);
        cx.notify();
    }

    // ==================================================================
    // 附件队列（2.9：预览条三态缩略图）
    // ==================================================================

    /// 注入图片装载面（install 装配；与消息流同一装载器——同款缩略图）。
    pub(crate) fn set_image_loader(
        &mut self,
        loader: Rc<dyn ImageLoader>,
        cx: &mut Context<Self>,
    ) {
        self.image_loader = Some(loader);
        // 已在队列的附件（理论时序不会发生，防御性补装载）
        self.load_attachments(cx);
    }

    /// 入队一张图片附件（真按钮的落盘通道 2.C-2 接；本批按钮入队演示占位）。
    /// 入队即发起装载（生命周期钩子，不在 render 里起任务——编码指南 §5），
    /// 三态到位后预览条按同款缩略图渲染。
    pub fn add_attachment(&mut self, attachment: QueuedImageAttachment, cx: &mut Context<Self>) {
        self.attachments.push(attachment);
        self.load_attachments(cx);
        cx.notify();
    }

    /// 扫队列发起装载：无装载面（回声装置）或已有结果/在途的跳过。
    fn load_attachments(&mut self, cx: &mut Context<Self>) {
        let Some(loader) = self.image_loader.clone() else {
            return;
        };
        for attachment in self.attachments.clone() {
            if self.attachment_images.contains_key(&attachment.blob_hash) {
                continue;
            }
            let hash = attachment.blob_hash.clone();
            let mime = attachment.mime_type.clone();
            let task = loader.load(&hash, &mime, cx);
            self.image_tasks.push(cx.spawn(async move |this, cx| {
                let state = task.await;
                let _ = this.update(cx, |this, cx| {
                    this.attachment_images.insert(hash, state);
                    cx.notify();
                });
            }));
        }
    }

    /// 附件按钮动作（本批假装置）：入队一张演示占位附件，让预览条与
    /// 「发送清空」链路端到端可见。真实现（文件选择器 → engine
    /// `attach_image` → 拿 blobHash 入队）在 2.C-2。
    pub fn attach_image_stub(&mut self, cx: &mut Context<Self>) {
        let n = self.attachments.len() + 1;
        self.add_attachment(
            QueuedImageAttachment {
                blob_hash: format!("demo-hash-{n}"),
                mime_type: "image/png".to_owned(),
            },
            cx,
        );
    }

    /// 移除一张待发附件（预览条上的 × ）。
    pub fn remove_attachment(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.attachments.len() {
            self.attachments.remove(ix);
            cx.notify();
        }
    }

    // ==================================================================
    // @ 补全
    // ==================================================================

    /// 输入变化后刷新补全弹层：光标前有活 `@` 词 → 发起异步查询（结果
    /// 回写时开层，候选为空不开）；`@` 词消失 → 关层。
    ///
    /// 异步对账：查询任务句柄存 owner（新一轮查询 drop 旧任务），结果
    /// 回写以查询词为基准（[`Self::apply_completion_results`]）——用户
    /// 继续输入后旧结果过期，直接丢弃。
    pub fn refresh_completion(&mut self, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().to_string();
        let cursor = self.input.read(cx).selected_range().end;
        match at_query(&value, cursor) {
            Some((at_ix, query)) => {
                // 查询词没变就不重查（光标在词内移动不闪烁弹层），
                // 只更新替换锚点
                let query_unchanged = self
                    .completion_query
                    .as_deref()
                    .is_some_and(|inflight| inflight == query);
                if query_unchanged {
                    if let Some(completion) = self.completion.as_mut() {
                        completion.at_ix = at_ix;
                        completion.cursor = cursor;
                    }
                    return;
                }
                // 发起查询：任务句柄存 owner（旧任务 drop 即取消），查询
                // 词记为对账基准；弹层先保持现状（旧候选继续显示，避免
                // 逐字符打字时整层闪烁），结果回写时统一替换
                let query = query.to_owned();
                let task = self.source.search(&query, cx);
                self.completion_query = Some(query.clone());
                self.completion_task = Some(cx.spawn(async move |this, cx| {
                    let hits = task.await;
                    let _ = this.update(cx, |composer, cx| {
                        composer.apply_completion_results(&query, at_ix, cursor, hits, cx);
                    });
                }));
                cx.notify();
            }
            None => {
                // @ 词消失：关层 + 作废在途查询
                self.completion_task = None;
                self.completion_query = None;
                if self.completion.take().is_some() {
                    cx.notify();
                }
            }
        }
    }

    /// 补全结果回写（查询任务完成时）：查询词仍是发起时那一个才采纳
    /// （过期丢弃——用户已继续输入触发了更新的查询），且当时触发查询的
    /// 光标处仍能扫出同一个 @ 词（词被编辑掉的场景丢弃）。候选为空不开层。
    fn apply_completion_results(
        &mut self,
        query: &str,
        at_ix: usize,
        cursor: usize,
        hits: Vec<WorkspacePathHit>,
        cx: &mut Context<Self>,
    ) {
        let inflight_matches = self
            .completion_query
            .as_deref()
            .is_some_and(|inflight| inflight == query);
        if !inflight_matches {
            return;
        }
        self.completion_task = None;
        if !hits.is_empty() {
            self.completion = Some(Completion {
                at_ix,
                cursor,
                candidates: hits,
                selected: 0,
            });
        } else {
            self.completion = None;
            self.completion_query = None;
        }
        cx.notify();
    }

    /// 接受一条候选：把 `@查询词` 替换为 `@完整路径 `（尾随空格方便续打）。
    pub fn accept_completion(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(completion) = self.completion.take() else {
            return;
        };
        let Some(hit) = completion.candidates.get(ix) else {
            return;
        };
        let value = self.input.read(cx).value().to_string();
        let end = completion.cursor.min(value.len());
        let inserted = format!("@{} ", hit.path);
        let new_value = format!("{}{inserted}{}", &value[..completion.at_ix], &value[end..]);
        self.input
            .update(cx, |state, cx| state.set_value(new_value, window, cx));
        cx.notify();
    }

    /// 关闭补全弹层（Escape / 点外部的调用点在 2.C-2 随键位面接入；
    /// 本批只有测试驱动它，故仅测试编译存在）。
    #[cfg(test)]
    pub fn close_completion(&mut self, cx: &mut Context<Self>) {
        if self.completion.take().is_some() {
            cx.notify();
        }
    }

    // ==================================================================
    // 渲染
    // ==================================================================

    /// 附件预览条：每个附件一枚同款缩略图（2.9 三态形制——Ready 渲染图、
    /// 其余不可预览卡），右上角叠移除 ×。
    fn render_attachments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let zoom = cx.craftwave().zoom;
        let mut bar = div().w_full().flex().flex_row().flex_wrap().gap(px(theme::SPACE_2));
        for (ix, attachment) in self.attachments.iter().enumerate() {
            let id = SharedString::from(format!("attachment-preview-{ix}"));
            let debug = id.clone();
            let thumb: gpui_kit::AnyElement = match self.attachment_images.get(&attachment.blob_hash)
            {
                Some(ImageLoad::Ready { image, dims }) => images::render_ready_thumb(
                    id, image, *dims, cx,
                )
                .debug_selector(move || debug.to_string())
                .into_any_element(),
                Some(ImageLoad::Unsupported { mime, original }) => images::render_unsupported_thumb(
                    id,
                    mime,
                    original.as_ref(),
                    cx,
                )
                .debug_selector(move || debug.to_string())
                .into_any_element(),
                Some(ImageLoad::Omitted) => images::render_omitted_thumb(id, cx)
                    .debug_selector(move || debug.to_string())
                    .into_any_element(),
                // 无装载面或装载中：mime 占位卡（不假装有图）
                None => {
                    let colors = cx.theme().colors;
                    div()
                        .id(id)
                        .size(px(images::IMAGE_THUMB))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(theme::rounded_md(zoom)))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.muted)
                        .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                        .text_color(colors.muted_foreground)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .truncate()
                        .debug_selector(move || debug.to_string())
                        .child(attachment.mime_type.clone())
                        .into_any_element()
                }
            };
            bar = bar.child(
                div()
                    .relative()
                    .child(thumb)
                    .child(
                        div()
                            .id(("attachment-remove", ix))
                            .absolute()
                            .top(px(theme::SPACE_1))
                            .right(px(theme::SPACE_1))
                            .size(px(theme::ICON_LG))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(theme::ROUNDED_FULL))
                            .bg(cx.theme().colors.popover)
                            .cursor_pointer()
                            .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                            .text_color(cx.theme().colors.muted_foreground)
                            .hover(|style| style.bg(cx.theme().colors.list_hover))
                            .child("×")
                            .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                                this.remove_attachment(ix, cx);
                            })),
                    ),
            );
        }
        bar
    }

    /// @ 补全弹层（挂在输入壳上方：absolute bottom_full）。
    fn render_completion(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let zoom = cx.craftwave().zoom;
        let Some(completion) = &self.completion else {
            return div().into_any_element();
        };
        let mut list = div()
            .absolute()
            .bottom_full()
            .left_0()
            .w(px(COMPLETION_WIDTH * zoom))
            .rounded(px(theme::rounded_lg(zoom)))
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .overflow_hidden()
            .py(px(theme::SPACE_1));
        for (ix, hit) in completion.candidates.iter().enumerate() {
            let highlighted = ix == completion.selected;
            list = list.child(
                div()
                    .id(("completion-item", ix))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(theme::SPACE_3))
                    .px(px(theme::SPACE_4))
                    .py(px(theme::SPACE_2))
                    .when(highlighted, |row| row.bg(theme.list_hover))
                    .cursor_pointer()
                    .debug_selector(move || format!("completion-item-{ix}"))
                    .on_click(cx.listener(move |this: &mut Self, _, window, cx| {
                        this.accept_completion(ix, window, cx);
                    }))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                            .text_color(theme.foreground)
                            .child(format!("@{}", hit.path)),
                    )
                    .child(
                        div()
                            .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                            .text_color(theme.muted_foreground)
                            .child(hit.kind.clone()),
                    ),
            );
        }
        list.into_any_element()
    }
}

impl Render for ComposerState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // owned 拷贝（Copy）：后面的 render_attachments / render_completion
        // 与各 on_click listener 都要独占 cx，不能留 &cx 借用
        let theme = cx.theme().colors;
        let composer_surface = cx.craftwave().colors.composer_surface;
        let zoom = cx.craftwave().zoom;
        let mut shell = div().w_full().flex().flex_col().gap(px(theme::SPACE_2));
        if !self.attachments.is_empty() {
            shell = shell.child(self.render_attachments(cx));
        }
        // 输入壳：rounded-2xl（白名单形态）+ composer_surface 卡底 + 1px 边框
        shell
            .child(
                div()
                    .relative()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap(px(theme::SPACE_2))
                    .px(px(theme::SPACE_5))
                    .py(px(theme::SPACE_3))
                    .rounded(px(theme::rounded_2xl(zoom)))
                    .border_1()
                    .border_color(theme.border)
                    .bg(composer_surface)
                    // 补全弹层（开层时挂在壳上方）
                    .when(self.completion.is_some(), |composer| {
                        composer.child(self.render_completion(cx))
                    })
                    // 输入框：外观由壳承担（appearance/bordered 关掉组件自绘）
                    .child(
                        Textarea::new(&self.input)
                            .appearance(false)
                            .bordered(false)
                            .flex_1(),
                    )
                    // 停止钮：仅运行中渲染（■ 危险色）。点击桥到服务
                    // cancel_run——引擎置位取消令牌后补发终态帧，节流循环
                    // 自然收口（busy 由会话侧推送复位）
                    .when(self.busy, |composer| {
                        composer.child(
                            div()
                                .id("stop-button")
                                .cursor_pointer()
                                .flex_shrink_0()
                                .px(px(theme::SPACE_2))
                                .py(px(theme::SPACE_1))
                                .rounded(px(theme::rounded_md(zoom)))
                                .text_size(px(theme::font_size(
                                    theme::FONT_UI_CAPTION,
                                    zoom,
                                )))
                                .text_color(theme.danger)
                                .debug_selector(|| "stop-button".to_owned())
                                .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                    if let Some(service) = &this.cancel_target {
                                        let _ =
                                            service.update(cx, |service, cx| service.cancel(cx));
                                    }
                                }))
                                .child(t("Stop")),
                        )
                    })
                    // 附件按钮：＋（真文件选择器通道阶段 4 接 rfd，本批入
                    // 演示占位；真落盘通道由测试驱动验证）
                    .child(
                        div()
                            .id("attach-button")
                            .cursor_pointer()
                            .flex_shrink_0()
                            .px(px(theme::SPACE_2))
                            .py(px(theme::SPACE_1))
                            .rounded(px(theme::rounded_md(zoom)))
                            .text_size(px(theme::font_size(theme::FONT_UI_LG, zoom)))
                            .text_color(theme.muted_foreground)
                            .debug_selector(|| "attach-button".to_owned())
                            .on_click(cx.listener(|this: &mut Self, _, _, cx| {
                                this.attach_image_stub(cx);
                            }))
                            .child("＋"),
                    ),
            )
            .into_any_element()
    }
}

/// @ 词扫描：光标前最近的 `@`，到光标之间无空白即为活查询词。
/// 返回（`@` 下标，查询词）。跨行输入里 `@` 后带换行同样视作断开。
pub(crate) fn at_query(text: &str, cursor: usize) -> Option<(usize, &str)> {
    let end = cursor.min(text.len());
    let before = &text[..end];
    let at_ix = before.rfind('@')?;
    let query = &before[at_ix + 1..];
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    Some((at_ix, query))
}

/// composer 事件入口（mod.rs 的 subscribe_in 回调）：Change 刷新补全；
/// Enter（非 Shift）在弹层开着时接受高亮候选。普通 Enter 的「发送」不走
/// 这里——mod.rs 的回调先分流（弹层关着才发送），在会话自己的 Context 里
/// 完成追加（避免嵌套 update 会话实体，见 [`ComposerState::take_outgoing`]）。
/// Shift+Enter 不进这里——组件内建换行，不产生 PressEnter。
pub(crate) fn on_input_event(
    composer: &mut ComposerState,
    event: &InputEvent,
    window: &mut Window,
    cx: &mut Context<ComposerState>,
) {
    match event {
        InputEvent::Change => composer.refresh_completion(cx),
        InputEvent::PressEnter { shift: false, .. } if composer.completion_open() => {
            let selected = composer.selected_completion();
            composer.accept_completion(selected, window, cx);
        }
        _ => {}
    }
}
