//! 子代理视图（gpui-rewrite 3.6「列表 + 详情跳转」）+ B6 run 级错误卡。
//!
//! ## 架构取舍（对照旧栈 `Inspector.tsx` SubagentsPanel + Wake
//! `open_child_button`）
//!
//! - 旧栈的子代理列表住右栏 Inspector 抽屉（3.5 面板族范围），详情 =
//!   列表行内展开的只读抽屉；Wake 蓝本 `open_child_button` 是「派活工具
//!   调用 → 打开那条子会话详情页」。本批中栏优先：详情视图做**中栏内联
//!   覆盖层**（absolute inset_0，带返回入口，不换血 ConversationState
//!   ——对话流状态原样保留，返回即回到原位），列表与详情合在一层：头部
//!   给本会话全部已知子代理的行列表（点击切换详情目标），下方是选中项的
//!   历史只读渲染 + 汇报全文 + 运行中停止钮。工具簇卡的跳转钮（2.8 的
//!   占位）接真直达详情。
//! - 数据双通道（旧栈同构）：**帧面**（`StreamingMessage.subagents`，
//!   当前 run、attach 重放可恢复）∪ **事件面**（sub:spawn/step/report/
//!   done/error，含事件独有 error 文本）。sub_id 去重 upsert，帧面优先。
//! - 历史装载走 engine `load_subagent_history`（JSONL 只读回看），失败
//!   E_NOT_FOUND 视为「尚无历史」（刚 spawn 未收口 turn 的窗口期），其余
//!   失败原位显示。
//! - stop 接 engine `stop_subagent`（级联取消令牌置位；终态以该子代理的
//!   subFrame 序列 runEnded + subDone 为权威，UI 随帧面/事件面自然翻牌）。

use std::rc::Rc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::shimmer::ShimmerText;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, SharedString, div, px, relative};

use craftwave_engine::snapshot::SubRunState;
use craftwave_engine::storage::{MessageEntry, WirePart};

use crate::i18n::t;
use crate::text_util::one_line;
use crate::theme::{self, ActiveCraftwave as _};

use super::state::ConversationState;

/// 子代理 id 在列表行的展示位数（旧栈 `subId.slice(0, 8)` 同口径）。
const SUB_ID_DISPLAY: usize = 8;

/// sub:* 事件的字段束（[`ConversationState::sub_event`] 的 upsert 补丁；
/// None = 本事件不携带该字段——upsert 只覆盖到场字段）。
#[derive(Debug, Default)]
pub(crate) struct SubEventPatch {
    pub call_id: Option<String>,
    pub role: Option<String>,
    pub state: Option<SubRunState>,
    pub turn: Option<u32>,
    pub report: Option<String>,
    pub error: Option<String>,
}

/// 本会话一个子代理的轻量行（帧面与事件面归一后的形态；Inspector 列表、
/// 详情头与工具簇摘要行共用）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SubagentLite {
    pub sub_id: String,
    /// 委派发起的父工具调用 id（工具簇按它关联）。
    pub call_id: String,
    pub role: String,
    pub state: SubRunState,
    /// 已推进的模型轮次。
    pub turn: u32,
    /// `<report>` 结构化汇报全文（subReport 后有值）。
    pub report: String,
    /// 失败说明（sub:error 事件独有）。
    pub error: Option<String>,
}

impl SubagentLite {
    /// 从帧面结构态构造（事件面的 error 落在后续 upsert）。
    pub(crate) fn from_frame(
        sub: &craftwave_engine::snapshot::SubagentState,
    ) -> Self {
        Self {
            sub_id: sub.sub_id.clone(),
            call_id: sub.call_id.clone(),
            role: sub.role.clone(),
            state: sub.state,
            turn: sub.turn,
            report: sub.report.clone(),
            error: None,
        }
    }

    /// 终态徽标文案（四象限 + 运行中；旧栈 outcomeLabel 同表）。
    pub(crate) fn state_label(&self) -> &'static str {
        match self.state {
            SubRunState::Running => t("Running"),
            SubRunState::Done(craftwave_engine::backend::frames::SubOutcome::Report) => {
                t("Completed")
            }
            SubRunState::Done(craftwave_engine::backend::frames::SubOutcome::Partial) => {
                t("Budget exhausted")
            }
            SubRunState::Done(craftwave_engine::backend::frames::SubOutcome::Cancelled) => {
                t("Cancelled")
            }
            SubRunState::Done(craftwave_engine::backend::frames::SubOutcome::NoReport) => {
                t("No report")
            }
        }
    }
}

/// 子代理详情视图的装载状态（idle/在途/已装载/失败 四态建模，编码指南 §5）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SubHistoryState {
    /// 装载中。
    Loading,
    /// 装载成功（turn 序只读条目；可能是空——刚 spawn 尚无收口轮次）。
    Loaded(Rc<Vec<Rc<MessageEntry>>>),
    /// 装载失败（原位显示原因）。
    Failed(String),
}

/// 子代理详情视图（中栏覆盖层的状态面）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SubagentView {
    /// 当前详情目标的子代理 id。
    pub sub_id: String,
    /// 历史装载状态。
    pub history: SubHistoryState,
}

// ======================================================================
// ConversationState 的子代理分片（状态读写）
// ======================================================================

impl ConversationState {
    /// 已知子代理集合：帧面（当前 run）∪ 事件表，sub_id 去重、帧面优先
    /// （帧面随 attach 重放恢复，是当前 run 的权威；事件表补充历史项与
    /// error 文本）。
    pub(crate) fn known_subagents(&self) -> Vec<SubagentLite> {
        let mut out: Vec<SubagentLite> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        if let Some(streaming) = self.streaming().as_ref() {
            for sub in &streaming.subagents {
                seen.insert(sub.sub_id.clone());
                out.push(SubagentLite::from_frame(sub));
            }
        }
        for row in &self.sub_events {
            if seen.insert(row.sub_id.clone()) {
                out.push(row.clone());
            }
        }
        out
    }

    /// 按 id 查已知子代理（工具簇摘要行的状态源）。
    pub(crate) fn sub_row_of(&self, sub_id: &str) -> Option<SubagentLite> {
        self.known_subagents()
            .into_iter()
            .find(|row| row.sub_id == sub_id)
    }

    /// sub:* 事件 upsert（服务的事件接线调用；payload 带 sessionId，
    /// 匹配过滤在服务侧完成）。
    ///
    /// 事件字段束收进参数结构（8 参收 3，clippy too_many_arguments）。
    pub(crate) fn sub_event(
        &mut self,
        sub_id: String,
        patch: SubEventPatch,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self.sub_events.iter_mut().find(|row| row.sub_id == sub_id) {
            if row.call_id.is_empty()
                && let Some(call_id) = patch.call_id
            {
                row.call_id = call_id;
            }
            if row.role.is_empty()
                && let Some(role) = patch.role
            {
                row.role = role;
            }
            if let Some(state) = patch.state {
                row.state = state;
            }
            if let Some(turn) = patch.turn {
                row.turn = row.turn.max(turn);
            }
            if let Some(report) = patch.report {
                row.report = report;
            }
            if patch.error.is_some() {
                row.error = patch.error;
            }
        } else {
            self.sub_events.push(SubagentLite {
                sub_id,
                call_id: patch.call_id.unwrap_or_default(),
                role: patch.role.unwrap_or_default(),
                state: patch.state.unwrap_or(SubRunState::Running),
                turn: patch.turn.unwrap_or(0),
                report: patch.report.unwrap_or_default(),
                error: patch.error,
            });
        }
        cx.notify();
    }

    /// 打开子代理详情视图（跳转钮/列表行共用）：置目标 + 发起历史装载
    /// （装载走服务——engine 命令的面）。同目标重复打开 = 幂等。
    pub(crate) fn open_sub_view(&mut self, sub_id: String, cx: &mut Context<Self>) {
        if self
            .sub_view
            .as_ref()
            .is_some_and(|view| view.sub_id == sub_id)
        {
            return;
        }
        self.sub_view = Some(SubagentView {
            sub_id: sub_id.clone(),
            history: SubHistoryState::Loading,
        });
        cx.notify();
        let Some(service) = self.service.clone() else {
            // 回声装置：无装载面，给空态（不假装有历史）
            if let Some(view) = self.sub_view.as_mut() {
                view.history = SubHistoryState::Failed(t("Subagent history unavailable").to_owned());
            }
            cx.notify();
            return;
        };
        let sid = sub_id.clone();
        cx.defer(move |cx| {
            service.update(cx, |service, cx| service.load_sub_history(sid, cx));
        });
    }

    /// 历史装载回写（服务侧后台读盘完成后调用；E_NOT_FOUND 视为「尚无
    /// 历史」空态——刚 spawn 未收口轮次的窗口期）。
    pub(crate) fn sub_history_loaded(
        &mut self,
        sub_id: &str,
        result: Result<Vec<MessageEntry>, craftwave_engine::util::errors::IpcError>,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self
            .sub_view
            .as_mut()
            .filter(|view| view.sub_id == sub_id)
        else {
            return; // 用户已切走：过期回写丢弃
        };
        view.history = match result {
            Ok(entries) => SubHistoryState::Loaded(Rc::new(
                entries.into_iter().map(Rc::new).collect(),
            )),
            Err(err) if err.code == craftwave_engine::util::errors::E_NOT_FOUND => {
                SubHistoryState::Failed(
                    t("No history yet — the subagent has not finished a turn").to_owned(),
                )
            }
            Err(err) => SubHistoryState::Failed(err.message),
        };
        cx.notify();
    }

    /// 关闭详情视图（返回对话流）。
    pub(crate) fn close_sub_view(&mut self, cx: &mut Context<Self>) {
        if self.sub_view.take().is_some() {
            cx.notify();
        }
    }

    /// 会话切换 / 新发送：清空事件表并关闭详情视图。
    pub(crate) fn clear_subagents(&mut self, cx: &mut Context<Self>) {
        let had = !self.sub_events.is_empty() || self.sub_view.is_some();
        self.sub_events.clear();
        self.sub_view = None;
        if had {
            cx.notify();
        }
    }
}

// ======================================================================
// 渲染一：详情视图覆盖层（列表 + 详情一体；mod.rs 在中栏最外层挂）
// ======================================================================

/// 详情视图覆盖层（None = 关闭）。头部 = 返回钮 + 角色徽标 + 列表行 +
/// （运行中）停止钮；正文 = 历史只读渲染 + 汇报全文。
pub(crate) fn render_sub_view(
    state: &ConversationState,
    cx: &mut Context<ConversationState>,
) -> Option<AnyElement> {
    let view = state.sub_view.as_ref()?;
    let theme = cx.theme().colors;
    let scrim = cx.craftwave().colors.scrim;
    let zoom = cx.craftwave().zoom;
    let subs = state.known_subagents();
    let current = subs.iter().find(|row| row.sub_id == view.sub_id);

    let mut panel = div()
        .w_full()
        .max_h_full()
        .flex()
        .flex_col()
        .rounded(px(theme::rounded_xl(zoom)))
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .overflow_hidden();
    // 头部：返回 + 标题 + 停止（运行中）
    let mut head = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_3))
        .px(px(theme::SPACE_5))
        .py(px(theme::SPACE_3))
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .id("sub-view-back")
                .debug_selector(|| "sub-view-back".to_owned())
                .cursor_pointer()
                .flex()
                .items_center()
                .gap(px(theme::SPACE_1))
                .px(px(theme::SPACE_2))
                .py(px(theme::SPACE_1))
                .rounded(px(theme::rounded_md(zoom)))
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .text_color(theme.muted_foreground)
                .hover(|style| style.bg(theme.list_hover).text_color(theme.foreground))
                .on_click(cx.listener(|this: &mut ConversationState, _, _, cx| {
                    this.close_sub_view(cx);
                }))
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::ChevronLeft)
                        .with_size(px(theme::ICON_SM)),
                )
                .child(t("Back to conversation")),
        )
        .child(
            div()
                .text_size(px(theme::font_size(theme::FONT_UI_BASE, zoom)))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.foreground)
                .child(t("Subagents")),
        )
        .child(div().flex_1());
    if let Some(row) = current
        && row.state == SubRunState::Running
    {
        let sub_id = row.sub_id.clone();
        let click_sub_id = sub_id.clone();
        head = head.child(
            div()
                .id(SharedString::from(format!("sub-stop-{}", row.sub_id)))
                .debug_selector(move || format!("sub-stop-{sub_id}"))
                .cursor_pointer()
                .px(px(theme::SPACE_3))
                .py(px(theme::SPACE_1))
                .rounded(px(theme::rounded_md(zoom)))
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .text_color(theme.danger)
                .hover(|style| style.bg(theme.danger.opacity(0.08)))
                .on_click(cx.listener(move |this: &mut ConversationState, _, _, cx| {
                    stop_subagent(this, &click_sub_id, cx);
                }))
                .child(t("Stop")),
        );
    }
    panel = panel.child(head);

    // 列表行（本会话全部已知子代理；点击切换详情目标）
    if !subs.is_empty() {
        let mut list = div()
            .w_full()
            .flex()
            .flex_col()
            .px(px(theme::SPACE_5))
            .py(px(theme::SPACE_2))
            .gap(px(theme::SPACE_1))
            .debug_selector(|| "subagent-list".to_owned());
        for row in &subs {
            let selected = row.sub_id == view.sub_id;
            list = list.child(render_sub_list_row(row, selected, cx));
        }
        panel = panel.child(list);
    }

    // 详情体：目标 meta（角色/turn/错误）+ 历史 + 汇报
    panel = panel.child(render_sub_detail(state, view, current, cx));

    Some(
        div()
            .id("subagent-view")
            .debug_selector(|| "subagent-view".to_owned())
            .absolute()
            .inset_0()
            .occlude()
            .bg(scrim)
            .flex()
            .justify_center()
            .items_start()
            .px(px(theme::SPACE_8))
            .py(px(theme::SPACE_8))
            .on_click(cx.listener(|this: &mut ConversationState, _, _, cx| {
                this.close_sub_view(cx);
            }))
            .child(
                div()
                    .w_full()
                    .max_w(px(super::state::READER_MAX_WIDTH))
                    .max_h_full()
                    .id("sub-view-panel")
                    .flex()
                    .flex_col()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .child(panel),
            )
            .into_any_element(),
    )
}

/// 列表行：角色徽标 + subId 片段 + 状态 + turn 计数；选中高亮。
fn render_sub_list_row(
    row: &SubagentLite,
    selected: bool,
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let target = row.sub_id.clone();
    div()
        .id(SharedString::from(format!("subagent-row-{}", row.sub_id)))
        .debug_selector(move || format!("subagent-row-{}", row.sub_id))
        .cursor_pointer()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_2))
        .px(px(theme::SPACE_3))
        .py(px(theme::SPACE_2))
        .rounded(px(theme::rounded_md(zoom)))
        .border_1()
        .border_color(if selected { theme.ring } else { theme.border })
        .when(row.state == SubRunState::Running && !selected, |line| {
            line.bg(theme.muted)
        })
        .hover(|style| style.bg(theme.list_hover))
        .on_click(cx.listener(move |this: &mut ConversationState, _, _, cx| {
            this.open_sub_view(target.clone(), cx);
        }))
        .child(
            div()
                .flex_shrink_0()
                .px(px(theme::SPACE_2))
                .py(px(theme::SPACE_1) / 2.0)
                .rounded(px(theme::rounded_sm(zoom)))
                .bg(theme.accent)
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.foreground)
                .child(row.role.clone()),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .whitespace_nowrap()
                .truncate()
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(theme.muted_foreground)
                .child(short_sub_id(&row.sub_id)),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(outcome_color(row.state, cx))
                .child(row.state_label()),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(theme.muted_foreground)
                .child(t("turn N").replace("{n}", &row.turn.to_string())),
        )
        .into_any_element()
}

/// 详情体：meta 行（错误说明）+ 历史（装载态/逐 turn 只读渲染）+ 汇报全文。
fn render_sub_detail(
    state: &ConversationState,
    view: &SubagentView,
    current: Option<&SubagentLite>,
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let mut body = div()
        .w_full()
        .min_h_0()
        .flex()
        .flex_col()
        .id(SharedString::from(format!("sub-detail-scroll-{}", view.sub_id)))
        .gap(px(theme::SPACE_3))
        .px(px(theme::SPACE_5))
        .py(px(theme::SPACE_3))
        .overflow_y_scroll();
    if let Some(error) = current.and_then(|row| row.error.as_ref()) {
        body = body.child(
            div()
                .w_full()
                .px(px(theme::SPACE_2))
                .py(px(theme::SPACE_1))
                .rounded(px(theme::rounded_sm(zoom)))
                .bg(theme.danger.opacity(0.08))
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .text_color(theme.danger)
                .child(error.clone()),
        );
    }
    match &view.history {
        SubHistoryState::Loading => body = body.child(loading_line(cx)),
        SubHistoryState::Failed(message) => {
            body = body.child(
                div()
                    .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                    .text_color(theme.muted_foreground)
                    .child(message.clone()),
            );
        }
        SubHistoryState::Loaded(entries) => {
            if entries.is_empty() {
                body = body.child(
                    div()
                        .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                        .text_color(theme.muted_foreground)
                        .child(t("No history yet — the subagent has not finished a turn")),
                );
            }
            for entry in entries.iter() {
                body = body.child(render_sub_history_entry(entry, cx));
            }
        }
    }
    // 汇报全文（subReport 后；永不截断——纪律三）
    if let Some(report) = current.map(|row| row.report.as_str()).filter(|r| !r.trim().is_empty()) {
        body = body.child(
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(theme::SPACE_1))
                .border_t_1()
                .border_color(theme.border)
                .pt(px(theme::SPACE_3))
                .child(
                    div()
                        .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                        .text_color(cx.craftwave().colors.foreground_subtlest)
                        .child(t("Report")),
                )
                .child(
                    gpui_kit::component::text::TextView::markdown("sub-report", report.to_owned())
                        .style(gpui_kit::component::text::TextViewStyle {
                            is_dark: cx.theme().is_dark(),
                            ..Default::default()
                        })
                        .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                        .line_height(relative(theme::LEADING_ASSISTANT))
                        .text_color(theme.foreground),
                ),
        );
    }
    let _ = state;
    body.into_any_element()
}

/// 单个 turn 条目的只读渲染：turn 标签 + 工具行（调用/结果配对展示）+
/// 推理淡行 + 正文。
fn render_sub_history_entry(
    entry: &MessageEntry,
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let mono = cx.theme().mono_font_family.clone();
    let is_report = entry.id.ends_with(":report");
    let turn_label = if is_report {
        t("Final report").to_owned()
    } else {
        let n = entry.id.split(":a").nth(1).unwrap_or("").to_owned();
        t("turn N").replace("{n}", &n)
    };
    let mut item = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_1))
        .rounded(px(theme::rounded_md(zoom)))
        .border_1()
        .border_color(theme.border)
        .p(px(theme::SPACE_3))
        .debug_selector(move || format!("sub-entry-{}", entry.id))
        .child(
            div()
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(cx.craftwave().colors.foreground_subtlest)
                .child(turn_label),
        );
    for part in &entry.parts {
        match part {
            WirePart::ToolCall {
                name, args_json, ..
            } => {
                item = item.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .flex_row()
                        .gap(px(theme::SPACE_2))
                        .child(
                            div()
                                .flex_shrink_0()
                                .px(px(theme::SPACE_1))
                                .rounded(px(theme::rounded_sm(zoom)))
                                .bg(theme.muted)
                                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                                .font_family(mono.clone())
                                .text_color(theme.foreground)
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .truncate()
                                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                                .font_family(mono.clone())
                                .text_color(theme.muted_foreground)
                                .child(one_line(args_json, 160)),
                        ),
                );
            }
            WirePart::ToolResult { ok, summary, .. } => {
                item = item.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                        .text_color(if *ok { theme.success } else { theme.danger })
                        .child(format!("↳ {}", summary)),
                );
            }
            WirePart::Reasoning { text } => {
                item = item.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .whitespace_normal()
                        .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                        .text_color(theme.muted_foreground)
                        .child(text.clone()),
                );
            }
            WirePart::Text { text } if !text.trim().is_empty() => {
                item = item.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .whitespace_normal()
                        .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                        .line_height(relative(theme::LEADING_ASSISTANT))
                        .text_color(theme.foreground)
                        .child(text.clone()),
                );
            }
            _ => {}
        }
    }
    item.into_any_element()
}

/// 装载中行（B3：扫光不用 spinner）。
fn loading_line(cx: &mut Context<ConversationState>) -> AnyElement {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    ShimmerText::new(t("Loading history…"))
        .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
        .text_color(theme.muted_foreground)
        .into_any_element()
}

/// 子代理 id 的展示片段（前 8 位）。
fn short_sub_id(sub_id: &str) -> String {
    sub_id.chars().take(SUB_ID_DISPLAY).collect()
}

/// 终态徽标配色（四象限：report=success / partial=warning / cancelled=
/// muted / no_report=danger / 运行中=ring 蓝）。
fn outcome_color(
    state: SubRunState,
    cx: &Context<ConversationState>,
) -> gpui_kit::Hsla {
    let theme = cx.theme().colors;
    match state {
        SubRunState::Running => theme.ring,
        SubRunState::Done(craftwave_engine::backend::frames::SubOutcome::Report) => theme.success,
        SubRunState::Done(craftwave_engine::backend::frames::SubOutcome::Partial) => theme.warning,
        SubRunState::Done(craftwave_engine::backend::frames::SubOutcome::Cancelled) => {
            theme.muted_foreground
        }
        SubRunState::Done(craftwave_engine::backend::frames::SubOutcome::NoReport) => theme.danger,
    }
}

/// 停止子代理（详情头/工具簇摘要行共用）：defer 到服务走 engine 命令。
pub(crate) fn stop_subagent(
    state: &mut ConversationState,
    sub_id: &str,
    cx: &mut Context<ConversationState>,
) {
    let Some(service) = state.service.clone() else {
        return;
    };
    let sub_id = sub_id.to_owned();
    cx.defer(move |cx| {
        service.update(cx, |service, cx| service.stop_subagent(&sub_id, cx));
    });
}

// ======================================================================
// B6 run 级错误卡（消息流尾部条带；ask 卡之上）
// ======================================================================

/// run 级错误卡：错误摘要行 + 默认展开的完整错误详情（纪律三：错误永不
/// 截断，全文可选可复制）+ 分流按钮（模型/供应商类 → 打开模型设置占位；
/// 其他 → 重试（重发最后一条 user 消息））+ 复制完整错误 + 关闭。
pub(crate) fn render_run_error(
    state: &ConversationState,
    cx: &mut Context<ConversationState>,
) -> Option<AnyElement> {
    let error = state.run_error.as_ref()?;
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let full = error.full_text();
    let is_model = error.is_model_error();
    // 摘要行 = 错误消息首行（完整文本在详情区，永不丢失）
    let summary = error
        .message
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(&error.message)
        .to_owned();
    let mut actions = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(theme::SPACE_2));
    // 分流按钮（B6：模型不可用 → 设置入口占位；其他 → 重试）
    if is_model {
        actions = actions.child(
            div()
                .id("error-open-settings")
                .debug_selector(|| "error-open-settings".to_owned())
                .cursor_pointer()
                .px(px(theme::SPACE_3))
                .py(px(theme::SPACE_1))
                .rounded(px(theme::rounded_md(zoom)))
                .border_1()
                .border_color(theme.border)
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .text_color(theme.foreground)
                .hover(|style| style.bg(theme.list_hover))
                // 设置页归 3.7——本批占位：派发壳的 OpenSettings action
                // （main.rs 现为日志占位处理器，设置入口接通后自动生效）
                .on_click(move |_, _, cx| {
                    cx.dispatch_action(&crate::OpenSettings);
                })
                .child(t("Open model settings")),
        );
    } else {
        actions = actions.child(
            div()
                .id("error-retry")
                .debug_selector(|| "error-retry".to_owned())
                .cursor_pointer()
                .px(px(theme::SPACE_3))
                .py(px(theme::SPACE_1))
                .rounded(px(theme::rounded_md(zoom)))
                .bg(theme.primary)
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .text_color(theme.primary_foreground)
                .hover(|style| style.opacity(0.9))
                .on_click(cx.listener(|this: &mut ConversationState, _, _, cx| {
                    retry_last_run(this, cx);
                }))
                .child(t("Retry")),
        );
    }
    // 一键复制完整错误（错误码 + 消息全文）
    actions = actions.child(
        div()
            .id("error-copy")
            .debug_selector(|| "error-copy".to_owned())
            .cursor_pointer()
            .px(px(theme::SPACE_3))
            .py(px(theme::SPACE_1))
            .rounded(px(theme::rounded_md(zoom)))
            .border_1()
            .border_color(theme.border)
            .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
            .text_color(theme.muted_foreground)
            .hover(|style| style.bg(theme.list_hover).text_color(theme.foreground))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(t("Copy full error")).build(window, cx)
            })
            .on_click(move |_, _, cx| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(full.clone()));
            })
            .child(t("Copy error")),
    );
    // 关闭
    actions = actions.child(
        div()
            .id("error-close")
            .cursor_pointer()
            .px(px(theme::SPACE_2))
            .py(px(theme::SPACE_1))
            .rounded(px(theme::rounded_md(zoom)))
            .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
            .text_color(theme.muted_foreground)
            .hover(|style| style.bg(theme.list_hover).text_color(theme.foreground))
            .on_click(cx.listener(|this: &mut ConversationState, _, _, cx| {
                this.dismiss_run_error(cx);
            }))
            .child("×"),
    );
    Some(
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_2))
            .px(px(theme::SPACE_4))
            .py(px(theme::SPACE_3))
            .mx(px(theme::SPACE_5))
            .mb(px(theme::SPACE_2))
            .rounded(px(theme::rounded_lg(zoom)))
            .border_1()
            .border_color(theme.danger.opacity(0.35))
            .bg(theme.danger.opacity(0.05))
            .debug_selector(|| "run-error-card".to_owned())
            // 摘要行
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(theme::SPACE_2))
                    .child(
                        gpui_kit::component::Icon::new(gpui_kit::assets::IconName::CircleAlert)
                            .with_size(px(theme::ICON_MD))
                            .text_color(theme.danger)
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(summary),
                    ),
            )
            // 详情默认展开：错误码 + 完整消息（纪律三：全文可选可复制，
            // 永不截断；走 TextView 保证文本可选择——错误文本的 markdown
            // 元字符只影响样式不影响内容完整性）
            .child(
                gpui_kit::component::text::TextView::markdown(
                    "run-error-detail",
                    error.full_text(),
                )
                .style(gpui_kit::component::text::TextViewStyle {
                    is_dark: cx.theme().is_dark(),
                    ..Default::default()
                })
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .line_height(relative(theme::LEADING_CODE))
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(theme.muted_foreground),
            )
            .child(actions)
            .into_any_element(),
    )
}

/// 重试：重发最后一条 user 消息（defer 到服务走 branch_resend——同父
/// 挂新分支，不篡改历史）。无 user 消息时无动作。
fn retry_last_run(
    state: &mut ConversationState,
    cx: &mut Context<ConversationState>,
) {
    let Some(last_user) = state
        .messages()
        .iter()
        .rev()
        .find(|entry| entry.role == craftwave_engine::storage::MessageRole::User)
        .map(|entry| (**entry).clone())
    else {
        return;
    };
    let text = last_user
        .parts
        .iter()
        .find_map(|part| match part {
            craftwave_engine::storage::WirePart::Text { text } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    if text.trim().is_empty() {
        return;
    }
    this_dismiss_error(state, cx);
    let Some(service) = state.service.clone() else {
        return;
    };
    cx.defer(move |cx| {
        service.update(cx, |service, cx| {
            service.branch_resend(last_user.parent_id.clone(), text, Vec::new(), cx)
        });
    });
}

/// 关闭错误卡（settle/清理共用的小助手）。
fn this_dismiss_error(
    state: &mut ConversationState,
    cx: &mut Context<ConversationState>,
) {
    state.dismiss_run_error(cx);
}
