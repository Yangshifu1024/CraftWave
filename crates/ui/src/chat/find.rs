//! 会话内查找（gpui-rewrite 阶段 3.8「Cmd+F + timeline 高亮」）。
//!
//! 组成：⌘F 打开的查找条（composer 上方，形态对齐旧栈查找面）+ 命中
//! 计数（n/m）+ 上/下一个（Enter / Shift+Enter 或按钮）+ Escape 关闭；
//! 命中高亮按 rendering.md「行分发」的搜索跳转条目：淡 primary 底 +
//! **负 margin/等量 padding**——背景外扩不推挤内容（Wake 7266-7273）。
//!
//! ## 高亮形态（TextView 能力核实的取舍，交付报告同步）
//!
//! gpui-base 的 `TextViewState::set_range_highlights` 支持词级命中高亮，
//! 但只对 **owned state**（`TextView::new(&state)`）开放；本仓正文走
//! `TextView::markdown(id, text)`（内部 `use_keyed_state`，状态私有），
//! 外部拿不到状态实体——词级高亮进不去。按任务预案降级：
//! - **TextView 行**（user 气泡 / Assistant 正文 Markdown）：行整体淡底
//!   （`row_is_hit` 驱动，row.rs 的行包装挂）；
//! - **普通文本行**（thinking 展开体、工具 Input/Output 等宽段）：
//!   `StyledText::with_highlights` 词级命中底（[`highlighted_text`]）；
//! - 当前命中行（n/m 落点）在两类行上一视同仁地行级闪示（强一档底 +
//!   淡出动效）；`reduce_motion` 时直接静态高亮（`with_animation` 的
//!   内建降级是 oneshot 终态——全透明，不适用，故该分支不给动画）。
//!
//! ## 查找范围与语义
//!
//! 当前会话活动路径的历史消息（正文 / 工具参数 / 思考文本，一行一命中；
//! 流式末条不参与——内容边写边变，命中位置不稳定）。**过滤行不参与**：
//! 纪律二滤掉的行源数据在但渲染为零高空行，跳过（跳转不会落空行）。
//! 大小写不敏感子串匹配（[`find_ranges`] 逐字符大小写折叠）。
//!
//! 跳转定位：`ListState::scroll_to(Item 级)`（内建 stop_following——跳转
//! 即暂停贴底跟随，滚回底部自动续跟是 list 内建语义）。

use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, App, Context, Entity, HighlightStyle, SharedString, StyledText, Window,
    div, px,
};

use craftwave_engine::storage::{MessageEntry, WirePart};

use crate::i18n::t;
use crate::theme::{self, ActiveCraftwave as _};

use super::state::ConversationState;

/// 跳转闪示的总时长（前段保持、尾段淡出；量级对齐动效 token 的
/// duration-rise 档，曲线在 animator 内用 `theme::ease_cw`）。
const FLASH_TOTAL: Duration = Duration::from_millis(1400);

/// 查找条状态（`ConversationState.find` 槽位；None = 关闭）。
pub(crate) struct FindBar {
    /// 查询输入框（open_find 时建——`InputState::new` 要 `&mut Window`）。
    pub(crate) input: Entity<InputState>,
    /// 当前查询词（run_find 时从输入框抄下的快照；渲染高亮以此为基准，
    /// 与输入框实时值解耦——输入过程中命中面稳定）。
    pub(crate) query: String,
    /// 命中行号（升序；行号 = 消息下标，与 ListState 行号同口径）。
    pub(crate) hits: Vec<usize>,
    /// 当前命中（hits 的下标；hits 空时无意义）。
    pub(crate) current: usize,
}

/// 打开查找条（FindInSession action / 菜单入口）：建输入框（要 window）、
/// 订阅输入事件（Change 重查 / Enter 上下导航）、聚焦。已开时只聚焦
/// （重复 ⌘F 回到查找条）。
pub(crate) fn open_find(
    this: &mut ConversationState,
    window: &mut Window,
    cx: &mut Context<ConversationState>,
) {
    if let Some(find) = &this.find {
        find.input.update(cx, |input, cx| input.focus(window, cx));
        return;
    }
    let input = cx.new(|cx| InputState::new(window, cx));
    // 输入事件：Change → 重查；Enter/Shift+Enter → 下/上一个。
    // 订阅随 owner（本实体）保活存入 _subscriptions（编码指南 §1）
    let subscription = cx.subscribe_in(
        &input,
        window,
        |this: &mut ConversationState,
         _input: &Entity<InputState>,
         event: &InputEvent,
         _window: &mut Window,
         cx: &mut Context<ConversationState>| match event {
            InputEvent::Change => run_find(this, cx),
            InputEvent::PressEnter { shift, .. } => {
                if *shift {
                    find_prev(this, cx);
                } else {
                    find_next(this, cx);
                }
            }
            _ => {}
        },
    );
    this._subscriptions.push(subscription);
    this.find = Some(FindBar {
        input,
        query: String::new(),
        hits: Vec::new(),
        current: 0,
    });
    run_find(this, cx);
    if let Some(find) = &this.find {
        find.input.update(cx, |input, cx| input.focus(window, cx));
    }
    cx.notify();
}

/// 关闭查找条（Escape / 关闭钮）：清命中与闪示，焦点交还 composer。
pub(crate) fn close_find(
    this: &mut ConversationState,
    window: &mut Window,
    cx: &mut Context<ConversationState>,
) {
    if this.find.take().is_none() && this.find_flash.is_none() {
        return;
    }
    this.find_flash = None;
    this.find_flash_task = None;
    if let Some(composer) = &this.composer {
        let input = composer.read(cx).input().clone();
        input.update(cx, |input, cx| input.focus(window, cx));
    }
    cx.notify();
}

/// 查找条开着时的 Escape（沿焦点路径从输入框冒泡上来）与关闭钮共用。
pub(crate) fn on_escape(
    this: &mut ConversationState,
    window: &mut Window,
    cx: &mut Context<ConversationState>,
) {
    if this.find.is_some() {
        close_find(this, window, cx);
    }
}

/// 重查（输入变化 / 打开时）：从输入框抄查询词快照，扫活动路径历史行。
/// 有命中跳第一处（行级闪示 + Item 级定位）；无命中视图不动。
/// pub(crate)：程序化写入查询词（组件约定 set_value 不触发 Change 事件）
/// 后由调用方显式重查——测试注入查询词走同一条路。
pub(crate) fn run_find(this: &mut ConversationState, cx: &mut Context<ConversationState>) {
    let query = this
        .find
        .as_ref()
        .map(|find| find.input.read(cx).value().trim().to_string())
        .unwrap_or_default();
    let hits = if query.is_empty() {
        Vec::new()
    } else {
        scan_hits(this.messages(), &query)
    };
    if let Some(find) = &mut this.find {
        find.query = query;
        find.hits = hits;
        find.current = 0;
    }
    this.find_flash = None;
    this.find_flash_task = None;
    if let Some(row) = this.find.as_ref().and_then(|find| find.hits.first().copied()) {
        jump_to_row(this, row, cx);
    }
    cx.notify();
}

/// 下一个命中（Enter / ▼ 钮）：循环（最后一个再下一个回到第一个）。
pub(crate) fn find_next(this: &mut ConversationState, cx: &mut Context<ConversationState>) {
    let Some(row) = this.find.as_mut().and_then(|find| {
        if find.hits.is_empty() {
            None
        } else {
            find.current = (find.current + 1) % find.hits.len();
            Some(find.hits[find.current])
        }
    }) else {
        return;
    };
    jump_to_row(this, row, cx);
    cx.notify();
}

/// 上一个命中（Shift+Enter / ▲ 钮）：循环。
pub(crate) fn find_prev(this: &mut ConversationState, cx: &mut Context<ConversationState>) {
    let Some(row) = this.find.as_mut().and_then(|find| {
        if find.hits.is_empty() {
            None
        } else {
            find.current = (find.current + find.hits.len() - 1) % find.hits.len();
            Some(find.hits[find.current])
        }
    }) else {
        return;
    };
    jump_to_row(this, row, cx);
    cx.notify();
}

/// 跳到命中行：Item 级滚动（内建 stop_following——上滚暂停语义同源）+
/// 闪示该行。非 reduce_motion 时按总时长派一个清理任务（闪示元素淡出后
/// 离场）；reduce_motion 静态高亮、停留到下一次导航/关闭。
fn jump_to_row(this: &mut ConversationState, row: usize, cx: &mut Context<ConversationState>) {
    this.find_flash = Some(row);
    this.find_flash_gen = this.find_flash_gen.wrapping_add(1);
    // 锚点行级定位（Wake pin_list_row 同款：偏移 0 落行顶，显式停跟）
    this.list.scroll_to(gpui_kit::ListOffset {
        item_ix: row,
        offset_in_item: px(0.),
    });
    if cx.reduce_motion() {
        this.find_flash_task = None;
        return;
    }
    this.find_flash_task = Some(cx.spawn(async move |this, cx| {
        cx.background_executor().timer(FLASH_TOTAL).await;
        let _ = this.update(cx, |this, cx| {
            // 期间又有新跳转（任务被换）就不清——新任务的 timer 负责它
            if this.find_flash_task.is_some() {
                this.find_flash = None;
                this.find_flash_task = None;
                cx.notify();
            }
        });
    }));
}

// ======================================================================
// 纯函数面（scan / 匹配；find_test 直接对账）
// ======================================================================

/// 扫活动路径历史行：过滤行跳过，命中行号升序返回。pub(crate)：
/// 测试直接对账扫描面。
pub(crate) fn scan_hits(messages: &[Rc<MessageEntry>], query: &str) -> Vec<usize> {
    let needle = query.to_lowercase();
    messages
        .iter()
        .enumerate()
        .filter(|(_, entry)| !ConversationState::is_log_entry(entry))
        .filter(|(_, entry)| row_search_text(entry).to_lowercase().contains(&needle))
        .map(|(ix, _)| ix)
        .collect()
}

/// 一行的可查找文本：正文（Text）+ 工具参数（ToolCall args_json）+
/// 工具输出摘要（ToolResult summary）+ 思考文本（Reasoning），换行拼接
/// （跨段不误拼成词）。
pub(crate) fn row_search_text(entry: &MessageEntry) -> String {
    let mut text = String::new();
    for part in &entry.parts {
        match part {
            WirePart::Text { text: body } | WirePart::Reasoning { text: body } => {
                text.push_str(body);
                text.push('\n');
            }
            WirePart::ToolCall { args_json, .. } => {
                text.push_str(args_json);
                text.push('\n');
            }
            WirePart::ToolResult { summary, .. } => {
                text.push_str(summary);
                text.push('\n');
            }
            // 图片与压缩快照/运行标记没有可查词
            WirePart::Image { .. } | WirePart::PlanSnapshot { .. } | WirePart::RunMarker { .. } => {
            }
        }
    }
    text
}

/// 大小写不敏感子串的全部命中区间（字节区间，字符边界对齐；逐字符大小写
/// 折叠比较——不经过 to_lowercase 分配，也不受 İ 类多字符映射的字节错位
/// 影响，代价是极罕见的一对多折叠匹配不到，可接受）。
pub(crate) fn find_ranges(haystack: &str, needle: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    if needle.is_empty() || haystack.is_empty() {
        return ranges;
    }
    let hay: Vec<(usize, char)> = haystack.char_indices().collect();
    let ned: Vec<char> = needle.chars().collect();
    if ned.len() > hay.len() {
        return ranges;
    }
    let fold = |c: char| c.to_lowercase().next().unwrap_or(c);
    let first = fold(ned[0]);
    for start in 0..=hay.len() - ned.len() {
        if fold(hay[start].1) != first {
            continue;
        }
        let matched = ned
            .iter()
            .enumerate()
            .all(|(offset, expected)| fold(hay[start + offset].1) == fold(*expected));
        if matched {
            let from = hay[start].0;
            let last = hay[start + ned.len() - 1];
            ranges.push(from..last.0 + last.1.len_utf8());
        }
    }
    ranges
}

// ======================================================================
// 渲染（row.rs 与 mod.rs 的挂点）
// ======================================================================

/// 当前命中行（None = 无闪示）。row.rs 的行包装据此挂负 margin 条。
pub(crate) fn flash_row(this: &ConversationState) -> Option<usize> {
    this.find_flash
}

/// 某行是否为命中行（非当前的命中行给 TextView 行淡底）。
pub(crate) fn row_is_hit(this: &ConversationState, ix: usize) -> bool {
    this.find
        .as_ref()
        .is_some_and(|find| !find.query.is_empty() && find.hits.contains(&ix))
}

/// 当前查询词（无查找面或空查询时 None）。
pub(crate) fn active_query(this: &ConversationState) -> Option<&str> {
    this.find
        .as_ref()
        .filter(|find| !find.query.is_empty())
        .map(|find| find.query.as_str())
}

/// 普通文本段的词级命中底（thinking 展开体 / 工具 Input/Output 段共用）：
/// 命中区间套 `HighlightStyle` 淡 primary 底（字形内建背景，不推挤文本），
/// 无命中原样返回普通文本元素。
pub(crate) fn highlighted_text(
    text: impl Into<SharedString>,
    query: Option<&str>,
    hit_bg: gpui_kit::Hsla,
) -> gpui_kit::AnyElement {
    let text = text.into();
    let ranges = query.map(|q| find_ranges(&text, q)).unwrap_or_default();
    if ranges.is_empty() {
        return div().child(text).into_any_element();
    }
    let highlights = ranges
        .into_iter()
        .map(|range| {
            (
                range,
                HighlightStyle {
                    background_color: Some(hit_bg),
                    ..Default::default()
                },
            )
        })
        .collect::<Vec<_>>();
    StyledText::new(text)
        .with_highlights(highlights)
        .into_any_element()
}

/// 行级闪示包装（row.rs 的行内容经此过一手）：当前命中行给「淡 primary
/// 底 + 负 margin/等量 padding」（背景外扩不推挤内容，Wake 7266-7273），
/// 非 reduce_motion 套淡出动效（动画 id 带 gen——同一行再次跳转也重放）。
pub(crate) fn wrap_flash(
    this: &ConversationState,
    ix: usize,
    inner: gpui_kit::AnyElement,
    cx: &App,
) -> gpui_kit::AnyElement {
    if flash_row(this) != Some(ix) {
        return inner;
    }
    let zoom = cx.craftwave().zoom;
    let active = cx.craftwave().colors.find_hit_active;
    let flash_gen = this.find_flash_gen;
    let flash = div()
        .w_full()
        .rounded(px(theme::rounded_lg(zoom)))
        .bg(active)
        .mx(px(-theme::SPACE_3))
        .px(px(theme::SPACE_3))
        .my(px(-theme::SPACE_1))
        .py(px(theme::SPACE_1))
        .debug_selector(|| format!("find-flash-{ix}"))
        .child(inner);
    if cx.reduce_motion() {
        // 静态高亮：停留到下一次导航/关闭（任务预案的降级路径）
        return flash.into_any_element();
    }
    flash
        .with_animation(
            SharedString::from(format!("find-flash-{flash_gen}")),
            Animation::new(FLASH_TOTAL),
            move |flash, delta| {
                // 前段保持全亮，尾段 ease 曲线淡出（统一过渡曲线近似）
                let fade = if delta < 0.55 {
                    0.0
                } else {
                    theme::ease_cw((delta - 0.55) / 0.45)
                };
                flash.bg(active.opacity(1.0 - fade))
            },
        )
        .into_any_element()
}

/// 查找条（composer 上方；mod.rs Render 在 composer 之前挂）：输入框 +
/// n/m 计数 + 上/下一个 + 关闭。Escape 沿输入框焦点路径冒泡到容器。
pub(crate) fn render_find_bar(
    this: &mut ConversationState,
    cx: &mut Context<ConversationState>,
) -> Option<gpui_kit::AnyElement> {
    let find = this.find.as_ref()?;
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let input = find.input.clone();
    let count = find.hits.len();
    let current = if count > 0 { find.current + 1 } else { 0 };
    let count_label: SharedString = if count == 0 {
        t("No matches").into()
    } else {
        format!("{current}/{count}").into()
    };
    let nav_btn = |id: &'static str, icon: IconName, tooltip: &'static str| {
        div()
            .id(id)
            .cursor_pointer()
            .flex_shrink_0()
            .flex()
            .items_center()
            .px(px(theme::SPACE_1))
            .py(px(theme::SPACE_1) / 2.0)
            .rounded(px(theme::rounded_sm(zoom)))
            .text_color(theme.muted_foreground)
            .hover(|style| style.bg(theme.list_hover).text_color(theme.foreground))
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(t(tooltip)).build(window, cx)
            })
            .child(Icon::new(icon).with_size(px(theme::ICON_SM)))
    };
    Some(
        h_flex()
            .w_full()
            .items_center()
            .gap(px(theme::SPACE_2))
            .px(px(theme::SPACE_4))
            .py(px(theme::SPACE_2))
            .mb(px(theme::SPACE_2))
            .rounded(px(theme::rounded_lg(zoom)))
            .border_1()
            .border_color(theme.border)
            .bg(theme.muted)
            // Escape 关闭：输入框自身的 Escape 处理不消费时沿焦点路径冒泡
            .on_action(cx.listener(|this: &mut ConversationState, _: &Escape, window, cx| {
                on_escape(this, window, cx);
            }))
            .debug_selector(|| "find-bar".to_owned())
            .child(
                div()
                    .w(px(180.0))
                    .flex_shrink_0()
                    .child(Input::new(&input).appearance(false).bordered(false).small()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(theme::font_size(theme::FONT_UI_CAPTION, zoom)))
                    .text_color(theme.muted_foreground)
                    .child(count_label),
            )
            .child(div().flex_1())
            .child(
                nav_btn("find-prev", IconName::ChevronUp, "Previous match").on_click(
                    cx.listener(|this: &mut ConversationState, _, _, cx| {
                        find_prev(this, cx);
                    }),
                ),
            )
            .child(
                nav_btn("find-next", IconName::ChevronDown, "Next match").on_click(
                    cx.listener(|this: &mut ConversationState, _, _, cx| {
                        find_next(this, cx);
                    }),
                ),
            )
            .child(
                nav_btn("find-close", IconName::X, "Close").on_click(cx.listener(
                    |this: &mut ConversationState, _, window, cx| {
                        close_find(this, window, cx);
                    },
                )),
            )
            .into_any_element(),
    )
}
