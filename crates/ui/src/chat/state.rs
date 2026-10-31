//! 消息流底座（gpui-rewrite 2.4）：会话状态 Entity + ListState 虚拟列表。
//!
//! ## 数据源（rendering.md「数据层」）
//!
//! - 历史消息：`Rc<Vec<Rc<MessageEntry>>>` 克隆共享（快照的活动路径线性序），
//!   渲染路径逐行借用，零深拷贝；
//! - 流式末条：`Option<StreamingMessage>`，挂在历史之后作为最后一行；
//! - 行号 = 消息下标（流式行 = 历史长度）：**过滤器不改行号映射**——被纪律二
//!   滤掉的行渲染为零高空 div（Wake 7128-7135 返回空 div 同款），这样展开
//!   状态两张 `HashSet<行号>` 的键与 ListState 行号永远一一对应。
//!
//! ## ListState 路径（spike 03 号票 + gpui list 内建 follow-tail）
//!
//! - `ListState::new(0, ListAlignment::Bottom, px(512))`：聊天日志从底部对齐，
//!   512px overscan 惰性测量不等高行；容器必须 `.size_full()`（spike 踩平的
//!   坑：list 默认 ListSizingBehavior::Auto，高度全凭样式，不给就是 0）；
//! - `set_follow_mode(FollowMode::Tail)`：新消息到达自动贴底跟随；用户上滚
//!   自动暂停（`delta.y > 0` 即停），滚回底部自动续跟——两个行为都是 list
//!   内建语义，本模块只负责初始化与在行数/行高变化时走对路径；
//! - 行数变化：`splice(old_range, count)`（追加 = 尾部插一格 / 收口 = 尾部
//!   删一格 / 整体替换 = `reset`）；
//! - 行高变化：展开折叠手工 `splice(ix..ix+1, 1)` 重测（纪律一，Wake 7191
//!   平移）、流式内容增长 `remeasure_items(ix..ix+1)`、缩放换档 `remeasure()`。
//!
//! ## 选择与滚动（rendering.md「选择与滚动」）
//!
//! gpui-base 的文本选择机制在「拖拽选择到视口边缘」时用 gpui-component
//! `AutoScroll::compute_delta`（16px 触发区 + 80px 坡道）合成 `ScrollEvent`
//! 派发进窗口，list 的滚轮处理器原生消费同一事件类型——只要行内有
//! 可选文本（`.selectable`）且本列表可滚，边缘自动滚动即成立。本批正文是
//! 普通文本占位（下一批换 `TextView::markdown` 才挂 `.selectable(true)`），
//! 事件通路已由测试钉住（见 conversation_test.rs 的合成 ScrollEvent 用例）。

use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::{Context, Entity, FollowMode, IntoElement, ListAlignment, ListState, px};

use craftwave_engine::snapshot::{StreamPart, StreamingMessage};
use craftwave_engine::storage::{MessageEntry, MessageRole, WirePart};

use super::row;

/// 阅读轴最大宽度（px）：全消息 `justify_center + max_w` 共用同一竖直轴
/// （rendering.md「行分发」）。与详情头共用轴是阶段 3.5 面板族的事，先钉值。
pub const READER_MAX_WIDTH: f32 = 720.0;

/// User 气泡最大宽度（rendering.md 钉值 540px）。
pub const USER_BUBBLE_MAX_WIDTH: f32 = 540.0;

/// 虚拟列表 overscan（rendering.md「数据层」钉值 512px，白名单②：惰性
/// 测量的预渲染窗口；不等高行上下各预渲染 512px）。
pub const LIST_OVERSCAN: f32 = 512.0;

/// 会话状态：消息源 + 展开状态 + ListState + Composer。Entity 承载，
/// `cx.notify` 驱动重渲染；行内容由 [`Self::render_row`] 按角色分发
/// （实现见 row.rs）。本类型同时是中栏视图（Render 实现在 mod.rs）——
/// gpui 的 list 元素长在本实体的元素树里，行数/行高变化后的 `cx.notify`
/// 必须落在这里才能让列表重排（spike 单实体模式的原因）。
pub struct ConversationState {
    /// 历史消息（活动路径线性序；Rc 克隆共享）。
    messages: Rc<Vec<Rc<MessageEntry>>>,
    /// 流式中的 run 渲染态（None = 无活跃 run / 已收口）。
    streaming: Option<StreamingMessage>,
    /// thinking 展开表（键 = 行号；跨重渲染记忆，废除一切自动收起）。
    expanded_thinking: HashSet<usize>,
    /// 工具簇展开表（键 = 行号；与 thinking 相互独立，折叠不嵌套）。
    expanded_tools: HashSet<usize>,
    /// 虚拟列表状态（Bottom 对齐 + 512px overscan + Tail 跟随）。
    /// mod.rs 的 Render 实现要读它建 list 元素，故 crate 内可见。
    pub(crate) list: ListState,
    /// Composer 实体（install 装配；中栏列的下半段）。
    pub(crate) composer: Option<Entity<super::composer::ComposerState>>,
    /// 引擎会话服务（install 装配；None = 回声模式的测试装置）。
    /// Enter 分流经它走真发送链；服务持本实体的弱引用（反向），
    /// 这里持强引用不构成环。
    pub(crate) service: Option<Entity<super::service::SessionService>>,
    /// 上次见过的缩放档（render 每帧比对，换档触发全列表重测）。
    pub(crate) zoom_seen: f32,
    /// splice(ix..ix+1, 1) 重测执行次数（对账计数：测试断言「展开折叠触发了
    /// 重测」，生产路径零成本自增）。
    pub(crate) splice_retest_count: usize,
    /// remeasure_items 执行次数（流式行内容增长的重测对账计数）。
    pub(crate) remeasure_count: usize,
    /// view 寿命订阅的保活句柄（composer 输入事件 → 发送/补全分流，
    /// install 时挂上）。Subscription 一旦 drop 会取消订阅——必须存进
    /// owner 而不是 detach（detach 后回调挂在 emitter 上，owner 重建时
    /// 会累积旧回调，编码指南 §1）。
    #[allow(dead_code)] // 保活语义：持有即订阅在，从不读取
    pub(crate) _subscriptions: Vec<gpui_kit::Subscription>,
}

impl ConversationState {
    /// 空会话：列表 0 行，开启 Tail 跟随（新消息到达即贴底）。
    pub fn new() -> Self {
        let list = ListState::new(0, ListAlignment::Bottom, px(LIST_OVERSCAN));
        list.set_follow_mode(FollowMode::Tail);
        Self {
            messages: Rc::new(Vec::new()),
            streaming: None,
            expanded_thinking: HashSet::new(),
            expanded_tools: HashSet::new(),
            list,
            composer: None,
            service: None,
            // 从 0 起步：首帧 changed 必真，列表按当前档重测一次
            zoom_seen: 0.0,
            splice_retest_count: 0,
            remeasure_count: 0,
            _subscriptions: Vec::new(),
        }
    }

    /// Composer 实体（mod.rs 的 install 装配后存在；测试取它驱动输入行为。
    /// 生产面 Render 直接读字段）。
    #[cfg(test)]
    pub(crate) fn composer(&self) -> Option<&Entity<super::composer::ComposerState>> {
        self.composer.as_ref()
    }

    /// 当前行数 = 历史条数 + 流式行（有则 1）。行号即消息下标。
    pub fn row_count(&self) -> usize {
        self.messages.len() + usize::from(self.streaming.is_some())
    }

    /// 历史消息列表（只读借用；行分发与测试用）。
    pub fn messages(&self) -> &[Rc<MessageEntry>] {
        &self.messages
    }

    /// 流式末条（只读借用）。
    pub fn streaming(&self) -> Option<&StreamingMessage> {
        self.streaming.as_ref()
    }

    /// thinking 展开表当前是否含该行。
    pub fn thinking_expanded(&self, ix: usize) -> bool {
        self.expanded_thinking.contains(&ix)
    }

    /// 工具簇展开表当前是否含该行。
    pub fn tools_expanded(&self, ix: usize) -> bool {
        self.expanded_tools.contains(&ix)
    }

    // ==================================================================
    // 快照驱动路径（2.C-1 假数据与 2.C-2 真帧流共用的唯一写入口）
    // ==================================================================

    /// 整体替换历史消息（会话加载 / run 落盘重载后的新历史）。走 `reset`：
    /// 行数与行高缓存全部推倒，滚动由 Tail 跟随接管回底部。
    /// 展开表按行号记忆，换历史即换内容——一并清空。
    pub fn set_history(&mut self, history: Rc<Vec<Rc<MessageEntry>>>, cx: &mut Context<Self>) {
        let count = history.len();
        self.messages = history;
        self.expanded_thinking.clear();
        self.expanded_tools.clear();
        self.list.reset(count);
        cx.notify();
    }

    /// 回声式追加一条消息（本批假发送通道；真 start_chat 在 2.C-2 会改为
    /// 「落盘重载 → apply_snapshot」同一形态）。尾部 `splice(n..n, 1)` 插一格，
    /// Tail 跟随下列表自动贴底；若流式行在场，它整体后移一格，行号映射
    /// （流式行 = messages.len()）保持正确。
    pub fn append_message(&mut self, entry: Rc<MessageEntry>, cx: &mut Context<Self>) {
        let mut vec = (*self.messages).clone();
        vec.push(entry);
        self.messages = Rc::new(vec);
        let n = self.messages.len();
        self.list.splice(n - 1..n - 1, 1);
        cx.notify();
    }

    /// 设置 / 更新 / 收起流式末条：
    /// - 出现：尾部插一格；收口：尾部删一格（随后壳会以落盘重载的新历史
    ///   调 `set_history`，流式行内容由历史行接管）；
    /// - 内容增长（打字机增量 / 工具增量）：行高会变，走
    ///   `remeasure_items(ix..ix+1)`——不动行数、不炸滚动位置（这正是
    ///   spike「流式更新走 ListState 刷新路径」的精确形态）。
    pub fn set_streaming(
        &mut self,
        next: Option<StreamingMessage>,
        cx: &mut Context<Self>,
    ) {
        let ix = self.messages.len();
        match (&self.streaming, &next) {
            (None, Some(_)) => self.list.splice(ix..ix, 1),
            (Some(_), None) => {
                self.list.splice(ix..ix + 1, 0);
                // 该行的展开记忆随行内容一起消失
                self.expanded_thinking.remove(&ix);
                self.expanded_tools.remove(&ix);
            }
            (Some(old), Some(new)) => {
                if old != new {
                    self.list.remeasure_items(ix..ix + 1);
                    self.remeasure_count += 1;
                }
            }
            (None, None) => {}
        }
        self.streaming = next;
        cx.notify();
    }

    /// 完整快照入口（2.C-2 帧流的产出口）：历史指针不同才整体替换（同指针
    /// 的快照不值得 reset 打掉滚动状态），流式态按差异路由到 `set_streaming`。
    /// 历史以 `Rc<Vec<Rc<MessageEntry>>>` 传入（壳从 SessionTreeData 活动路径
    /// 构建时即包 Rc——`FrameAggregator::ticker` 的 history 参数同形态），
    /// 指针相等判定让同历史的重复快照零成本通过。
    pub fn apply_snapshot(
        &mut self,
        history: Rc<Vec<Rc<MessageEntry>>>,
        streaming: Option<StreamingMessage>,
        cx: &mut Context<Self>,
    ) {
        if !Rc::ptr_eq(&self.messages, &history) {
            self.set_history(history, cx);
        }
        if self.streaming != streaming {
            self.set_streaming(streaming, cx);
        }
    }

    // ==================================================================
    // 展开状态（纪律一：折叠只有 thinking / 工具簇，跨重渲染记忆）
    // ==================================================================

    /// 切换 thinking 面板展开：置位/除名 + 手工重测该行（行高变了）。
    pub fn toggle_thinking(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !self.expanded_thinking.remove(&ix) {
            self.expanded_thinking.insert(ix);
        }
        self.retest_row(ix, cx);
    }

    /// 切换工具簇卡展开：同上。
    pub fn toggle_tools(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !self.expanded_tools.remove(&ix) {
            self.expanded_tools.insert(ix);
        }
        self.retest_row(ix, cx);
    }

    /// 行高变更重测：`splice(ix..ix+1, 1)` 把该行标记回 Unmeasured，下个
    /// 布局帧重新测高（Wake 7191 平移）。滚动位置由 splice 的锚点算术保持。
    fn retest_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.list.splice(ix..ix + 1, 1);
        self.splice_retest_count += 1;
        cx.notify();
    }

    /// 全列表重测（spec 2.3 缩放钩子）：字号档变化后行高缓存整体失效。
    /// ChatView 的 render 每帧比对 zoom 档（ui_zoom::changed）后调这里。
    pub fn remeasure_all(&mut self, cx: &mut Context<Self>) {
        self.list.remeasure();
        cx.notify();
    }

    // ==================================================================
    // 行分发（四形态 + 过滤器；元素构建在 row.rs，这里只做判定与路由）
    // ==================================================================

    /// 纪律二过滤器：运行日志型 = Assistant 且无正文、无工具、无图
    /// （parts 只剩 thinking，或为空）。源数据保留，只是不渲染。
    pub fn is_log_entry(entry: &MessageEntry) -> bool {
        entry.role == MessageRole::Assistant
            && entry
                .parts
                .iter()
                .all(|p| matches!(p, WirePart::Reasoning { .. }))
    }

    /// 流式行的同口径过滤器（流式消息无图；无正文且无工具 = 日志型）。
    pub fn is_log_streaming(streaming: &StreamingMessage) -> bool {
        streaming
            .parts
            .iter()
            .all(|p| matches!(p, StreamPart::Reasoning { .. }))
    }

    /// 该行是否被过滤器跳过（渲染为零高空行）。
    pub fn row_filtered(&self, ix: usize) -> bool {
        match self.messages.get(ix) {
            Some(entry) => Self::is_log_entry(entry),
            None => self
                .streaming
                .as_ref()
                .is_some_and(Self::is_log_streaming),
        }
    }

    /// 渲染一行（gpui::list 的行回调入口）。四形态 + 过滤器：
    /// User 气泡 / Assistant 全宽列 / System 胶囊 / 日志型零高空行。
    pub fn render_row(&self, ix: usize, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        if self.row_filtered(ix) {
            // 纪律二：滤掉的行返回零高空 div，不渲染内容（源数据保留）
            return div_empty_row();
        }
        row::render_row(self, ix, cx)
    }
}

/// 零高空行（过滤器产物）：任何样式都不给，布局测高即为 0。
fn div_empty_row() -> gpui_kit::AnyElement {
    gpui_kit::div().into_any_element()
}

/// 展示槽位模型：一条 Assistant 消息渲染成 thinking → 正文 → 工具簇三槽位
/// （rendering.md「行分发」钉死的顺序；多段 reasoning / 多工具各自合并——
/// 纪律一：每条消息一张簇卡，thinking 面板在正文之前）。
pub(crate) enum Slot {
    /// thinking 面板（合并该消息全部 Reasoning；折叠态由展开表决定）。
    Thinking { text: String },
    /// 正文（合并该消息全部 Text；本批普通文本占位，下一批换
    /// TextView::markdown —— 届时同一槽位换构造即可）。
    Body { text: String },
    /// 工具簇卡（该消息全部工具调用一张卡；折叠态由展开表决定）。
    Tools { calls: Vec<CallLite> },
}

/// 单个工具调用的展示面（WirePart 与 StreamPart 归一后的形态）。
pub(crate) struct CallLite {
    pub name: String,
    pub args_json: String,
    /// None = 仍在执行 / 等待结果。
    pub ok: Option<bool>,
    pub summary: String,
}

/// WirePart 序列 → 槽位序列（历史消息）。工具结果与调用按 call_id 配对
/// （wire 里 ToolResult 紧随对应 ToolCall 到达，倒查即可）。
pub(crate) fn slots_from_wire(parts: &[WirePart]) -> Vec<Slot> {
    let mut thinking = String::new();
    let mut body = String::new();
    let mut calls: Vec<CallLite> = Vec::new();
    for part in parts {
        match part {
            WirePart::Reasoning { text } => {
                if !thinking.is_empty() {
                    thinking.push_str("\n\n");
                }
                thinking.push_str(text);
            }
            WirePart::Text { text } => {
                body.push_str(text);
            }
            WirePart::ToolCall {
                call_id,
                name,
                args_json,
            } => {
                // 倒查同 id 的 ToolResult（到达序保证在后面不远）
                let result = parts.iter().rev().find_map(|p| match p {
                    WirePart::ToolResult {
                        call_id: id,
                        ok,
                        summary,
                    } if id == call_id => Some((*ok, summary.clone())),
                    _ => None,
                });
                calls.push(CallLite {
                    name: name.clone(),
                    args_json: args_json.clone(),
                    ok: result.as_ref().map(|(ok, _)| *ok),
                    summary: result
                        .map(|(_, summary)| summary)
                        .unwrap_or_default(),
                });
            }
            // 工具结果已并入调用、图片不进 Assistant 正文面（气泡面归 User）
            WirePart::ToolResult { .. } | WirePart::Image { .. } => {}
            // 压缩快照的展示面随 3.3 会话树切片另接（本批不渲染）
            WirePart::PlanSnapshot { .. } | WirePart::RunMarker { .. } => {}
        }
    }
    finish_slots(thinking, body, calls)
}

/// StreamPart 序列 → 槽位序列（流式行）。结果内嵌在 ToolCall part，
/// 无需配对。
pub(crate) fn slots_from_stream(parts: &[StreamPart]) -> Vec<Slot> {
    let mut thinking = String::new();
    let mut body = String::new();
    let mut calls: Vec<CallLite> = Vec::new();
    for part in parts {
        match part {
            StreamPart::Reasoning { text } => {
                if !thinking.is_empty() {
                    thinking.push_str("\n\n");
                }
                thinking.push_str(text);
            }
            StreamPart::Text { text } => body.push_str(text),
            StreamPart::ToolCall {
                name,
                args_json,
                result,
                ..
            } => calls.push(CallLite {
                name: name.clone(),
                args_json: args_json.clone(),
                ok: result.as_ref().map(|r| r.ok),
                summary: result
                    .as_ref()
                    .map(|r| r.ui_summary.clone())
                    .unwrap_or_default(),
            }),
        }
    }
    finish_slots(thinking, body, calls)
}

/// 归一收口：空槽位丢弃，顺序固定 thinking → 正文 → 工具簇。
fn finish_slots(thinking: String, body: String, calls: Vec<CallLite>) -> Vec<Slot> {
    let mut slots = Vec::new();
    if !thinking.is_empty() {
        slots.push(Slot::Thinking { text: thinking });
    }
    if !body.is_empty() {
        slots.push(Slot::Body { text: body });
    }
    if !calls.is_empty() {
        slots.push(Slot::Tools { calls });
    }
    slots
}
