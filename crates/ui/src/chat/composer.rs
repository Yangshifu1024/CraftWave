//! Composer（gpui-rewrite 2.10）：多行输入 + Enter 发送 + @ 补全 + 图片附件。
//!
//! ## 组件面（spike 04 号票踩平的用法直接沿用）
//!
//! - gpui-component `Textarea`（**不是** `Input`——那是单行组件）：
//!   `auto_grow(1, 6)` 一行起步、最多六行滚动；`submit_on_enter(true)` 后
//!   Enter 走 `InputEvent::PressEnter`（Shift+Enter 组件内建换行）；
//! - `set_value` 需要 `&mut Window`：输入事件订阅用 `cx.subscribe_in`
//!   五参闭包（事件回调拿得到 window）——装配在 mod.rs 的 install 里。
//!
//! ## @ 补全（at-refs 平移的接口面）
//!
//! 输入 `@` 触发文件路径补全：光标前最近一个 `@` 到光标之间无空白即视为
//! 查询词。真数据源是 engine `api/workspace.rs::search_workspace_paths`
//! （需要 AppState 装配链，2.C-2 / 集成周接通），本批经
//! [`CompletionSource`] trait 用假数据 stub（fixture::FakeCompletionSource）。
//!
//! 弹层交互本批：点击条目插入 / Enter 接受高亮项；键盘上下导航与 Escape
//! 关闭随 2.C-2 的键位面一起接（本批 up/down 键还归输入框光标移动）。
//!
//! ## 附件（队列类型用 engine 的 QueuedImageAttachment）
//!
//! 附件按钮本批入队的是演示占位（真按钮走文件选择器 + `attach_image`
//! 落盘拿 blobHash，2.C-2 接线）；发送时附件展开为 WirePart::Image 并清空。

use std::rc::Rc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::{Entity, Window, div, px};

use craftwave_engine::api::workspace::WorkspacePathHit;
use craftwave_engine::followup::QueuedImageAttachment;
use craftwave_engine::storage::{MessageEntry, MessageRole, WirePart};
use craftwave_engine::util::ids::new_uuid;
use craftwave_engine::storage;

use crate::i18n::t;
use crate::theme::{self, ActiveCraftwave as _};


/// @ 补全数据源接口：查询词 → 命中条目（engine `WorkspacePathHit`）。
/// 2.C-2 的真实现包一层 `search_workspace_paths`（同步调用，预算 P95<300ms；
/// 若届时要异步化，把返回改成 Task 再平移调用点）。
pub trait CompletionSource {
    fn search(&self, query: &str) -> Vec<WorkspacePathHit>;
}

/// 打开的补全弹层状态（None = 关闭）。
pub struct Completion {
    /// `@` 在输入全文中的下标（接受时从这里替换）。
    pub at_ix: usize,
    /// 触发时的光标位置（查询词右界；接受时替换到这里的旧文）。
    pub cursor: usize,
    /// 当前查询词。
    pub query: String,
    /// 候选条目。
    pub candidates: Vec<WorkspacePathHit>,
    /// 高亮项下标（键盘导航 2.C-2 接；本批 Enter 接受它）。
    pub selected: usize,
}

/// Composer 状态：输入框 + 附件队列 + @ 补全。发送链（会话追加 /
/// 2.C-2 的 start_chat）在 mod.rs 的订阅回调层完成——composer 只负责
/// 产出输入面数据（见 `take_outgoing` 的注释）。
pub struct ComposerState {
    /// 多行输入框（TextareaState 实体；渲染走 `Textarea::new`）。
    input: Entity<TextareaState>,
    /// 图片附件队列（engine 类型；发送时清空）。
    attachments: Vec<QueuedImageAttachment>,
    /// 打开的补全弹层（None = 关闭）。
    completion: Option<Completion>,
    /// @ 补全数据源（本批假 stub，2.C-2 换 engine 搜索）。
    source: Rc<dyn CompletionSource>,
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

    // ==================================================================
    // 发送（本批回声式：用户消息直接追加进会话；真 start_chat 在 2.C-2）
    // ==================================================================

    /// 取出待发消息：正文 + 附件 → 一条 User `MessageEntry`，同时清空
    /// 输入与附件队列。空白输入返回 None（不空发）。
    ///
    /// 为什么不在这里追加进会话：Enter 事件走会话实体的订阅回调，回调期间
    /// 会话实体正在被 update——gpui 禁止嵌套 update 同一实体。所以本方法
    /// 只产出消息，追加由 mod.rs 的回调在会话自己的 Context 里完成
    /// （会话的 splice 重排与 notify 都落在它的 Context 上，语义完整）。
    pub fn take_outgoing(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Rc<MessageEntry>> {
        let text = self.input.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return None;
        }
        // 正文在前，附件随其后展开为 Image part（wire 契约：blobHash + mime）
        let mut parts = vec![WirePart::Text { text }];
        for attachment in &self.attachments {
            parts.push(WirePart::Image {
                blob_hash: attachment.blob_hash.clone(),
                mime_type: attachment.mime_type.clone(),
            });
        }
        let entry = Rc::new(MessageEntry {
            id: new_uuid(),
            // parent_id / 树挂接随 2.C-2 的 start_chat 装配链补上（回声式
            // 消息只进渲染流，不落盘、不挂树）
            parent_id: None,
            role: MessageRole::User,
            parts,
            created_at: storage::trust::now_ms(),
            run_id: None,
            status: None,
        });
        self.attachments.clear();
        self.completion = None;
        self.input.update(cx, |state, cx| state.set_value("", window, cx));
        cx.notify();
        Some(entry)
    }

    // ==================================================================
    // 附件队列
    // ==================================================================

    /// 入队一张图片附件（真按钮的落盘通道 2.C-2 接；本批按钮入队演示占位）。
    pub fn add_attachment(&mut self, attachment: QueuedImageAttachment, cx: &mut Context<Self>) {
        self.attachments.push(attachment);
        cx.notify();
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

    /// 输入变化后刷新补全弹层：光标前有活 `@` 词 → 查询并开层（候选为空
    /// 不开）；`@` 词消失 → 关层。
    pub fn refresh_completion(&mut self, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().to_string();
        let cursor = self.input.read(cx).selected_range().end;
        match at_query(&value, cursor) {
            Some((at_ix, query)) => {
                // 查询词没变就不重查（光标在词内移动不闪烁弹层）
                let query_unchanged =
                    self.completion.as_ref().is_some_and(|c| c.query == query);
                if query_unchanged {
                    if let Some(completion) = self.completion.as_mut() {
                        completion.at_ix = at_ix;
                        completion.cursor = cursor;
                    }
                    return;
                }
                let candidates = self.source.search(query);
                self.completion = (!candidates.is_empty()).then(|| Completion {
                    at_ix,
                    cursor,
                    query: query.to_owned(),
                    candidates,
                    selected: 0,
                });
                cx.notify();
            }
            None => {
                if self.completion.take().is_some() {
                    cx.notify();
                }
            }
        }
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

    /// 附件预览条：每个附件一枚 chip（mime 标签 + 移除 ×）。
    fn render_attachments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let zoom = cx.craftwave().zoom;
        let mut bar = div().w_full().flex().flex_row().flex_wrap().gap(px(theme::SPACE_2));
        for (ix, attachment) in self.attachments.iter().enumerate() {
            bar = bar.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(theme::SPACE_1))
                    .px(px(theme::SPACE_3))
                    .py(px(theme::SPACE_1))
                    .rounded(px(theme::rounded_md(zoom)))
                    .bg(theme.muted)
                    .text_size(px(theme::font_size(theme::FONT_UI_CAPTION, zoom)))
                    .text_color(theme.muted_foreground)
                    .child(attachment.mime_type.clone())
                    .child(
                        div()
                            .id(("attachment-remove", ix))
                            .cursor_pointer()
                            .text_color(theme.muted_foreground)
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
            .w(px(360.0))
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
                    // 附件按钮：＋（真文件选择器通道 2.C-2 接，本批入演示占位）
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
