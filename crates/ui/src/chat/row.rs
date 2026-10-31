//! 行分发四形态的元素构建（gpui-rewrite 2.4 / 2.C-3 / 2.C-4，rendering.md
//! 「行分发」「Markdown 正文」「thinking 面板」「工具簇卡」「图片」逐条）。
//!
//! 判定与路由在 state.rs（`render_row` 入口），本模块只产元素：
//!
//! - **User**：右对齐气泡，`max_w(540px)`、muted 底、行高 1.85、内容段
//!   （`TextView::markdown`，段间距 0.5rem）直挂气泡不加包装层（max-content
//!   探测问题的 Wake 结论）；图片 part 走三态缩略图（images.rs，点击 Ready
//!   态开 zoom）；B5 右上折角 + 1px 边框（折角本批为占位形制——方形翻角
//!   chip，gpui 无 div 级 rotate，三角折角待有 svg/路径面后精修）；
//! - **Assistant**：全宽列 = thinking 面板槽位（2.7 转正：真 chevron 图标
//!   旋转 + B4 流式态）→ 正文槽位（`TextView::markdown` 四定制点，段间距
//!   0.6rem）→ 工具簇槽位（2.8 转正：簇卡 + 600 字符契约），平铺；
//! - **System / CompactSummary**：居中小胶囊（one_line 压行 + `.truncate()`
//!   宽度兜底）；
//! - **过滤器**：运行日志型 → state.rs 已拦截为零高空行，不会进到本模块。
//!
//! 纪律对照：折叠不嵌套（只有 thinking / 工具簇两个槽位可折）；截断只发生在
//! 元数据（胶囊文本、thinking 摘要、工具参数摘要与 600 字符预览），正文槽位
//! 永不截断；错误 Output 全文可选可复制，长报错展开后自然增高（不嵌套滚动
//! 容器抢主区滚轮——Wake 10037 注释的教训）。

use gpui_kit::assets::IconName;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::shimmer::ShimmerText;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::{ActiveTheme as _, Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, FontWeight, Radians, SharedString,
    StyleRefinement, div, px, relative, rems,
};

use crate::i18n::t;
use crate::text_util::{clip_display, one_line};
use crate::theme::{self, ActiveCraftwave as _};

use super::images::{self, ImageLoad};
use super::markdown_links;
use super::state::{
    CallLite, ConversationState, READER_MAX_WIDTH, Slot, StreamView, USER_BUBBLE_MAX_WIDTH,
};

/// thinking 展开区的左缩进（rendering.md「thinking 面板」钉值 30px，
/// 白名单②）。
const THINKING_INDENT: f32 = 30.0;
/// B5 折角 chip 的角距与边长（占位形制钉值：方形翻角贴右上角；gpui 无
/// div 级 rotate，三角折角随 svg/路径能力接入后连同这两个值一起重订）。
const FOLD_CHIP_INSET: f32 = 6.0;
/// B5 折角 chip 边长（见 [`FOLD_CHIP_INSET`] 注释）。
const FOLD_CHIP_SIZE: f32 = 8.0;
/// thinking 摘要的显示单元宽度预算（rendering.md 钉值 88 格；先 one_line(400)
/// 压行再按宽截断）。
const THINKING_SUMMARY_WIDTH: usize = 88;
/// 胶囊文本的字符预算（纪律二 one_line 单行截断；只压行数不精确控宽——
/// 宽度由 `.truncate()` 按容器兜底，预算大到几乎不影响实际展示）。
const PILL_MAX_CHARS: usize = 200;
/// 工具 Input/Output 的展示预览上限（rendering.md 纪律三钉值 600 字符；
/// 复制按钮永远复制完整原文——见 [`tool_section`]）。
const TOOL_TEXT_PREVIEW_LIMIT: usize = 600;
/// 工具簇折叠头的横向预留（chevron + 标题 + 跳转钮 + 失败 badge + 内边距
/// 的保守合计）：参数摘要的可用宽 = 阅读区宽 - 本值（Wake 7103 的 156px
/// 同语义；取整到 ours 的间距档）。
const TOOL_HEAD_RESERVE: f32 = 180.0;
/// 等宽字符格宽系数（em 倍数）：Menlo 等宽 ASCII 格宽 ≈ 0.6em。格宽是
/// 「可显示格数」反算的估算量，误差由 min clamp 与 clip_display 的省略号
/// 兜底（Wake TOOL_MONO_CELL_WIDTH 6.9px / 11.6px 字号 ≈ 同系数）。
const MONO_CELL_EM: f32 = 0.6;

/// 行渲染总入口：角色分发 + 阅读轴包装（`justify_center + max_w` 共用竖直轴，
/// 首条 pt / 末条 pb 呼吸边距）。
pub(crate) fn render_row(
    state: &ConversationState,
    ix: usize,
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let zoom = cx.craftwave().zoom;
    // 角色分发（流式行按 Assistant 形态走三槽位列，附 B4/跳转钮的流式上下文）
    let content: AnyElement = if ix >= state.messages().len() {
        match state.streaming() {
            Some(streaming) => {
                let view = StreamView {
                    thinking_span: streaming.thinking.clone(),
                    live: streaming.ended.is_none(),
                    subagents: streaming.subagents.clone(),
                };
                let slots = super::state::slots_from_stream(&streaming.parts, &view.subagents);
                render_assistant(state, ix, slots, &view, cx)
            }
            None => div().into_any_element(),
        }
    } else {
        let entry = &state.messages()[ix];
        match entry.role {
            craftwave_engine::storage::MessageRole::User => render_user(state, entry, ix, cx),
            craftwave_engine::storage::MessageRole::Assistant => render_assistant(
                state,
                ix,
                super::state::slots_from_wire(&entry.parts),
                &StreamView::idle(),
                cx,
            ),
            craftwave_engine::storage::MessageRole::System => render_system(entry, cx),
        }
    };
    // 阅读轴：全消息共用同一竖直轴；首条 pt / 末条 pb 呼吸边距。
    // 3.3 分支栏（消息有多个兄弟分支时）挂在消息下方同一轴内
    let last = ix + 1 == state.row_count();
    let branch_bar = render_branch_bar(state, ix, cx);
    // 3.8 查找命中高亮：非当前的命中行，若命中词落在 TextView 正文
    // （Text part），给行整体淡 primary 底（TextView 内部词级高亮不可达
    // 的降级形态，find.rs 模块注释有核实结论）；当前命中行走 wrap_flash
    // 的闪示条（强一档底 + 淡出，负 margin/等量 padding 外扩不推挤）。
    let hit_tint = super::find::row_is_hit(state, ix) && row_has_visible_text(state, ix);
    let hit_tint_bg = cx.craftwave().colors.find_hit;
    let axis = div()
        .w_full()
        .max_w(px(READER_MAX_WIDTH))
        .text_size(px(theme::font_size(theme::FONT_UI_BASE, zoom)))
        .text_color(cx.theme().foreground)
        .when(hit_tint, |axis| {
            axis.rounded(px(theme::rounded_md(zoom))).bg(hit_tint_bg)
        })
        .child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(content)
                .children(branch_bar),
        );
    div()
        .w_full()
        .flex()
        .justify_center()
        .when(ix == 0, |row| row.pt(px(theme::SPACE_5)))
        .when(last, |row| row.pb(px(theme::SPACE_6)))
        .child(super::find::wrap_flash(
            state,
            ix,
            axis.into_any_element(),
            cx,
        ))
        .into_any_element()
}

/// 该行是否有 TextView 正文可挂行级淡底（Text part 在场——命中词落在
/// thinking/工具参数等普通段时走词级高亮，不给行底）。
fn row_has_visible_text(state: &ConversationState, ix: usize) -> bool {
    state
        .messages()
        .get(ix)
        .is_some_and(|entry| {
            entry
                .parts
                .iter()
                .any(|p| matches!(p, craftwave_engine::storage::WirePart::Text { .. }))
        })
}

// ======================================================================
// 分支栏（3.3）：‹ k/n › 相邻分支切换；User 行附「编辑重发」入口。
// 单分支隐藏（旧栈 BranchPickerPrimitive.Root hideWhenSingleBranch 语义）。
// ======================================================================

/// 分支栏（None = 单分支 / 无表数据——不渲染任何控件）。
fn render_branch_bar(
    state: &ConversationState,
    ix: usize,
    cx: &mut Context<ConversationState>,
) -> Option<AnyElement> {
    let entry = state.messages().get(ix)?;
    let info = state.branch_of(&entry.id)?;
    if info.count <= 1 {
        return None;
    }
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let message_id = entry.id.clone();
    // 相邻分支切换钮：点击 defer 调服务切活动叶 + 重载（会话实体的
    // listener 栈内不能嵌套 update 会话，与 Enter 发送同款手法）
    let branch_btn = |dir: super::service::BranchDir, icon_name: IconName| {
        let message_id = message_id.clone();
        let btn_id: SharedString = match dir {
            super::service::BranchDir::Prev => format!("branch-prev-{message_id}").into(),
            super::service::BranchDir::Next => format!("branch-next-{message_id}").into(),
        };
        div()
            .id(btn_id)
            .cursor_pointer()
            .flex_shrink_0()
            .rounded(px(theme::rounded_md(zoom)))
            .text_color(theme.muted_foreground)
            .hover(|style| style.bg(theme.list_hover).text_color(theme.foreground))
            .tooltip(move |window, cx| {
                let label = match dir {
                    super::service::BranchDir::Prev => t("Previous branch"),
                    super::service::BranchDir::Next => t("Next branch"),
                };
                gpui_kit::component::tooltip::Tooltip::new(label).build(window, cx)
            })
            .on_click(cx.listener(move |this: &mut ConversationState, _, _, cx| {
                let Some(service) = this.service.clone() else {
                    return;
                };
                let id = message_id.clone();
                cx.defer(move |cx| {
                    service.update(cx, |service, cx| service.switch_branch(&id, dir, cx));
                });
            }))
            .child(Icon::new(icon_name).with_size(px(theme::ICON_SM)))
    };
    let is_user = entry.role == craftwave_engine::storage::MessageRole::User;
    // 编辑重发入口（仅 User 行——旧栈 BranchBar editOnly 语义）：composer
    // 预填原文 + 记目标父节点，Enter 发送走 branch_resend 产生新分支
    let resend_btn = is_user.then(|| {
        let original = entry
            .parts
            .iter()
            .find_map(|part| match part {
                craftwave_engine::storage::WirePart::Text { text } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        let entry_snapshot = (**entry).clone();
        div()
            .id(SharedString::from(format!("branch-resend-{}", entry.id)))
            .cursor_pointer()
            .flex_shrink_0()
            .rounded(px(theme::rounded_md(zoom)))
            .text_color(theme.muted_foreground)
            .hover(|style| style.bg(theme.list_hover).text_color(theme.foreground))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(t("Edit and resend")).build(window, cx)
            })
            .on_click(cx.listener(move |this: &mut ConversationState, _, window, cx| {
                this.begin_resend(&entry_snapshot, cx);
                if let Some(composer) = &this.composer {
                    composer.update(cx, |composer, cx| {
                        composer.set_text(&original, window, cx);
                    });
                }
            }))
            .child(Icon::new(IconName::Pencil).with_size(px(theme::ICON_SM)))
    });
    Some(
        h_flex()
            .gap(px(theme::SPACE_1))
            .mt(px(theme::SPACE_2))
            .debug_selector(|| format!("branch-bar-{ix}"))
            .child(branch_btn(super::service::BranchDir::Prev, IconName::ChevronLeft))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                    .text_color(cx.craftwave().colors.dim)
                    .child(format!("{}/{}", info.index + 1, info.count)),
            )
            .child(branch_btn(super::service::BranchDir::Next, IconName::ChevronRight))
            .children(resend_btn)
            .into_any_element(),
    )
}

// ======================================================================
// User：右对齐气泡（B5 右上折角 + 1px 边框）+ 图片三态缩略图
// ======================================================================

fn render_user(
    state: &ConversationState,
    entry: &craftwave_engine::storage::MessageEntry,
    ix: usize,
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    // owned 拷贝（ThemeColor/CraftwaveColors 都是 Copy）：不留对 cx 的借用
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let (muted, border, background) = (theme.muted, theme.border, theme.background);
    let mut bubble = div()
        .flex()
        .flex_col()
        .max_w(px(USER_BUBBLE_MAX_WIDTH))
        .bg(muted)
        .border_1()
        .border_color(border)
        // 圆角层级：首层面板 rounded-xl；右上角收小（B5 折角的形状暗示）
        .rounded(px(theme::rounded_xl(zoom)))
        .rounded_tr(px(theme::rounded_sm(zoom)))
        .px(px(theme::SPACE_5))
        .py(px(theme::SPACE_3))
        // 行高 1.85（rendering.md 钉值）；文字样式挂在气泡上，内容段直挂
        .text_size(px(theme::font_size(theme::FONT_UI_BASE, zoom)))
        .line_height(relative(theme::LEADING_USER))
        .text_color(theme.foreground)
        .relative()
        .overflow_hidden();
    // 内容段直挂气泡（无包装层——Wake 教训见模块注释）：Text part 是
    // markdown 段（段间距 0.5rem），Image part 是三态缩略图（2.9 转正）
    let mut image_ix = 0usize;
    for (part_ix, part) in entry.parts.iter().enumerate() {
        match part {
            craftwave_engine::storage::WirePart::Text { text } => {
                bubble = bubble.child(
                    markdown_body(
                        SharedString::from(format!("user-{ix}-{part_ix}")),
                        text,
                        rems(0.5),
                        cx,
                    )
                    .into_any_element(),
                );
            }
            craftwave_engine::storage::WirePart::Image { mime_type, .. } => {
                let this_image_ix = image_ix;
                image_ix += 1;
                let thumb = render_user_image(state, ix, this_image_ix, mime_type, cx);
                bubble = bubble.child(div().mt(px(theme::SPACE_2)).child(thumb));
            }
            // User 消息不产生其他 part（防御：忽略即不渲染）
            _ => {}
        }
    }
    // B5 右上折角：占位形制——角上 8px 方形翻角 chip（bg 同窗底色 + 1px 边框，
    // 视觉上像气泡右上角折起的一页）。gpui 无 div 级 rotate，45° 三角折角
    // 等 svg/路径能力接入后再精修；位置与尺寸不随内容变化。
    bubble = bubble.child(
        div()
            .absolute()
            .top(px(FOLD_CHIP_INSET))
            .right(px(FOLD_CHIP_INSET))
            .size(px(FOLD_CHIP_SIZE))
            .bg(background)
            .border_1()
            .border_color(border)
            .rounded(px(theme::rounded_sm(zoom) / 2.0)),
    );
    // 右对齐：气泡贴阅读轴右缘
    div().w_full().flex().justify_end().child(bubble).into_any_element()
}

/// User 消息里的单张图片：三态缩略图（images.rs 形制）——Ready 渲染图
/// （点击开 zoom）、Unsupported 明示预览不可用（有原始字节给下载原图）、
/// Omitted 明示转录上限；装载中给 mime 占位卡（不假装有图）。
fn render_user_image(
    state: &ConversationState,
    ix: usize,
    image_ix: usize,
    mime_type: &str,
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let id = SharedString::from(format!("message-image-{ix}-{image_ix}"));
    match state.image_state(ix, image_ix) {
        Some(ImageLoad::Ready { image, dims }) => images::render_ready_thumb(id.clone(), image, *dims, cx)
            .debug_selector(move || format!("image-{ix}-{image_ix}"))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(t("Zoom image")).build(window, cx)
            })
            .on_click(cx.listener(move |this: &mut ConversationState, _, _, cx| {
                this.open_zoom(ix, image_ix, cx);
            }))
            .into_any_element(),
        Some(ImageLoad::Unsupported { mime, original }) => {
            images::render_unsupported_thumb(id.clone(), mime, original.as_ref(), cx)
                .debug_selector(move || format!("image-{ix}-{image_ix}"))
                .into_any_element()
        }
        Some(ImageLoad::Omitted) => images::render_omitted_thumb(id.clone(), cx)
            .debug_selector(move || format!("image-{ix}-{image_ix}"))
            .into_any_element(),
        // 装载中：mime 占位卡（同 104 形制；不假装有图）
        None => {
            let theme = cx.theme().colors;
            let zoom = cx.craftwave().zoom;
            div()
                .id(id)
                .size(px(images::IMAGE_THUMB))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::rounded_md(zoom)))
                .border_1()
                .border_color(theme.border)
                .bg(theme.muted)
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(theme.muted_foreground)
                .overflow_hidden()
                .whitespace_nowrap()
                .truncate()
                .debug_selector(move || format!("image-{ix}-{image_ix}"))
                .child(mime_type.to_owned())
                .into_any_element()
        }
    }
}

// ======================================================================
// Assistant：全宽列 = thinking 面板 → 正文 → 工具簇（平铺）
// ======================================================================

fn render_assistant(
    state: &ConversationState,
    ix: usize,
    slots: Vec<Slot>,
    view: &StreamView,
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let mut col = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_3))
        // 正文行高 1.9（rendering.md 钉值）；thinking 展开区自己覆盖行高
        .line_height(relative(theme::LEADING_ASSISTANT));
    for slot in slots {
        match slot {
            Slot::Thinking { text } => {
                col = col.child(render_thinking_panel(state, ix, &text, view, cx))
            }
            Slot::Body { text } => col = col.child(render_body(ix, &text, cx)),
            Slot::Tools { calls } => col = col.child(render_tool_cluster(state, ix, &calls, cx)),
        }
    }
    col.into_any_element()
}

/// 正文槽位：`TextView::markdown` 四定制点（段间距 0.6rem）+ `.selectable(true)`
/// + 链接分流（见 [`markdown_body`]）。**永不截断**（纪律三）。
///
/// 外层 `w_full` div 只承载测试定位钩子（debug_selector）——Assistant 是
/// 全宽定宽列，不走 max-content 探测，加包装层安全（Wake 结论只约束靠
/// 内容撑宽的 User 气泡，那里的 markdown 段保持直挂）。
fn render_body(ix: usize, text: &str, cx: &mut Context<ConversationState>) -> AnyElement {
    let view = markdown_body(
        SharedString::from(format!("body-{ix}")),
        text,
        rems(0.6),
        cx,
    );
    div()
        .w_full()
        .debug_selector(move || format!("body-{ix}"))
        .child(view)
        .into_any_element()
}

/// 对话正文共用的 Markdown 视图构造（Wake `markdown_body` 行为照搬）：
/// 表格、引用块与分隔线由组件原生解析，本函数覆写四个定制点——
///
/// 1. **标题字号 = 基准 × 系数**（theme.rs §2 乘法体系：h1 1.45×/h2 1.28×/
///    h3 1.14×/h4 1.05×/h5+ 基准），基准随全局缩放档走（zoom）——标题克制、
///    天然支持全局缩放；
/// 2. 段间距参数化（user 0.5rem / assistant 0.6rem，调用方传）；
/// 3. 代码块一个 StyleRefinement（muted 底 + 1px border + 圆角）+
///    `code_block_actions`（语言名 + 复制钮，**复制走 gpui clipboard 写
///    完整原文**——600 字符契约的代码块版）；
/// 4. `.selectable(true)` 全文可选。
///
/// 主题细节：**主题写进元素 id**（`{id}-dark/-light`）——TextView 的语法
/// 高亮色在解析期固化，id 带主题让亮暗切换走同步重解析，代码块不闪色。
/// 链接点击走 [`markdown_links::with_session_links`] 分流（项目根接线随
/// 阶段 3.3 会话树接入，本批传空 = 相对路径不解析，绝对路径 / file: URI /
/// `~` 路径照常工作）。
fn markdown_body(id: SharedString, text: &str, paragraph_gap: gpui_kit::Rems, cx: &App) -> TextView {
    let theme = cx.theme().colors;
    let dark = cx.theme().is_dark();
    let zoom = cx.craftwave().zoom;
    let code_radius = px(theme::rounded_lg(zoom));
    // 语法高亮色解析期固化：主题进 id（见函数注释）
    let themed_id: SharedString = format!("{id}-{}", if dark { "dark" } else { "light" }).into();
    let view = TextView::markdown(themed_id, text.to_owned())
        .style(
            TextViewStyle {
                // 标题基准 = 全局字号基准（rem 源头），系数在 heading_font_size 闭包
                heading_base_font_size: px(theme::base_font_size(zoom)),
                paragraph_gap,
                is_dark: dark,
                highlight_theme: if dark {
                    HighlightTheme::default_dark()
                } else {
                    HighlightTheme::default_light()
                },
                ..Default::default()
            }
            .heading_font_size(move |level, _base| match level {
                1 => px(theme::markdown_heading(theme::MD_H1, zoom)),
                2 => px(theme::markdown_heading(theme::MD_H2, zoom)),
                3 => px(theme::markdown_heading(theme::MD_H3, zoom)),
                4 => px(theme::markdown_heading(theme::MD_H4, zoom)),
                // h5+ 回到正文基准（rendering.md 系数表）
                _ => px(theme::base_font_size(zoom)),
            })
            // 代码块容器：muted 底 + 1px 边框 + 圆角（色从主题取，切主题随窗刷新重取）
            .code_block(
                StyleRefinement::default()
                    .bg(theme.muted)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(code_radius),
            ),
        )
        // 代码块角落的操作区：语言名 + 复制完整原文（ClipboardItem 走 gpui
        // clipboard，不经系统命令）
        .code_block_actions(move |block, _window, cx| {
            let theme = cx.theme().colors;
            let zoom = cx.craftwave().zoom;
            let code = block.code();
            h_flex()
                .items_center()
                .gap(px(theme::SPACE_2))
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(theme.muted_foreground)
                .when_some(block.lang(), |actions, lang| actions.child(lang))
                .child(
                    div()
                        .id("code-copy")
                        .px(px(theme::SPACE_1))
                        .rounded(px(theme::rounded_sm(zoom)))
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.list_hover))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(t("Copy code"))
                                .build(window, cx)
                        })
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(code.to_string()));
                        })
                        .child(t("Copy")),
                )
        })
        .selectable(true);
    markdown_links::with_session_links(view, "")
}

// ---------------------------------------------------------------------
// thinking 面板（2.7 转正）：折叠行 = chevron 图标（展开 rotate 90°）+
// "Thinking" + 单行斜体摘要；展开为左缩进 + 左导线纯文本（纪律一，默认
// 折叠，展开状态跨重渲染记忆）。流式期间走 B4：流动摘要（最后一行吸尾）
// + 思考计时（秒针任务驱动，reduce_motion 只降级动效不降级计时）。
// ---------------------------------------------------------------------

/// thinking 面板（Wake thinking_panel + ZCode B4 形态合流）。
fn render_thinking_panel(
    state: &ConversationState,
    ix: usize,
    text: &str,
    view: &StreamView,
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    // owned 拷贝：后面的 cx.listener 需要独占 cx
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let muted_foreground = theme.muted_foreground;
    let expanded = state.thinking_expanded(ix);
    // B4 流式态：run 未收口且本行带思考段（折叠头给流动摘要与计时）
    let streaming = view.live && view.thinking_span.is_some();
    // 折叠头的摘要：
    // - 流式折叠 → 最后一行非空文本、吸尾贴右（新行到达时尾部始终可见，
    //   ZCode「摘要行滚动显示最后一行」的 gpui 等效实现）；
    // - 其余 → one_line(400) 压行 + 88 格显示宽截断（rendering.md 钉值）
    let tail_summary = streaming && !expanded;
    let summary = if tail_summary {
        last_nonempty_line(text).to_owned()
    } else {
        clip_display(&one_line(text, 400), THINKING_SUMMARY_WIDTH)
    };
    // 思考计时（B4）：流式开着算到当下（秒针任务每秒重渲）；已收口取
    // seal 时长。无思考段的行（历史）不显示计时。
    let timer = view.thinking_span.as_ref().map(|span| {
        let seconds = match span.ended_at {
            Some(ended) => ended.duration_since(span.started_at).as_secs(),
            None if view.live => span.started_at.elapsed().as_secs(),
            None => 0,
        };
        thinking_duration_text(seconds)
    });
    let subtlest = cx.craftwave().colors.foreground_subtlest;
    let mut head = div()
        .id(("thinking-head", ix))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_2))
        .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
        .text_color(muted_foreground)
        .cursor_pointer()
        // 测试定位钩子（仅 test-support 生效，生产零成本 no-op）
        .debug_selector(move || format!("thinking-head-{ix}"))
        .on_click(cx.listener(move |this: &mut ConversationState, _, _, cx| {
            this.toggle_thinking(ix, cx);
        }))
        // chevron：展开 rotate 90°（真图标 + Icon::rotate 变换，2.C-1 的
        // ▸/▾ 字符占位就此退役）
        .child(
            Icon::new(IconName::ChevronRight)
                .with_size(px(theme::ICON_SM))
                .text_color(muted_foreground)
                .flex_shrink_0()
                .when(expanded, |icon| {
                    icon.rotate(Radians(std::f32::consts::FRAC_PI_2))
                }),
        );
    // 标签：流式折叠给流动摘要标签（扫光由 ShimmerText 承担，reduce_motion
    // 时组件内建降级为静态文本）；其余给静态标签
    head = if tail_summary {
        head.child(
            ShimmerText::new(t("Thinking…"))
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .text_color(muted_foreground)
                .flex_shrink_0(),
        )
    } else {
        head.child(
            div()
                .flex_shrink_0()
                .text_color(muted_foreground)
                .child(t("Thinking")),
        )
    };
    if let Some(timer) = timer {
        head = head.child(
            div()
                .flex_shrink_0()
                .text_color(subtlest)
                .child(timer),
        );
    }
    head = head.child(if tail_summary {
        // 流动摘要：justify_end 让溢出内容向左溢出——可见部分永远是最后一
        // 行的尾部（吸尾），等效旧栈 scrollLeft = scrollWidth 的吸底滚动
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .justify_end()
            .child(
                div()
                    .whitespace_nowrap()
                    .italic()
                    .child(summary),
            )
    } else {
        div()
            .flex_1()
            .min_w_0()
            .italic()
            .overflow_hidden()
            .child(summary)
    });
    if !expanded {
        return head.into_any_element();
    }
    // 展开体：左缩进 30px（钉值）+ 左导线（B4）+ 11.5px 小字号、muted、
    // 行高 1.7、纯文本（不用 Markdown——性能自觉）；视觉降级完整，正文是主角。
    // 3.8 查找开着时命中词给词级淡 primary 底（普通文本段的高亮路径）
    let hit_bg = cx.craftwave().colors.find_hit;
    let query = super::find::active_query(state).map(str::to_owned);
    div()
        .w_full()
        .flex()
        .flex_col()
        .child(head)
        .child(
            div()
                .ml(px(THINKING_INDENT))
                .mt(px(theme::SPACE_1))
                .pl(px(theme::SPACE_3))
                .border_l_1()
                .border_color(theme.border)
                .text_size(px(theme::font_size(theme::FONT_UI_THINKING, zoom)))
                .text_color(muted_foreground)
                .line_height(relative(theme::LEADING_THINKING))
                .child(super::find::highlighted_text(
                    text.to_owned(),
                    query.as_deref(),
                    hit_bg,
                )),
        )
        .into_any_element()
}

/// 思考时长文案：最少 1 秒（旧栈 Math.ceil 口径），跨分钟带分（"1分5秒"）。
pub(crate) fn thinking_duration_text(seconds: u64) -> String {
    let seconds = seconds.max(1);
    if seconds < 60 {
        format!("{}{}", seconds, t("sec"))
    } else {
        format!("{}{}{}{}", seconds / 60, t("min"), seconds % 60, t("sec"))
    }
}

/// 最后一行非空文本（B4 流动摘要的取材；全空回落空串）。
pub(crate) fn last_nonempty_line(text: &str) -> &str {
    text.trim_end()
        .split('\n')
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

// ---------------------------------------------------------------------
// 工具簇卡（2.8 转正）：每条消息一张簇卡（纪律一）。折叠头 = chevron +
// 工具名（单）/ "N tool calls"（多）+ 等宽参数摘要（宽度自适应格数）+
// 跳转占位钮 + 失败数红 badge；展开区 = border-t 分隔，每调用
// Input/Output 分节（600 字符契约 + 完整复制 + 失败 danger 面）。
// ---------------------------------------------------------------------

/// 参数摘要的「宽度自适应格数」：`(阅读区宽 - 预留px) / 等宽字符宽`
/// （Wake 7103-7104 手法；阅读区宽是 Render 每帧实测的中栏宽——
/// `ConversationState::reader_width_seen`）。
fn tool_arg_cells(reader_width: f32, zoom: f32) -> usize {
    let cell = MONO_CELL_EM * theme::font_size(theme::FONT_UI_SM, zoom);
    let budget = (reader_width - TOOL_HEAD_RESERVE).max(80.0);
    ((budget / cell) as usize).max(8)
}

/// 折叠头的参数摘要：单工具给参数单行（one_line 压行 + 按格数截断）；
/// 多工具给工具名串（Wake tool_cluster_heading 同款）。空参数不出摘要。
fn tool_head_arg(calls: &[CallLite], arg_cells: usize) -> Option<String> {
    match calls {
        [] => None,
        [only] => {
            let arg = one_line(&only.args_json, 400);
            (!arg.is_empty()).then(|| clip_display(&arg, arg_cells))
        }
        many => {
            let names = many
                .iter()
                .map(|call| call.name.as_str())
                .collect::<Vec<_>>()
                .join(" · ");
            Some(clip_display(&names, arg_cells))
        }
    }
}

/// Input/Output 的完整展示原文：摘要与错误载荷都非空时换行拼接
/// （B3「失败原因可复制」的完整原文 = 渲染面持有的全部文本）。
fn tool_output_text(call: &CallLite) -> String {
    match (call.summary.as_str(), &call.error_text) {
        (summary, None) => summary.to_owned(),
        ("", Some(error)) => error.clone(),
        (summary, Some(error)) => format!("{summary}\n{error}"),
    }
}

/// 等宽面（工具簇的等宽元素共用）：等宽族 + 字号档（ui-sm）。
/// `tool_section` 的参数束——8 参收 7（clippy too_many_arguments）。
struct MonoFace {
    family: SharedString,
    zoom: f32,
}

impl MonoFace {
    fn new(family: &SharedString, zoom: f32) -> Self {
        Self {
            family: family.clone(),
            zoom,
        }
    }

    fn text_size(&self) -> gpui_kit::Pixels {
        px(theme::font_size(theme::FONT_UI_SM, self.zoom))
    }
}

/// 等宽数据面（工具参数/输出体共用）：ui-sm 档字号 + 等宽族。
/// 内容收 `IntoElement`——普通串与带词级高亮的 [`super::find::highlighted_text`]
/// 元素都能进。
fn mono_text(
    text: impl IntoElement,
    face: &MonoFace,
    color: gpui_kit::Hsla,
) -> gpui_kit::Div {
    div()
        .w_full()
        .min_w_0()
        .whitespace_normal()
        .text_size(face.text_size())
        .font_family(face.family.clone())
        .text_color(color)
        .child(text)
}

fn render_tool_cluster(
    state: &ConversationState,
    ix: usize,
    calls: &[CallLite],
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    // owned 拷贝：后面的 cx.listener 需要独占 cx
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let face = MonoFace::new(&cx.theme().mono_font_family, zoom);
    let expanded = state.tools_expanded(ix);
    let failed = calls.iter().filter(|c| c.ok == Some(false)).count();
    let arg_cells = tool_arg_cells(state.reader_width_seen, zoom);
    // 折叠头文案：单工具直接给名字，多工具 "N tool calls"
    let title = if calls.len() == 1 {
        calls[0].name.clone()
    } else {
        format!("{} {}", calls.len(), t("tool calls"))
    };
    // 派生了子代理：折叠头给跳转钮（多派时展开区逐调用给，头上的钮只在
    // 其恰好一派时显示，Wake open_child_button 语义）；目标 = call 的
    // sub_id，点击开详情视图（3.6 接线）
    let subagent_calls: Vec<(usize, String)> = calls
        .iter()
        .enumerate()
        .filter_map(|(call_ix, call)| {
            call.sub_id
                .clone()
                .map(|sub_id| (call_ix, sub_id))
        })
        .collect();
    let head_jump = match (&expanded, subagent_calls.as_slice()) {
        (false, [(_, sub_id)]) => Some(sub_id.clone()),
        _ => None,
    };
    let mut head = div()
        .id(("tools-head", ix))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_2))
        .px(px(theme::SPACE_3))
        .py(px(theme::SPACE_2))
        .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
        .text_color(theme.muted_foreground)
        .cursor_pointer()
        .debug_selector(move || format!("tools-head-{ix}"))
        .on_click(cx.listener(move |this: &mut ConversationState, _, _, cx| {
            this.toggle_tools(ix, cx);
        }))
        .child(
            Icon::new(IconName::ChevronRight)
                .with_size(px(theme::ICON_SM))
                .text_color(theme.muted_foreground)
                .flex_shrink_0()
                .when(expanded, |icon| {
                    icon.rotate(Radians(std::f32::consts::FRAC_PI_2))
                }),
        )
        .child(
            div()
                .flex_shrink_0()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.foreground)
                .child(title),
        );
    // 等宽参数摘要（宽度自适应格数；有摘要才占位，无则给弹性格）
    match tool_head_arg(calls, arg_cells) {
        Some(arg) => head = head.child(mono_text(arg, &face, theme.muted_foreground).flex_1()),
        None => head = head.child(div().flex_1()),
    }
    if let Some(head_sub_id) = head_jump {
        head = head.child(
            div()
                .id(("tool-open-subagent", ix))
                .debug_selector(move || format!("tool-open-subagent-{ix}"))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px(px(theme::SPACE_1))
                .py(px(theme::SPACE_1) / 2.0)
                .rounded(px(theme::rounded_sm(zoom)))
                .cursor_pointer()
                .text_color(theme.muted_foreground)
                .hover(|style| style.bg(theme.list_hover))
                .tooltip(|window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(t("Open subagent")).build(window, cx)
                })
                // 跳转钮（3.6 接线）：开子代理详情视图。stop_propagation
                // 不透传给折叠头误触开合
                .on_click(cx.listener(move |this: &mut ConversationState, _, _, cx| {
                    this.open_sub_view(head_sub_id.clone(), cx);
                }))
                .child(
                    Icon::new(IconName::CornerDownRight).with_size(px(theme::ICON_SM)),
                ),
        );
    }
    if failed > 0 {
        // 失败数红 badge（"2 failed"）：danger 淡底 + danger 字（Wake 9885 同款）
        head = head.child(
            div()
                .flex_shrink_0()
                .px(px(theme::SPACE_2))
                .rounded(px(theme::ROUNDED_FULL))
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(theme.danger)
                .bg(theme.danger.opacity(0.12))
                .child(format!("{} {}", failed, t("failed"))),
        );
    }
    // 卡壳：rounded-lg 行卡 + muted 淡底；展开区 border-t 分隔
    let mut card = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .rounded(px(theme::rounded_lg(zoom)))
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted.opacity(0.35))
        .child(head);
    if expanded {
        card = card.child(render_tool_cluster_body(state, ix, calls, &face, cx));
    }
    card.into_any_element()
}

/// 展开区：每调用 Input/Output 分节。600 字符契约（纪律三）：展示前 600
/// 字符 + 明示 "First 600 characters" + 复制按钮永远复制完整原文；错误
/// Output 用 danger 淡底 + danger 字色；长报错展开后自然增高（不嵌套滚动
/// 容器）。运行中的调用给扫光态（B3：不用 spinner）。
fn render_tool_cluster_body(
    state: &ConversationState,
    ix: usize,
    calls: &[CallLite],
    face: &MonoFace,
    cx: &mut Context<ConversationState>,
) -> gpui_kit::Div {
    let theme = cx.theme().colors;
    let zoom = face.zoom;
    let arg_cells = tool_arg_cells(state.reader_width_seen, zoom);
    // 3.8 词级命中高亮的参数（查询词 + 命中底色）；查找关着时 None
    let find_tint = super::find::active_query(state)
        .map(|query| (query.to_owned(), cx.craftwave().colors.find_hit));
    let mut body = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .border_t_1()
        .border_color(theme.border);
    for (call_ix, call) in calls.iter().enumerate() {
        // 多调用：给调用名行（名 + mono 参数摘要）；单调用的名已在折叠头
        let mut item = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .px(px(theme::SPACE_3))
            .py(px(theme::SPACE_2))
            .gap(px(theme::SPACE_1))
            .when(call_ix > 0, |item| {
                item.border_t_1().border_color(theme.border)
            });
        if calls.len() > 1 {
            let arg = one_line(&call.args_json, 400);
            item = item.child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap(px(theme::SPACE_2))
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                            .text_color(if call.ok == Some(false) {
                                theme.danger
                            } else {
                                theme.foreground
                            })
                            .child(call.name.clone()),
                    )
                    .child(
                        mono_text(clip_display(&arg, arg_cells), face, theme.muted_foreground)
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap(),
                    ),
            );
        }
        if let Some(sub_id) = &call.sub_id {
            // 子代理摘要行（3.6）：状态徽标（运行中扫光）+ turn 计数 +
            // 跳转钮 + 运行中停止钮——委派在对话流的内联呈现
            item = item.child(render_subagent_summary_line(
                state, ix, call_ix, sub_id, face, cx,
            ));
        }
        // Input 分节（参数 JSON 非空白才给——空白只参与「有无」判断，
        // 展示与复制保留原始字节）
        if !call.args_json.trim().is_empty() {
            item = item.child(tool_section(
                SharedString::from(format!("tool-copy-{ix}-{call_ix}-input")),
                t("Input"),
                t("Copy full input"),
                &call.args_json,
                false,
                face,
                find_tint.clone(),
                cx,
            ));
        }
        // Output 分节：结果在场才给；失败 = danger 面（B3 失败原因可复制：
        // 复制钮复制完整原文 = 摘要 + 错误载荷）
        let output = tool_output_text(call);
        match call.ok {
            Some(ok) if !output.trim().is_empty() => {
                item = item.child(tool_section(
                    SharedString::from(format!("tool-copy-{ix}-{call_ix}-output")),
                    t("Output"),
                    t("Copy full output"),
                    &output,
                    !ok,
                    face,
                    find_tint.clone(),
                    cx,
                ));
            }
            // 仍在执行：扫光态文案（B3：不用 spinner；reduce_motion 时
            // ShimmerText 组件内建降级为静态文本）
            None => {
                item = item.child(
                    div()
                        .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                        .text_color(theme.muted_foreground)
                        .child(ShimmerText::new(t("Running…")).text_size(px(
                            theme::font_size(theme::FONT_UI_SM, zoom),
                        ))),
                );
            }
            Some(_) => {}
        }
        body = body.child(item);
    }
    body
}

/// 子代理摘要行（3.6 呈现面，工具簇展开区内每个委派调用一行）：CornerDownRight 图标 + 角色 + 状态徽标（运行中扫光 / 四象限着色终态）+ turn 计数 + 「打开子代理」跳转 + 运行中「停止」。子代理 id 不在已知集合（帧面与事件表都未达——极窄窗口）时按纯跳转钮渲染。
fn render_subagent_summary_line(
    state: &ConversationState,
    ix: usize,
    call_ix: usize,
    sub_id: &str,
    face: &MonoFace,
    cx: &mut Context<ConversationState>,
) -> gpui_kit::Div {
    let theme = cx.theme().colors;
    let zoom = face.zoom;
    let row = state.sub_row_of(sub_id);
    let running = row
        .as_ref()
        .is_none_or(|row| row.state == craftwave_engine::snapshot::SubRunState::Running);
    let mut line = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_2))
        .px(px(theme::SPACE_2))
        .py(px(theme::SPACE_1) / 2.0)
        .rounded(px(theme::rounded_sm(zoom)))
        .bg(theme.muted.opacity(0.5))
        .debug_selector(move || format!("subagent-line-{ix}-{call_ix}"))
        .child(
            Icon::new(IconName::CornerDownRight)
                .with_size(px(theme::ICON_SM))
                .text_color(theme.muted_foreground)
                .flex_shrink_0(),
        );
    // 角色徽标（未知时给「subagent」兜底文案）
    line = line.child(
        div()
            .flex_shrink_0()
            .px(px(theme::SPACE_2))
            .rounded(px(theme::rounded_sm(zoom)))
            .bg(theme.accent)
            .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.foreground)
            .child(
                row.as_ref()
                    .map(|row| row.role.clone())
                    .unwrap_or_else(|| "subagent".to_owned()),
            ),
    );
    // 状态徽标：运行中扫光（B3 不用 spinner）/ 终态四象限着色
    line = line.child(match &row {
        Some(row) if row.state != craftwave_engine::snapshot::SubRunState::Running => {
            let color = match row.state {
                craftwave_engine::snapshot::SubRunState::Done(
                    craftwave_engine::backend::frames::SubOutcome::Report,
                ) => theme.success,
                craftwave_engine::snapshot::SubRunState::Done(
                    craftwave_engine::backend::frames::SubOutcome::Partial,
                ) => theme.warning,
                craftwave_engine::snapshot::SubRunState::Done(
                    craftwave_engine::backend::frames::SubOutcome::Cancelled,
                ) => theme.muted_foreground,
                _ => theme.danger,
            };
            div()
                .flex_shrink_0()
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(color)
                .child(row.state_label())
        }
        _ => div().flex_shrink_0().child(
            ShimmerText::new(t("Running"))
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(theme.muted_foreground),
        ),
    });
    // turn 计数（已知时）
    if let Some(row) = &row {
        line = line.child(
            div()
                .flex_shrink_0()
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(theme.muted_foreground)
                .child(crate::i18n::t("turn N").replace("{n}", &row.turn.to_string())),
        );
    }
    line = line.child(div().flex_1());
    // 跳转钮
    let jump_target = sub_id.to_owned();
    line = line.child(
        div()
            .id(SharedString::from(format!(
                "subagent-line-open-{ix}-{call_ix}"
            )))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(theme::SPACE_1))
            .px(px(theme::SPACE_1))
            .rounded(px(theme::rounded_sm(zoom)))
            .cursor_pointer()
            .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
            .text_color(theme.muted_foreground)
            .hover(|style| style.bg(theme.list_hover).text_color(theme.foreground))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(t("Open subagent")).build(window, cx)
            })
            .on_click(cx.listener(move |this: &mut ConversationState, _, _, cx| {
                this.open_sub_view(jump_target.clone(), cx);
            }))
            .child(t("Open subagent")),
    );
    // 运行中停止钮（接 engine stop_subagent）
    if running {
        let stop_target = sub_id.to_owned();
        line = line.child(
            div()
                .id(SharedString::from(format!(
                    "subagent-line-stop-{ix}-{call_ix}"
                )))
                .flex_shrink_0()
                .px(px(theme::SPACE_2))
                .py(px(theme::SPACE_1) / 2.0)
                .rounded(px(theme::rounded_sm(zoom)))
                .cursor_pointer()
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(theme.danger)
                .hover(|style| style.bg(theme.danger.opacity(0.08)))
                .debug_selector(move || format!("subagent-line-stop-{ix}-{call_ix}"))
                .on_click(cx.listener(move |this: &mut ConversationState, _, _, cx| {
                    super::subagent::stop_subagent(this, &stop_target, cx);
                }))
                .child(t("Stop")),
        );
    }
    line
}


/// 展示 = 前 600 字符 + 截断时明示 "First 600 characters"；复制按钮永远
/// 写完整原文进剪贴板；错误面 danger 淡底 + danger 字色；正文 whitespace
/// normal 自然增高（不嵌套滚动容器抢主阅读区滚轮）。3.8 查找开着时正文
/// 命中词给词级淡 primary 底（`find` 携带查询词与命中底色）。
#[allow(clippy::too_many_arguments)] // 分节契约本就有七参，3.8 只加 find 一参
fn tool_section(
    copy_id: SharedString,
    label: &'static str,
    copy_tooltip: &'static str,
    body: &str,
    is_error: bool,
    face: &MonoFace,
    find: Option<(String, gpui_kit::Hsla)>,
    cx: &App,
) -> gpui_kit::Div {
    let theme = cx.theme().colors;
    let (shown, truncated) = clip_tool_text(body, TOOL_TEXT_PREVIEW_LIMIT);
    let full = body.to_string();
    let copy_id_debug = copy_id.clone();
    let accent = if is_error {
        theme.danger
    } else {
        theme.muted_foreground
    };
    div().w_full().min_w_0().flex().flex_col().gap(px(theme::SPACE_1))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap(px(theme::SPACE_2))
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, face.zoom)))
                .text_color(accent)
                .child(label)
                .when(truncated, |header| {
                    header.child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(t("First 600 characters")),
                    )
                })
                .child(div().flex_1())
                .child(
                    div()
                        .id(copy_id.clone())
                        .debug_selector(move || copy_id_debug.to_string())
                        .flex_shrink_0()
                        .px(px(theme::SPACE_1))
                        .rounded(px(theme::rounded_sm(face.zoom)))
                        .cursor_pointer()
                        .text_color(theme.muted_foreground)
                        .hover(|style| {
                            style.bg(theme.list_hover).text_color(theme.foreground)
                        })
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(copy_tooltip)
                                .build(window, cx)
                        })
                        // 契约核心：永远复制完整原文（shown 只是预览）
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(full.clone()));
                        })
                        .child(Icon::new(IconName::Copy).with_size(px(theme::ICON_SM))),
                ),
        )
        .child(match &find {
            Some((query, hit_bg)) => mono_text(
                super::find::highlighted_text(shown, Some(query), *hit_bg),
                face,
                accent,
            )
            .px(px(theme::SPACE_3))
            .py(px(theme::SPACE_2))
            .rounded(px(theme::rounded_sm(face.zoom)))
            .bg(if is_error {
                theme.danger.opacity(0.08)
            } else {
                theme.muted
            }),
            None => mono_text(shown, face, accent)
                .px(px(theme::SPACE_3))
                .py(px(theme::SPACE_2))
                .rounded(px(theme::rounded_sm(face.zoom)))
                .bg(if is_error {
                    theme.danger.opacity(0.08)
                } else {
                    theme.muted
                }),
        })
}

/// 前 N 个**字符**的预览截断（Wake clip_tool_text 照搬：纪律三的 600 字符
/// 按 Unicode 字符计，不是显示单元宽——显示单元宽截断归元数据摘要
/// clip_display 的职责）。
fn clip_tool_text(text: &str, limit: usize) -> (String, bool) {
    let mut chars = text.chars();
    let shown: String = chars.by_ref().take(limit).collect();
    let truncated = chars.next().is_some();
    (
        if truncated {
            format!("{shown}…")
        } else {
            shown
        },
        truncated,
    )
}

// ======================================================================
// System / CompactSummary：居中小胶囊（单行截断——只有元数据可截断）
// ======================================================================

fn render_system(entry: &craftwave_engine::storage::MessageEntry, cx: &mut App) -> AnyElement {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    // 正文取全部 Text part 拼接（压缩摘要即纯文本）
    let text: String = entry
        .parts
        .iter()
        .filter_map(|p| match p {
            craftwave_engine::storage::WirePart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    div()
        .w_full()
        .flex()
        .justify_center()
        .child(
            div()
                .max_w(px(READER_MAX_WIDTH))
                .bg(theme.muted)
                .rounded(px(theme::rounded_md(zoom)))
                .px(px(theme::SPACE_4))
                .py(px(theme::SPACE_1))
                .text_size(px(theme::font_size(theme::FONT_UI_CAPTION, zoom)))
                .text_color(cx.craftwave().colors.foreground_subtlest)
                // 纪律二：one_line 压成单行（多段摘要折成一行），宽度截断由
                // .truncate() 按容器兜底
                .whitespace_nowrap()
                .truncate()
                .overflow_hidden()
                .child(one_line(&text, PILL_MAX_CHARS)),
        )
        .into_any_element()
}
