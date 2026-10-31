//! 行分发四形态的元素构建（gpui-rewrite 2.4 / 2.C-3，rendering.md「行分发」
//! 「Markdown 正文」逐条）。
//!
//! 判定与路由在 state.rs（`render_row` 入口），本模块只产元素：
//!
//! - **User**：右对齐气泡，`max_w(540px)`、muted 底、行高 1.85、内容段
//!   （`TextView::markdown`，段间距 0.5rem）直挂气泡不加包装层（max-content
//!   探测问题的 Wake 结论）；B5 右上折角 + 1px 边框（折角本批为占位形制——
//!   方形翻角 chip，gpui 无 div 级 rotate，三角折角待有 svg/路径面后精修）；
//! - **Assistant**：全宽列 = thinking 面板槽位 → 正文槽位（`TextView::markdown`
//!   四定制点，段间距 0.6rem）→ 工具簇槽位，平铺；
//! - **System / CompactSummary**：居中小胶囊（one_line 压行 + `.truncate()`
//!   宽度兜底）；
//! - **过滤器**：运行日志型 → state.rs 已拦截为零高空行，不会进到本模块。
//!
//! 纪律对照：折叠不嵌套（只有 thinking / 工具簇两个槽位可折）；截断只发生在
//! 元数据（胶囊文本、thinking 摘要、工具参数摘要），正文槽位永不截断。

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::component::text::{TextView, TextViewStyle};
use gpui_kit::component::{ActiveTheme as _, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, SharedString, StyleRefinement, div, px, relative,
    rems,
};

use crate::i18n::t;
use crate::text_util::{clip_display, one_line};
use crate::theme::{self, ActiveCraftwave as _};

use super::markdown_links;
use super::state::{CallLite, ConversationState, READER_MAX_WIDTH, Slot, USER_BUBBLE_MAX_WIDTH};

// 本批占位形制的两个长度（折角 chip、缩进导线），均有注释说明去向。
/// thinking 展开区的左缩进（rendering.md「thinking 面板」钉值 30px）。
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
/// 图片缩略占位边长（rendering.md「图片」钉值 104px；按宽高比分流下一批）。
const IMAGE_THUMB: f32 = 104.0;

/// 行渲染总入口：角色分发 + 阅读轴包装（`justify_center + max_w` 共用竖直轴，
/// 首条 pt / 末条 pb 呼吸边距）。
pub(crate) fn render_row(
    state: &ConversationState,
    ix: usize,
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let zoom = cx.craftwave().zoom;
    // 角色分发（流式行按 Assistant 形态走三槽位列）
    let content: AnyElement = if ix >= state.messages().len() {
        match state.streaming() {
            Some(streaming) => {
                render_assistant(state, ix, super::state::slots_from_stream(&streaming.parts), cx)
            }
            None => div().into_any_element(),
        }
    } else {
        let entry = &state.messages()[ix];
        match entry.role {
            craftwave_engine::storage::MessageRole::User => render_user(entry, ix, cx),
            craftwave_engine::storage::MessageRole::Assistant => {
                render_assistant(state, ix, super::state::slots_from_wire(&entry.parts), cx)
            }
            craftwave_engine::storage::MessageRole::System => render_system(entry, cx),
        }
    };
    // 阅读轴：全消息共用同一竖直轴；首条 pt / 末条 pb 呼吸边距
    let last = ix + 1 == state.row_count();
    div()
        .w_full()
        .flex()
        .justify_center()
        .when(ix == 0, |row| row.pt(px(theme::SPACE_5)))
        .when(last, |row| row.pb(px(theme::SPACE_6)))
        .child(
            div()
                .w_full()
                .max_w(px(READER_MAX_WIDTH))
                .text_size(px(theme::font_size(theme::FONT_UI_BASE, zoom)))
                .text_color(cx.theme().foreground)
                .child(content),
        )
        .into_any_element()
}

// ======================================================================
// User：右对齐气泡（B5 右上折角 + 1px 边框）
// ======================================================================

fn render_user(
    entry: &craftwave_engine::storage::MessageEntry,
    ix: usize,
    cx: &mut App,
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
    // markdown 段（段间距 0.5rem），Image part 是块级占位（缩略图三态是
    // 2.9 的事，本批只占位）
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
                bubble = bubble.child(
                    div()
                        .mt(px(theme::SPACE_2))
                        .size(px(IMAGE_THUMB))
                        .rounded(px(theme::rounded_md(zoom)))
                        .bg(background)
                        .border_1()
                        .border_color(border)
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                        .text_color(theme.muted_foreground)
                        .child(mime_type.clone()),
                );
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

// ======================================================================
// Assistant：全宽列 = thinking 面板 → 正文 → 工具簇（平铺）
// ======================================================================

fn render_assistant(
    state: &ConversationState,
    ix: usize,
    slots: Vec<Slot>,
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
                col = col.child(render_thinking_panel(state, ix, &text, cx))
            }
            Slot::Body { text } => col = col.child(render_body(ix, &text, cx)),
            Slot::Tools { calls } => {
                col = col.child(render_tool_cluster(state, ix, &calls, cx))
            }
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
// thinking 面板（折叠行：chevron + "Thinking" + 单行斜体摘要；展开为
// 左导线纯文本——纪律一，默认折叠，展开状态跨重渲染记忆）
// ---------------------------------------------------------------------

fn render_thinking_panel(
    state: &ConversationState,
    ix: usize,
    text: &str,
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    // owned 拷贝：后面的 cx.listener 需要独占 cx
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let expanded = state.thinking_expanded(ix);
    // 摘要两级截断（rendering.md 钉值）：one_line(400) 压成一行再按显示
    // 单元宽度截到 88 格（CJK 记 2，字素簇不劈半）
    let summary = clip_display(&one_line(text, 400), THINKING_SUMMARY_WIDTH);
    let muted_foreground = theme.muted_foreground;
    // 折叠行：chevron（▸/▾ 表达旋转 90° 语义；图标面接入后换 Icon+rotate）
    let head = div()
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
        .child(if expanded { "▾" } else { "▸" })
        .child(t("Thinking"))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .italic()
                .overflow_hidden()
                .child(summary),
        );
    if !expanded {
        return head.into_any_element();
    }
    // 展开体：左缩进 30px、11.5px 小字号、muted、行高 1.7、纯文本（不用
    // Markdown——性能自觉）；视觉降级完整，正文是主角
    div()
        .w_full()
        .flex()
        .flex_col()
        .child(head)
        .child(
            div()
                .ml(px(THINKING_INDENT))
                .mt(px(theme::SPACE_1))
                .text_size(px(theme::font_size(theme::FONT_UI_THINKING, zoom)))
                .text_color(muted_foreground)
                .line_height(relative(theme::LEADING_THINKING))
                
                .child(text.to_owned()),
        )
        .into_any_element()
}

// ---------------------------------------------------------------------
// 工具簇卡（每条消息一张簇卡；折叠头 + Input/Output 分节展开——纪律一）
// ---------------------------------------------------------------------

fn render_tool_cluster(
    state: &ConversationState,
    ix: usize,
    calls: &[CallLite],
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    // owned 拷贝：后面的 cx.listener 需要独占 cx
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let expanded = state.tools_expanded(ix);
    let failed = calls.iter().filter(|c| c.ok == Some(false)).count();
    // 折叠头文案：单工具直接给名字，多工具 "N tool calls"
    let title = if calls.len() == 1 {
        calls[0].name.clone()
    } else {
        format!("{} {}", calls.len(), t("tool calls"))
    };
    let mut head = div()
        .id(("tools-head", ix))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_2))
        .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
        .text_color(theme.muted_foreground)
        .cursor_pointer()
        .debug_selector(move || format!("tools-head-{ix}"))
        .on_click(cx.listener(move |this: &mut ConversationState, _, _, cx| {
            this.toggle_tools(ix, cx);
        }))
        .child(if expanded { "▾" } else { "▸" })
        .child(title);
    if failed > 0 {
        // 失败数红 badge（"2 failed"）——B3 数据面先立，样式精修随 2.8
        head = head.child(
            div()
                .px(px(theme::SPACE_2))
                .rounded(px(theme::ROUNDED_FULL))
                .bg(theme.danger)
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(theme.danger_foreground)
                .child(format!("{} {}", failed, t("failed"))),
        );
    }
    if !expanded {
        return head.into_any_element();
    }
    // 展开区：border-t 分隔，每调用 Input/Output 分节。600 字符契约与
    // 完整复制按钮是 2.8 的活；本批参数摘要单行截断（元数据可截断）
    let mut body = div()
        .w_full()
        .flex()
        .flex_col()
        .mt(px(theme::SPACE_2))
        .pt(px(theme::SPACE_2))
        .border_t_1()
        .border_color(theme.border)
        .gap(px(theme::SPACE_3));
    for call in calls {
        body = body.child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_1))
                .child(
                    div()
                        .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                        .text_color(theme.foreground)
                        .child(call.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                        .text_color(theme.muted_foreground)
                        .overflow_hidden()
                        .child(one_line(&call.args_json, 120)),
                )
                .child(
                    div()
                        .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                        .text_color(if call.ok == Some(false) {
                            theme.danger
                        } else {
                            theme.muted_foreground
                        })
                        .overflow_hidden()
                        .child(if call.summary.is_empty() {
                            t("Running…").to_owned()
                        } else {
                            one_line(&call.summary, 120)
                        }),
                ),
        );
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .child(head)
        .child(body)
        .into_any_element()
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
