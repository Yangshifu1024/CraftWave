//! Ask 权限面板（gpui-rewrite 3.4「权限与确认」的确认部分）。
//!
//! ## 形态取舍（对照旧栈 `AskPanel.tsx`）
//!
//! 旧栈是「fixed 底部浮层」——权限请求出现在对话流区域底部、不弹系统框。
//! 本批平移为**中栏消息流尾部条带**（消息列表与 Composer 之间的独立行）：
//! 与旧栈同属「内联在对话流里」，不用 gpui-kit dialog（确需模态的场景
//! 目前没有——审批必须可边看消息边决定，模态反而挡上下文）。多 ask 依序
//! 纵排（engine 同一会话同一时刻至多一个未决询问——槽位串行保证，队列
//! 形态是防御面：后开先答，互不遮盖）。
//!
//! ## 载荷五形态与控件（`AskPayload` 以 engine 源码为准）
//!
//! - `Approval`：命令行完整展示（等宽 + 复制完整）、围栏原因警示、灰区
//!   审计注记、计划范围对照、diff 预览（逐行增删着色 + 复制全文）→
//!   `resolve_ask(kind=approval, optionId)`；
//! - `Question`：逐题选项 chips 单选 + 自由文本输入（render 惰性建
//!   InputState——`InputState::new` 要 window，事件路径没有）→
//!   `resolve_ask(kind=question, answers)`；
//! - `McpTrust` / `ProjectTrust`：摘要块（服务器/指纹等）+ 选项按钮 →
//!   应答同 Approval 协议；
//! - `ExternalDir`：授权路径 + 粒度说明 + 允许/拒绝（外部授权形态——
//!   engine 有此 ask 类型，展示授权目标与等待态说明）。
//!
//! ## 生命周期
//!
//! ask:opened 入队 → 用户点选项按钮（question 为提交钮）→
//! `resolve_ask`（同步语义：oneshot 投递无 await 点，直接跑）→ 成功即
//! 收束为终态行（已允许/已拒绝/已回答）；ask:closed 只做兜底清理
//! （autoConfirm 自动放行/取消等非本地路径，载荷无选项信息，不留终态行）；
//! E_NOT_FOUND（询问已失效）→「已失效」终态行，不残留可操作面板。
//! 会话切换时清空本地面板，挂起 ask 以 engine 槽位为权威恢复
//! （`session_pending_ask`）。

use std::collections::BTreeMap;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Context, Entity, FontWeight, SharedString, div, px,
};

use craftwave_engine::backend::ask::{AskAnswer, AskPayload};

use crate::i18n::t;
use crate::theme::{self, ActiveCraftwave as _};

use super::state::ConversationState;

/// 审批 diff 预览区的最大高度（基准 px，随缩放）：预览是审批主面但不可
/// 无限撑高消息流——区内滚动（嵌套滚动的取舍：审批卡是模态性交互件，
/// 滚轮让给预览区，与旧栈 DiffView max-h 同语义）。
const DIFF_MAX_HEIGHT: f32 = 360.0;
/// 摘要块（信任/授权卡）的最大高度（同上，长命令/URL 收进滚动）。
const SUMMARY_MAX_HEIGHT: f32 = 180.0;
/// 处置终态（终态行的分流文案与着色）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AskOutcome {
    /// 已允许（allow_once / always_session / always_allow / allow）。
    Allowed,
    /// 已拒绝（deny 及其他非放行选项）。
    Denied,
    /// 已回答（question 形态）。
    Answered,
    /// 已失效（E_NOT_FOUND——重连/收口竞态期间已被解决或取消）。
    Expired,
}

/// 挂起的询问/审批卡（ask:opened 的渲染面）。
#[derive(Debug, Clone)]
pub(crate) struct PendingAsk {
    pub ask_id: String,
    pub payload: AskPayload,
    /// 应答已提交等待收口（防重复提交；收口前按钮禁用）。
    pub submitted: bool,
    /// 应答提交失败的原位错误（E_NOT_FOUND 不走本字段——直接失效移除）。
    pub error: Option<String>,
}

/// 卡片标题（载荷按形态给出的人话标题；终态行复用）。
impl PendingAsk {
    pub(crate) fn title(&self) -> String {
        match &self.payload {
            AskPayload::Question { title, .. } => title.clone(),
            AskPayload::Approval { title, .. } => title.clone(),
            AskPayload::McpTrust { server, .. } => {
                t("MCP server trust").replace("{server}", server)
            }
            AskPayload::ProjectTrust {
                resource_name, ..
            } => t("Project resource trust").replace("{resource}", resource_name),
            AskPayload::ExternalDir { .. } => t("External directory access").to_owned(),
        }
    }
}

/// 处置终态行（卡片收束后的流内痕迹）。
#[derive(Debug, Clone)]
pub(crate) struct ResolvedAsk {
    pub ask_id: String,
    pub title: String,
    pub outcome: AskOutcome,
}

// ======================================================================
// ConversationState 的 ask 分片（状态读写；渲染在下方自由函数）
// ======================================================================

impl ConversationState {
    /// 入队一张挂起卡（ask:opened 事件 / 会话切换恢复共用）。同 ask_id
    /// 幂等（重放与事件双通道可能重复到达）。
    pub(crate) fn open_ask(
        &mut self,
        ask_id: String,
        payload: AskPayload,
        cx: &mut Context<Self>,
    ) {
        if self.pending_asks.iter().any(|ask| ask.ask_id == ask_id) {
            return;
        }
        self.pending_asks.push(PendingAsk {
            ask_id,
            payload,
            submitted: false,
            error: None,
        });
        cx.notify();
    }

    /// 兜底清理（ask:closed）：卡还在队里（autoConfirm 自动放行/取消等
    /// 非本地路径——本地处置路径在 [`Self::settle_ask`] 已先移除）就静默
    /// 移除，不留终态行（closed 载荷无选项信息，编不了「已允许/已拒绝」）。
    pub(crate) fn close_ask(&mut self, ask_id: &str, cx: &mut Context<Self>) {
        if let Some(pos) = self.pending_asks.iter().position(|a| a.ask_id == ask_id) {
            self.pending_asks.remove(pos);
            cx.notify();
        }
    }

    /// 本地处置收束：卡替换为终态行（已允许/已拒绝/已回答）。
    pub(crate) fn settle_ask(
        &mut self,
        ask_id: &str,
        outcome: AskOutcome,
        cx: &mut Context<Self>,
    ) {
        let Some(pos) = self.pending_asks.iter().position(|a| a.ask_id == ask_id) else {
            return;
        };
        let card = self.pending_asks.remove(pos);
        self.resolved_asks.push(ResolvedAsk {
            ask_id: ask_id.to_owned(),
            title: card.title(),
            outcome,
        });
        // 该卡的输入框与选择表随卡收束释放
        self.ask_inputs.retain(|(ask, _), _| ask != ask_id);
        self.question_selections
            .retain(|(ask, _), _| ask != ask_id);
        cx.notify();
    }

    /// 应答提交失败（非失效类）原位显示，可重试：复位提交态 + 记错误。
    pub(crate) fn ask_failed(&mut self, ask_id: &str, message: String, cx: &mut Context<Self>) {
        if let Some(card) = self
            .pending_asks
            .iter_mut()
            .find(|a| a.ask_id == ask_id)
        {
            card.submitted = false;
            card.error = Some(message);
            cx.notify();
        }
    }

    /// 询问已失效（E_NOT_FOUND——重连/收口竞态期间已被解决或取消）：
    /// 卡替换为「已失效」终态行，不残留可操作面板（旧栈 useSubmitError
    /// 的同款语义）。
    pub(crate) fn expire_ask(&mut self, ask_id: &str, cx: &mut Context<Self>) {
        let Some(pos) = self.pending_asks.iter().position(|a| a.ask_id == ask_id) else {
            return;
        };
        let card = self.pending_asks.remove(pos);
        self.resolved_asks.push(ResolvedAsk {
            ask_id: ask_id.to_owned(),
            title: card.title(),
            outcome: AskOutcome::Expired,
        });
        self.ask_inputs.retain(|(ask, _), _| ask != ask_id);
        self.question_selections
            .retain(|(ask, _), _| ask != ask_id);
        cx.notify();
    }

    /// 会话切换 / 新发送：清空本地面板（挂起 ask 以 engine 槽为权威，
    /// 由服务在 open_session 时经 `session_pending_ask` 恢复）。
    pub(crate) fn clear_asks(&mut self, cx: &mut Context<Self>) {
        let changed = !self.pending_asks.is_empty()
            || !self.resolved_asks.is_empty()
            || !self.ask_inputs.is_empty()
            || !self.question_selections.is_empty();
        self.pending_asks.clear();
        self.resolved_asks.clear();
        self.ask_inputs.clear();
        self.question_selections.clear();
        if changed {
            cx.notify();
        }
    }

    /// 挂起卡队列（渲染读面）。
    pub(crate) fn pending_asks(&self) -> &[PendingAsk] {
        &self.pending_asks
    }

    /// 终态行列表（渲染读面）。
    pub(crate) fn resolved_asks(&self) -> &[ResolvedAsk] {
        &self.resolved_asks
    }

    /// 问卷自由文本题的输入框（None = 尚未建——render 惰性补）。
    pub(crate) fn ask_input(
        &self,
        ask_id: &str,
        question_id: &str,
    ) -> Option<&Entity<InputState>> {
        self.ask_inputs
            .get(&(ask_id.to_owned(), question_id.to_owned()))
    }

    /// 惰性补建缺失的问卷输入框（Render 入口调用；`InputState::new` 要
    /// window——事件路径建不了，渲染帧补齐后同帧直接可用。建满即收敛，
    /// 不 notify 不构成反馈环）。
    pub(crate) fn ensure_ask_inputs(
        &mut self,
        window: &mut gpui_kit::Window,
        cx: &mut Context<Self>,
    ) {
        let mut missing: Vec<(String, String)> = Vec::new();
        for card in &self.pending_asks {
            if let AskPayload::Question { questions, .. } = &card.payload {
                for question in questions {
                    if question.allow_free_text
                        && !self
                            .ask_inputs
                            .contains_key(&(card.ask_id.clone(), question.id.clone()))
                    {
                        missing.push((card.ask_id.clone(), question.id.clone()));
                    }
                }
            }
        }
        for (ask_id, question_id) in missing {
            let input = cx.new(|cx| InputState::new(window, cx));
            self.ask_inputs.insert((ask_id, question_id), input);
        }
    }
}

// ======================================================================
// 渲染：条带 = 终态行（最近在前）+ 挂起卡（依序纵排）
// ======================================================================

/// Ask 条带（None = 无终态行也无挂起卡——不占布局）。
pub(crate) fn render_ask_bar(
    state: &ConversationState,
    cx: &mut Context<ConversationState>,
) -> Option<AnyElement> {
    if state.pending_asks().is_empty() && state.resolved_asks().is_empty() {
        return None;
    }
    let mut stack = div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_2))
        .px(px(theme::SPACE_5))
        .pb(px(theme::SPACE_2));
    for resolved in state.resolved_asks() {
        stack = stack.child(render_resolved_row(resolved, cx));
    }
    for card in state.pending_asks() {
        stack = stack.child(render_pending_card(state, card, cx));
    }
    Some(stack.into_any_element())
}

/// 终态行：✓/✗ 图标 + 结果词 + 卡片标题（紧凑一行，无操作面）。
fn render_resolved_row(
    resolved: &ResolvedAsk,
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let (color, icon, label) = match resolved.outcome {
        AskOutcome::Allowed => (theme.success, gpui_kit::assets::IconName::Check, t("Allowed")),
        AskOutcome::Denied => (
            theme.danger,
            gpui_kit::assets::IconName::Close,
            t("Denied"),
        ),
        AskOutcome::Answered => (
            theme.success,
            gpui_kit::assets::IconName::Check,
            t("Answered"),
        ),
        AskOutcome::Expired => (
            theme.muted_foreground,
            gpui_kit::assets::IconName::CircleAlert,
            t("Expired"),
        ),
    };
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(theme::SPACE_2))
        .px(px(theme::SPACE_3))
        .py(px(theme::SPACE_1))
        .rounded(px(theme::rounded_md(zoom)))
        .debug_selector(move || format!("ask-resolved-{}", resolved.ask_id))
        .child(
            gpui_kit::component::Icon::new(icon)
                .with_size(px(theme::ICON_SM))
                .text_color(color),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .text_color(color)
                .child(label),
        )
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .truncate()
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .text_color(theme.muted_foreground)
                .child(resolved.title.clone()),
        )
        .into_any_element()
}

/// 挂起卡外壳：rounded-lg 行卡 + popover 底 + 1px 边框；内容按形态分发。
fn render_pending_card(
    state: &ConversationState,
    card: &PendingAsk,
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let body: AnyElement = match &card.payload {
        AskPayload::Question { questions, .. } => render_question_form(state, card, questions, cx),
        AskPayload::Approval {
            title,
            diff,
            command,
            fence_reason,
            plan_scope,
            audit,
            options,
            ..
        } => render_approval_form(card, title, diff.as_ref(), command.as_deref(), fence_reason.as_deref(), plan_scope.as_deref(), audit.as_ref(), options, cx),
        AskPayload::McpTrust {
            server,
            transport,
            fingerprint,
            command_or_url,
            options,
        } => {
            let transport_label = match transport {
                craftwave_engine::backend::ask::McpTransportKind::Stdio => t("stdio"),
                craftwave_engine::backend::ask::McpTransportKind::Http => t("http"),
            };
            render_trust_form(
                card,
                &t("MCP server trust").replace("{server}", server),
                &[
                    (t("Transport"), transport_label.to_owned()),
                    (t("Launch"), command_or_url.clone()),
                    (t("Fingerprint"), fingerprint.clone()),
                ],
                t("Trust this MCP server before connecting? It runs with your session's permissions."),
                options,
                cx,
            )
        }
        AskPayload::ProjectTrust {
            resource_name,
            scope,
            fingerprint,
            options,
        } => {
            let scope_label = match scope {
                craftwave_engine::backend::ask::SkillTrustScope::Global => t("Global skills"),
                craftwave_engine::backend::ask::SkillTrustScope::Project => t("Project skills"),
            };
            render_trust_form(
                card,
                &t("Project resource trust").replace("{resource}", resource_name),
                &[
                    (t("Source"), scope_label.to_owned()),
                    (t("Version fingerprint"), fingerprint.clone()),
                ],
                t("Trust this resource's current version? Untrusted skills are not loaded."),
                options,
                cx,
            )
        }
        AskPayload::ExternalDir { path, options } => render_trust_form(
            card,
            t("External directory access"),
            &[(t("Directory"), path.clone())],
            t("Allow read/write/list access to this directory and its subdirectories for this session?"),
            options,
            cx,
        ),
    };
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_2))
        .px(px(theme::SPACE_4))
        .py(px(theme::SPACE_3))
        .rounded(px(theme::rounded_lg(zoom)))
        .border_1()
        .border_color(theme.border)
        .bg(theme.popover)
        .debug_selector(move || format!("ask-card-{}", card.ask_id))
        .child(
            div()
                .text_size(px(theme::font_size(theme::FONT_UI_BASE, zoom)))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.foreground)
                .child(card.title()),
        )
        .child(body)
        .children(card.error.as_ref().map(|message| {
            div()
                .w_full()
                .px(px(theme::SPACE_2))
                .py(px(theme::SPACE_1))
                .rounded(px(theme::rounded_sm(zoom)))
                .border_1()
                .border_color(theme.danger.opacity(0.3))
                .bg(theme.danger.opacity(0.05))
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .text_color(theme.danger)
                .child(message.clone())
        }))
        .into_any_element()
}

/// 选项按钮行（全部形态通用）：放行类 primary 底、deny 边框款；推荐项带
/// 「推荐」角标；提交态禁点。点击经 defer 交给服务跑 `resolve_ask`。
fn render_option_buttons(
    card: &PendingAsk,
    options: &[craftwave_engine::backend::ask::AskOption],
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let ask_id = card.ask_id.clone();
    let mut row = div()
        .w_full()
        .flex()
        .flex_row()
        .flex_wrap()
        .justify_end()
        .gap(px(theme::SPACE_2));
    for option in options {
        let deny_like = option.mode == Some(craftwave_engine::backend::ask::AskMode::Deny)
            || option.id == "deny";
        let option_id = option.id.clone();
        let click_ask_id = ask_id.clone();
        let label: SharedString = if option.recommended == Some(true) {
            format!("{} · {}", option.label, t("recommended")).into()
        } else {
            option.label.clone().into()
        };
        let debug_id = SharedString::from(format!("ask-option-{}-{}", card.ask_id, option.id));
        row = row.child(
            div()
                .id(debug_id.clone())
                .debug_selector(move || debug_id.to_string())
                .cursor_pointer()
                .flex_shrink_0()
                .px(px(theme::SPACE_4))
                .py(px(theme::SPACE_1))
                .rounded(px(theme::ROUNDED_FULL))
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .when(card.submitted, |button| {
                    button.opacity(0.5)
                })
                .when(deny_like, |button| {
                    button
                        .border_1()
                        .border_color(theme.border)
                        .text_color(theme.foreground)
                        .hover(|style| style.border_color(theme.danger))
                })
                .when(!deny_like, |button| {
                    button
                        .bg(theme.primary)
                        .text_color(theme.primary_foreground)
                        .hover(|style| style.opacity(0.9))
                })
                .on_click(cx.listener(move |this: &mut ConversationState, _, _, cx| {
                    submit_approval_option(this, &click_ask_id, &option_id, cx);
                }))
                .child(label),
        );
    }
    row.into_any_element()
}

/// 审批类选项提交（trust/授权/审批三形态共用）：构造 Approval 应答交服务。
fn submit_approval_option(
    state: &mut ConversationState,
    ask_id: &str,
    option_id: &str,
    cx: &mut Context<ConversationState>,
) {
    let already = state
        .pending_asks()
        .iter()
        .find(|card| card.ask_id == ask_id)
        .is_some_and(|card| card.submitted);
    if already {
        return;
    }
    let answer = AskAnswer::Approval {
        option_id: option_id.to_owned(),
        edited_command: None,
    };
    dispatch_answer(state, ask_id.to_owned(), answer, cx);
}

/// 问卷提交：逐题收答案（选项 or 自由文本），未答齐不提交（按钮已禁，
/// 双重防御）。终态恒「已回答」。
fn submit_question(
    state: &mut ConversationState,
    card: &PendingAsk,
    cx: &mut Context<ConversationState>,
) {
    let AskPayload::Question { questions, .. } = &card.payload else {
        return;
    };
    let mut answers: BTreeMap<String, String> = BTreeMap::new();
    for question in questions {
        let free_text = state
            .ask_input(&card.ask_id, &question.id)
            .map(|input| input.read(cx).value().trim().to_owned())
            .unwrap_or_default();
        let answer = match (&question.options, question.allow_free_text) {
            (Some(options), _) => options
                .iter()
                .find(|choice| Some(&choice.id) == state.question_selection(card, &question.id))
                .map(|choice| choice.id.clone()),
            (None, true) => (!free_text.is_empty()).then(|| free_text.clone()),
            (None, false) => None,
        };
        // 选项题的附加自由输入：选了选项且另有文字时，文字拼在选项后
        //（协议 answers 值是单字符串——选项 id 优先）
        match answer {
            Some(value) => {
                answers.insert(question.id.clone(), value);
            }
            None => return, // 有题未答：放弃提交（按钮态已拦截，防御）
        }
    }
    dispatch_answer(
        state,
        card.ask_id.clone(),
        AskAnswer::Question { answers },
        cx,
    );
}

/// 统一提交出口：defer 到服务（同步语义的 resolve_ask 在服务侧跑——
/// 会话实体的 listener 栈内不能嵌套 update 服务正在借用的实体）。
fn dispatch_answer(
    state: &mut ConversationState,
    ask_id: String,
    answer: AskAnswer,
    cx: &mut Context<ConversationState>,
) {
    let Some(service) = state.service.clone() else {
        return;
    };
    cx.defer(move |cx| {
        service.update(cx, |service, cx| service.answer_ask(ask_id, answer, cx));
    });
}

/// 问卷表单：题目循环（选项 chips 单选 + 自由文本输入）+ 提交钮。
fn render_question_form(
    state: &ConversationState,
    card: &PendingAsk,
    questions: &[craftwave_engine::backend::ask::AskQuestion],
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let mut body = div().w_full().flex().flex_col().gap(px(theme::SPACE_3));
    let mut all_answered = true;
    for (q_ix, question) in questions.iter().enumerate() {
        let selected = state.question_selection(card, &question.id).cloned();
        let free_value = state
            .ask_input(&card.ask_id, &question.id)
            .map(|input| input.read(cx).value().trim().to_owned())
            .unwrap_or_default();
        let answered = selected.is_some() || !free_value.is_empty();
        all_answered = all_answered && answered;
        let mut item = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_1))
            .debug_selector(move || format!("ask-question-{}", question.id))
            .child(
                div()
                    .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                    .text_color(theme.foreground)
                    .child(question.text.clone()),
            );
        if let Some(options) = &question.options {
            let mut chips = div()
                .w_full()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(px(theme::SPACE_1));
            for choice in options {
                let active = selected.as_deref() == Some(choice.id.as_str());
                let choice_id = choice.id.clone();
                let question_id = question.id.clone();
                let ask_id = card.ask_id.clone();
                let chip_debug = format!("ask-choice-{}-{}", card.ask_id, choice.id);
                chips = chips.child(
                    div()
                        .id(SharedString::from(chip_debug.clone()))
                        .debug_selector(move || chip_debug.clone())
                        .cursor_pointer()
                        .px(px(theme::SPACE_3))
                        .py(px(theme::SPACE_1) / 2.0)
                        .rounded(px(theme::ROUNDED_FULL))
                        .border_1()
                        .border_color(if active {
                            theme.ring
                        } else {
                            theme.border
                        })
                        .bg(if active { theme.accent } else { theme.popover })
                        .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                        .text_color(theme.foreground)
                        .on_click(cx.listener(move |this: &mut ConversationState, _, _, cx| {
                            this.select_question_choice(&ask_id, &question_id, &choice_id, cx);
                        }))
                        .child(choice.label.clone()),
                );
            }
            item = item.child(chips);
        }
        if question.allow_free_text {
            if let Some(input) = state.ask_input(&card.ask_id, &question.id) {
                item = item.child(
                    Input::new(input)
                        .appearance(false)
                        .bordered(true)
                        .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom))),
                );
            }
            let _ = q_ix;
        }
        body = body.child(item);
    }
    let ask_id = card.ask_id.clone();
    let click_ask_id = ask_id.clone();
    body.child(
        div()
            .id(SharedString::from(format!("ask-submit-{}", card.ask_id)))
            .debug_selector(move || format!("ask-submit-{ask_id}"))
            .cursor_pointer()
            .flex_shrink_0()
            .self_end()
            .px(px(theme::SPACE_5))
            .py(px(theme::SPACE_1))
            .rounded(px(theme::ROUNDED_FULL))
            .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
            .when(all_answered && !card.submitted, |button| {
                button
                    .bg(theme.primary)
                    .text_color(theme.primary_foreground)
                    .hover(|style| style.opacity(0.9))
            })
            .when(!(all_answered && !card.submitted), |button| {
                button.opacity(0.4)
            })
            .on_click(cx.listener(move |this: &mut ConversationState, _, _, cx| {
                if let Some(card) = this
                    .pending_asks
                    .iter()
                    .find(|card| card.ask_id == click_ask_id)
                    .cloned()
                {
                    submit_question(this, &card, cx);
                }
            }))
            .child(t("Submit answer")),
    )
    .into_any_element()
}

/// 信任/授权类表单（McpTrust / ProjectTrust / ExternalDir 共用形态）：
/// 字段摘要块 + 说明行 + 选项按钮。
fn render_trust_form(
    card: &PendingAsk,
    _title: &str,
    fields: &[(&'static str, String)],
    explain: &str,
    options: &[craftwave_engine::backend::ask::AskOption],
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let mono = cx.theme().mono_font_family.clone();
    let mut summary = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_1))
        .px(px(theme::SPACE_3))
        .py(px(theme::SPACE_2))
        .rounded(px(theme::rounded_md(zoom)))
        .border_1()
        .border_color(theme.border)
        .id(SharedString::from(format!("ask-summary-{}", card.ask_id)))
        .max_h(px(SUMMARY_MAX_HEIGHT * zoom))
        .overflow_y_scroll();
    for (label, value) in fields {
        summary = summary
            .child(
                div()
                    .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                    .text_color(theme.muted_foreground)
                    .child(*label),
            )
            .child(
                div()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                    .font_family(mono.clone())
                    .text_color(theme.foreground)
                    .child(value.clone()),
            );
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_2))
        .child(summary)
        .child(
            div()
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .text_color(theme.muted_foreground)
                .child(explain.to_owned()),
        )
        .child(render_option_buttons(card, options, cx))
        .into_any_element()
}

/// 审批表单（`AskPayload::Approval` 全字段）：命令行 / 围栏原因 / 审计
/// 注记 / 计划范围对照 / diff 预览 + 选项按钮。正文完整可复制（纪律三：
/// 只有元数据可截断——diff 行超宽折行展示，不截内容）。
#[allow(clippy::too_many_arguments)]
fn render_approval_form(
    card: &PendingAsk,
    _title: &str,
    diff: Option<&craftwave_engine::backend::ask::ApprovalDiff>,
    command: Option<&str>,
    fence_reason: Option<&str>,
    plan_scope: Option<&str>,
    audit: Option<&craftwave_engine::backend::ask::AuditNote>,
    options: &[craftwave_engine::backend::ask::AskOption],
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let mono = cx.theme().mono_font_family.clone();
    let mut body = div().w_full().flex().flex_col().gap(px(theme::SPACE_2));
    // 命令行（命令类审批 / web 工具摘要行）：完整展示 + 复制完整原文
    if let Some(command) = command {
        body = body.child(code_block_with_copy(
            SharedString::from(format!("ask-copy-command-{}", card.ask_id)),
            "ask-command",
            command,
            &mono,
            cx,
        ));
    }
    // 围栏原因（警示行）
    if let Some(reason) = fence_reason {
        body = body.child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(theme::SPACE_1))
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .text_color(theme.warning)
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::CircleAlert)
                        .with_size(px(theme::ICON_SM))
                        .flex_shrink_0(),
                )
                .child(reason.to_owned()),
        );
    }
    // 灰区命令审计注记（ask=模型建议人工确认 / manual=审计失败转人工）
    if let Some(audit) = audit {
        let text = match audit.state {
            craftwave_engine::backend::ask::AuditNoteState::Ask => {
                t("Command audit suggests manual confirmation").replace("{reason}", &audit.reason)
            }
            craftwave_engine::backend::ask::AuditNoteState::Manual => {
                t("Command audit unavailable, confirm manually").replace("{reason}", &audit.reason)
            }
        };
        body = body.child(
            div()
                .w_full()
                .px(px(theme::SPACE_2))
                .py(px(theme::SPACE_1))
                .rounded(px(theme::rounded_sm(zoom)))
                .bg(theme.warning.opacity(0.10))
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .text_color(theme.warning)
                .debug_selector(move || format!("ask-audit-note-{}", card.ask_id))
                .child(text),
        );
    }
    // 计划范围门对照（范围卡）
    if let Some(path) = plan_scope {
        body = body.child(
            div()
                .w_full()
                .px(px(theme::SPACE_2))
                .py(px(theme::SPACE_1))
                .rounded(px(theme::rounded_sm(zoom)))
                .bg(theme.warning.opacity(0.10))
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .font_family(mono.clone())
                .text_color(theme.warning)
                .debug_selector(move || format!("ask-plan-scope-{}", card.ask_id))
                .child(t("Path outside approved plan").replace("{path}", path)),
        );
    }
    // diff 预览（逐行增删着色 + 复制全文；区内滚动防无限撑高）
    if let Some(diff) = diff {
        body = body.child(render_diff_preview(card, diff, &mono, cx));
    }
    body = body.child(render_option_buttons(card, options, cx));
    body.into_any_element()
}

/// diff 预览块：文件路径头 + 逐行着色（add=success / del=danger /
/// ctx=muted）+ 复制全文钮 + 区内滚动。
fn render_diff_preview(
    card: &PendingAsk,
    diff: &craftwave_engine::backend::ask::ApprovalDiff,
    mono: &SharedString,
    cx: &mut Context<ConversationState>,
) -> AnyElement {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let mut lines = div().w_full().flex().flex_col();
    for hunk in &diff.hunks {
        lines = lines.child(
            div()
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(theme.muted_foreground)
                .child(hunk.header.clone()),
        );
        for line in &hunk.lines {
            let (sign, color) = match line.kind {
                craftwave_engine::backend::ask::DiffLineKind::Add => ("+", theme.success),
                craftwave_engine::backend::ask::DiffLineKind::Del => ("-", theme.danger),
                craftwave_engine::backend::ask::DiffLineKind::Ctx => (" ", theme.muted_foreground),
            };
            lines = lines.child(
                div()
                    .w_full()
                    .min_w_0()
                    .whitespace_normal()
                    .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                    .font_family(mono.clone())
                    .text_color(color)
                    .child(format!("{sign}{}", line.text)),
            );
        }
    }
    // 复制全文：hunk 头 + 逐行原文（不修饰符号——原文即 diff 文本面）
    let mut full = String::new();
    for hunk in &diff.hunks {
        full.push_str(&hunk.header);
        full.push('\n');
        for line in &hunk.lines {
            full.push_str(&line.text);
            full.push('\n');
        }
    }
    let full_copy = full;
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_1))
        .debug_selector(move || format!("ask-diff-{}", card.ask_id))
        .child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::SPACE_2))
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(theme.muted_foreground)
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .truncate()
                        .font_family(mono.clone())
                        .child(diff.path.clone()),
                )
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "ask-copy-diff-{}",
                            card.ask_id
                        )))
                        .flex_shrink_0()
                        .px(px(theme::SPACE_1))
                        .rounded(px(theme::rounded_sm(zoom)))
                        .cursor_pointer()
                        .text_color(theme.muted_foreground)
                        .hover(|style| style.bg(theme.list_hover).text_color(theme.foreground))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(t("Copy full diff"))
                                .build(window, cx)
                        })
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(full_copy.clone()));
                        })
                        .child(t("Copy full diff")),
                ),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .px(px(theme::SPACE_3))
                .py(px(theme::SPACE_2))
                .rounded(px(theme::rounded_sm(zoom)))
                .bg(theme.muted)
                .id(SharedString::from(format!("ask-diff-scroll-{}", card.ask_id)))
                .max_h(px(DIFF_MAX_HEIGHT * zoom))
                .overflow_y_scroll()
                .child(lines),
        )
        .into_any_element()
}

/// 等宽代码块（命令行展示位）+ 复制完整原文（纪律三：正文完整可复制）。
/// `debug` 是测试定位钩子名。
fn code_block_with_copy(
    copy_id: SharedString,
    debug: &'static str,
    text: &str,
    mono: &SharedString,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme().colors;
    let zoom = cx.craftwave().zoom;
    let full = text.to_owned();
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_1))
        .debug_selector(|| format!("{debug}-block"))
        .child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::SPACE_2))
                .child(div().flex_1())
                .child(
                    div()
                        .id(copy_id)
                        .debug_selector(move || format!("{debug}-copy"))
                        .flex_shrink_0()
                        .px(px(theme::SPACE_1))
                        .rounded(px(theme::rounded_sm(zoom)))
                        .cursor_pointer()
                        .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                        .text_color(theme.muted_foreground)
                        .hover(|style| style.bg(theme.list_hover).text_color(theme.foreground))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(t("Copy full command"))
                                .build(window, cx)
                        })
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(full.clone()));
                        })
                        .child(t("Copy full command")),
                ),
        )
        .child(
            div()
                .w_full()
                .min_w_0()
                .px(px(theme::SPACE_3))
                .py(px(theme::SPACE_2))
                .rounded(px(theme::rounded_sm(zoom)))
                .bg(theme.muted)
                .whitespace_normal()
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .font_family(mono.clone())
                .text_color(theme.foreground)
                .child(text.to_owned()),
        )
        .into_any_element()
}

// ---------------------------------------------------------------------
// 问卷选择状态（chips 单选；挂在 ConversationState 上的小表）
// ---------------------------------------------------------------------

impl ConversationState {
    /// 某题当前选中的选项 id（None = 未选）。
    pub(crate) fn question_selection(
        &self,
        card: &PendingAsk,
        question_id: &str,
    ) -> Option<&String> {
        self.question_selections
            .get(&(card.ask_id.clone(), question_id.to_owned()))
    }

    /// 点选一个选项（单选语义；再点已选项 = 取消选择）。
    pub(crate) fn select_question_choice(
        &mut self,
        ask_id: &str,
        question_id: &str,
        choice_id: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (ask_id.to_owned(), question_id.to_owned());
        let already = self
            .question_selections
            .get(&key)
            .is_some_and(|current| current == choice_id);
        if already {
            self.question_selections.remove(&key);
        } else {
            self.question_selections.insert(key, choice_id.to_owned());
        }
        cx.notify();
    }
}
