//! 会话侧栏（gpui-rewrite 阶段 3.1「多会话 + 侧栏」）。
//!
//! ## 数据面与分组语义（照旧栈 `SessionWorkspace.tsx` 的行为平移）
//!
//! - 会话清单 = engine `list_sessions`（updatedAt 倒序）；项目 = `list_projects`；
//! - **messageCount = 0 的会话不进列表**（旧栈 `listed()` 纪律：新建未发送的
//!   会话不可见，首条消息落盘索引推进后才可见）；
//! - 分组 = 临时会话组（projectId None）+ 项目组（每组头下挂名下会话）——
//!   engine 不提供分组面，分组是 UI 侧按 `project_id` 派生的（旧栈同款）；
//! - 行状态槽：活动会话高亮（sidebar_accent 底）/ 生成中指示（当前会话且
//!   `SessionService::busy`，任务原文的数据面）/ 标题溢出渐隐（rendering.md
//!   UX 方案 C1）/ 相对时间（smart_time）/ 项目组头副行路径（tilde_path）；
//! - 「最后消息摘要（one_line）」槽位：`SessionMeta` 无 last-message 字段
//!   （engine index schema，本切片不动 engine），主行显示标题（自动起名后
//!   即摘要语义），次行显示消息数——缺口在交付报告说明。
//!
//! ## 稳定 ID 与虚拟化
//!
//! - 行 ElementId = 领域 ID：会话行用会话 id、组头用组键（**禁止下标**，
//!   coding-guides §2 红线）；
//! - 列表虚拟化：`ListState`（与消息流同款，不等高惰性测量）——行序列
//!   [`RowItem`] 由 [`build_rows`] 从数据派生，行号与可见位置解耦。
//!
//! ## 溢出渐隐（rendering.md C1，核实结论）
//!
//! gpui-pre 0.3.10 **没有 CSS `mask-image`**（alpha 遮罩）能力，`linear_gradient`
//! 只是背景绘制。等效手法：标题区右缘叠一条「侧栏底色 透明→不透明」的
//! 渐变覆盖条——文本被盖住的部分自然淡出，视觉即「右缘渐隐」而非省略号
//! （侧栏行底是纯色 sidebar，覆盖条与底同色无缝）。列表滚动渐隐（旧栈
//! `useScrollFade` 上下 24px）同手法，按 `ListState` 的滚动位置显隐。

use std::sync::Arc;

use gpui_kit::ListState;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::dialog::AlertDialog;
use gpui_kit::component::Icon;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::prelude::*;
use gpui_kit::{
    App, Context, Entity, FontWeight, Hsla, SharedString, Subscription, WeakEntity, Window,
    div, linear_color_stop, linear_gradient, px,
};

use craftwave_engine::api::session::{
    self, DeleteSessionArgs, ListSessionsArgs, RenameSessionArgs,
};
use craftwave_engine::app_state::AppState;
use craftwave_engine::storage::{ProjectMeta, SessionMeta};

use crate::chat::{EngineService, SessionService};
use crate::i18n::t;
use crate::text_util::{smart_time, tilde_path};
use crate::theme::{self, ActiveCraftwave as _};

#[cfg(test)]
#[path = "sidebar_test.rs"]
mod sidebar_test;

/// 标题渐隐覆盖条宽度（基准 px，随 zoom）：文本尾部被它压入侧栏底色，
/// 视觉为「右缘渐隐」。
const TITLE_FADE_WIDTH: f32 = 24.0;
/// 列表上下滚动渐隐的高度（旧栈 useScrollFade 的 24px 同值）。
const LIST_FADE_HEIGHT: f32 = 24.0;

/// 列表行序列的一项：组头或会话行（行号 → 领域对象的桥，`build_rows` 派生）。
#[derive(Clone, Debug)]
pub(crate) enum RowItem {
    /// 分组头：组键（稳定 ID 面）+ 标题 + 可选副行（项目组的 tilde 路径）。
    Group {
        id: SharedString,
        label: SharedString,
        subtitle: Option<String>,
    },
    /// 会话行。
    Session(SessionMeta),
}

/// 列表行序列（3.1 分组语义）：过滤未发送会话 → 临时组 + 项目分组。
/// 顺序：engine `list_sessions` 已按 updatedAt 倒序，过滤保序；项目组
/// 按 `list_projects` 的注册序（旧栈同款）。
pub(crate) fn build_rows(sessions: &[SessionMeta], projects: &[ProjectMeta]) -> Vec<RowItem> {
    let mut rows = Vec::new();
    let temporaries: Vec<&SessionMeta> = sessions
        .iter()
        .filter(|m| m.message_count > 0 && m.project_id.is_none())
        .collect();
    rows.push(RowItem::Group {
        id: "group-temporary".into(),
        label: t("Temporary sessions").into(),
        subtitle: None,
    });
    rows.extend(temporaries.into_iter().map(|m| RowItem::Session(m.clone())));
    for project in projects {
        rows.push(RowItem::Group {
            id: SharedString::from(format!("group-{}", project.id)),
            label: project.name.clone().into(),
            subtitle: Some(tilde_path(&project.home_dir)),
        });
        rows.extend(
            sessions
                .iter()
                .filter(|m| {
                    m.message_count > 0 && m.project_id.as_deref() == Some(project.id.as_str())
                })
                .map(|m| RowItem::Session(m.clone())),
        );
    }
    rows
}

/// 会话侧栏实体：清单数据 + 行序列 + ListState + 行内编辑态 + 删除确认。
pub struct SidebarState {
    /// engine 应用状态（清单读面；install 装配注入）。
    state: Arc<AppState>,
    /// 会话服务（切会话 / busy 面 / 新建；弱引用——服务在 Global 常驻，
    /// 侧栏随窗口生死）。
    service: WeakEntity<SessionService>,
    /// 会话清单（engine updatedAt 倒序）。
    sessions: Vec<SessionMeta>,
    /// 项目注册表（组头数据）。
    projects: Vec<ProjectMeta>,
    /// 首次装载是否完成（未完成不闪「还没有会话」空态）。
    loaded: bool,
    /// 行序列（refresh 后派生；ListState 行号 → RowItem）。
    rows: Vec<RowItem>,
    /// 虚拟列表状态（不等高惰性测量；与消息流同款）。
    list: ListState,
    /// 刷新代次（过期回写丢弃的对账键——coding-guides §5）。
    list_revision: usize,
    /// 行内重命名编辑态（Some = 正在编辑该会话；共享一个 InputState）。
    renaming: Option<String>,
    /// 重命名输入框（install 时建——InputState::new 要 &mut Window）。
    rename_input: Entity<InputState>,
    /// 删除确认目标（Some = 确认对话框打开）。
    confirming_delete: Option<SessionMeta>,
    /// 上次见过的缩放档（换档触发列表重测，spec 2.3 钩子）。
    zoom_seen: f32,
    /// 订阅保活（重命名输入框 Enter/Blur → 提交；编码指南 §1）。
    _subscriptions: Vec<Subscription>,
}

/// 侧栏装配（开窗闭包内调用，与 `chat::install` 并列，且须在其后——依赖
/// EngineService Global）：建实体 + 重命名输入框（要 window）+ 订阅输入
/// 事件 + 首次装载。
pub fn install(
    window: &mut Window,
    cx: &mut App,
    engine: &crate::engine::EngineBridge,
) -> Entity<SidebarState> {
    let rename_input = cx.new(|cx| InputState::new(window, cx));
    // 服务实体在 chat::install 时挂 Global（main.rs 同一闭包内顺序保证）
    let service = cx.global::<EngineService>().0.clone();
    let sidebar = cx.new(|_| SidebarState::new(engine.state.clone(), service, rename_input));
    sidebar.update(cx, |sidebar, cx| {
        // 重命名输入框的事件订阅（Enter / 失焦提交）；订阅存 owner 保活
        let input = sidebar.rename_input.clone();
        let subscription = cx.subscribe_in(
            &input,
            window,
            |sidebar: &mut SidebarState,
             _input: &Entity<InputState>,
             event: &InputEvent,
             _window: &mut Window,
             cx: &mut Context<SidebarState>| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    sidebar.commit_rename(cx);
                }
            },
        );
        sidebar._subscriptions.push(subscription);
        sidebar.refresh(cx);
    });
    sidebar
}

impl SidebarState {
    /// 直接构造（生产经 [`install`]；测试装置用它注入 tempdir 状态与
    /// 会话清单，不依赖窗口闭包的装配序）。
    pub(crate) fn new(
        state: Arc<AppState>,
        service: Entity<SessionService>,
        rename_input: Entity<InputState>,
    ) -> Self {
        Self {
            state,
            service: service.downgrade(),
            sessions: Vec::new(),
            projects: Vec::new(),
            loaded: false,
            rows: Vec::new(),
            list: ListState::new(0, gpui_kit::ListAlignment::Top, px(512.0)),
            list_revision: 0,
            renaming: None,
            rename_input,
            confirming_delete: None,
            zoom_seen: 0.0,
            _subscriptions: Vec::new(),
        }
    }

    // ==================================================================
    // 数据装载（engine 会话/项目清单 → 行序列）
    // ==================================================================

    /// 刷新清单：engine 两个同步读命令跑后台线程池（index.json 读盘不占
    /// 主线程），回写实体并重排行序列。回写按刷新代次对账（期间又发起过
    /// 刷新的旧结果丢弃——coding-guides §5）。
    pub(crate) fn refresh(&mut self, cx: &mut Context<Self>) {
        let state = self.state.clone();
        self.list_revision += 1;
        let revision = self.list_revision;
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_spawn(async move {
                    let sessions =
                        session::list_sessions(&state, ListSessionsArgs::default()).ok();
                    let projects = session::list_projects(&state).ok();
                    (sessions, projects)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.list_revision != revision {
                    return; // 过期回写：期间又发起过刷新
                }
                let (sessions, projects) = loaded;
                if let Some(sessions) = sessions {
                    this.sessions = sessions;
                }
                if let Some(projects) = projects {
                    this.projects = projects;
                }
                this.loaded = true;
                this.rebuild_rows(cx);
            });
        })
        .detach();
    }

    /// 行序列重排 + ListState 行数对账（行数变化才 reset——保滚动位置；
    /// 行数不变的内容更新由重渲自然刷新）。
    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        self.rows = build_rows(&self.sessions, &self.projects);
        if self.list.item_count() != self.rows.len() {
            self.list.reset(self.rows.len());
        }
        cx.notify();
    }

    // ==================================================================
    // 行操作：切会话 / 重命名 / 删除 / 新建
    // ==================================================================

    /// 点击会话行：走服务 `open_session`（历史装载 + attach 续流 +
    /// ConversationState 换血），随后刷新清单（updatedAt 排序变化跟进）。
    pub(crate) fn open_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        let Some(service) = self.service.upgrade() else {
            return;
        };
        let sid = session_id;
        // 侧栏自身 update 栈内不能嵌套 update 服务再回写会话实体链——
        // defer 推迟到本轮结束（与 Enter 发送分流同款手法）
        cx.defer(move |cx| {
            service.update(cx, |service, cx| service.open_session(sid, cx));
        });
        self.refresh(cx);
    }

    /// 进入行内重命名：预填当前标题并聚焦（共享输入框，一次编辑一行）。
    pub(crate) fn begin_rename(
        &mut self,
        session_id: String,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rename_input.update(cx, |input, cx| {
            input.set_value(title, window, cx);
            input.focus(window, cx);
        });
        self.renaming = Some(session_id.clone());
        // 编辑框比普通行高：该行重测（ListState 的测高缓存不会自己失效）
        if let Some(ix) = self.session_row_ix(&session_id) {
            self.list.splice(ix..ix + 1, 1);
        }
        cx.notify();
    }

    /// 提交重命名（Enter / 失焦）：空白或未变更直接退出编辑态；变更调
    /// engine `rename_session` 后刷新清单。失败同样退出编辑态并日志留痕
    /// （输入已失焦；错误面随阶段 3.7 设置面的 dialog 链路再补）。
    pub(crate) fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(session_id) = self.renaming.take() else {
            return;
        };
        let title = self.rename_input.read(cx).value().trim().to_owned();
        if let Some(ix) = self.session_row_ix(&session_id) {
            self.list.splice(ix..ix + 1, 1);
        }
        if title.is_empty() {
            cx.notify();
            return;
        }
        let unchanged = self
            .sessions
            .iter()
            .find(|m| m.id == session_id)
            .is_some_and(|m| m.title == title);
        if unchanged {
            cx.notify();
            return;
        }
        if let Err(err) = session::rename_session(
            &self.state,
            RenameSessionArgs { session_id, title },
        ) {
            eprintln!("[craftwave] 重命名失败：{err:?}");
        }
        self.refresh(cx);
    }

    /// 请求删除：打开确认对话框（删除不可逆属合理确认场景，coding-guides
    /// §10 允许）。
    pub(crate) fn request_delete(&mut self, meta: SessionMeta, cx: &mut Context<Self>) {
        self.confirming_delete = Some(meta);
        cx.notify();
    }

    /// 确认删除（对话框 OK）：engine `delete_session`（运行中会拒绝并
    /// 日志——引擎侧 E_SESSION_RUNNING 红线）；删的是当前会话时收摊视图
    /// 回「新会话」空态，随后刷新清单。
    pub(crate) fn confirm_delete(&mut self, cx: &mut Context<Self>) {
        let Some(meta) = self.confirming_delete.take() else {
            return;
        };
        let active = self
            .service
            .upgrade()
            .and_then(|service| service.read(cx).session_id().map(str::to_owned));
        match session::delete_session(
            &self.state,
            DeleteSessionArgs {
                session_id: meta.id.clone(),
            },
        ) {
            Ok(()) => {
                if active.as_deref() == Some(meta.id.as_str())
                    && let Some(service) = self.service.upgrade()
                {
                    service.update(cx, |service, cx| service.reset_to_new(cx));
                }
            }
            Err(err) => eprintln!("[craftwave] 删除会话失败：{err:?}"),
        }
        self.refresh(cx);
    }

    /// 取消删除（对话框 Cancel）。
    pub(crate) fn cancel_delete(&mut self, cx: &mut Context<Self>) {
        if self.confirming_delete.take().is_some() {
            cx.notify();
        }
    }

    /// 派发「新会话」：与菜单 ⌘N 同一条服务调用（一个逻辑命令只建模一次，
    /// coding-guides §4——侧栏大行 defer 到 App 层调服务，不经 action 再
    /// 绕一圈）。
    pub(crate) fn new_session(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.service.upgrade() else {
            return;
        };
        cx.defer(move |cx| {
            service.update(cx, |service, cx| service.reset_to_new(cx));
        });
    }

    /// 某会话所在行号（重命名行内编辑的 splice 重测定位）。
    fn session_row_ix(&self, session_id: &str) -> Option<usize> {
        self.rows.iter().position(|row| match row {
            RowItem::Session(meta) => meta.id == session_id,
            RowItem::Group { .. } => false,
        })
    }

    // ==================================================================
    // 渲染
    // ==================================================================

    /// 当前行渲染（gpui::list 行回调入口）：组头 / 会话行分发。
    fn render_row_item(
        &mut self,
        ix: usize,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        match self.rows.get(ix).cloned() {
            Some(RowItem::Group { id, label, subtitle }) => {
                render_group(&id, &label, subtitle.as_deref(), cx)
            }
            Some(RowItem::Session(meta)) => self.render_session_row(&meta, cx),
            None => div().into_any_element(),
        }
    }

    /// 会话行（行状态槽）：生成中指示（当前会话 busy）、标题（右缘渐隐）、
    /// 相对时间、消息数副行与重命名/删除小钮；活动会话 sidebar_accent
    /// 高亮。ElementId 取会话 id（稳定 ID 红线）。
    fn render_session_row(
        &mut self,
        meta: &SessionMeta,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let theme = cx.theme().colors;
        let zoom = cx.craftwave().zoom;
        let sidebar_bg = theme.sidebar;
        let Some(service) = self.service.upgrade() else {
            return div().into_any_element();
        };
        let active = service.read(cx).session_id() == Some(meta.id.as_str());
        let busy = active && service.read(cx).busy();
        let editing = self.renaming.as_deref() == Some(meta.id.as_str());
        let time_label: SharedString = smart_time(meta.updated_at).into();
        let count_label: SharedString = format!("{} {}", meta.message_count, t("messages")).into();
        let row_id: SharedString = format!("session-{}", meta.id).into();
        let row = div()
            .id(row_id)
            .px(px(theme::SPACE_2))
            .py(px(theme::SPACE_2))
            .rounded(px(theme::rounded_md(zoom)))
            .flex()
            .flex_col()
            .gap(px(theme::SPACE_1))
            .cursor_pointer()
            .when(active, |row| {
                row.bg(theme.sidebar_accent)
                    .text_color(theme.sidebar_accent_foreground)
            })
            .when(!active, |row| {
                row.text_color(theme.sidebar_foreground)
                    .hover(|style| style.bg(theme.sidebar_accent))
            })
            .debug_selector(move || format!("session-row-{}", meta.id));
        if editing {
            // 行内编辑态：共享输入框顶替标题行（Enter/失焦经订阅提交）；
            // 编辑态不挂行点击（避免 blur 提交与切会话在同一次点击里叠加），
            // 小钮也不渲染
            return row
                .child(
                    Input::new(&self.rename_input)
                        .appearance(false)
                        .bordered(false)
                        .small(),
                )
                .into_any_element();
        }
        let row = row.on_click({
            let id = meta.id.clone();
            let entity = cx.entity();
            move |_, _, cx| {
                let id = id.clone();
                entity.update(cx, |sidebar, cx| sidebar.open_session(id, cx));
            }
        });
        let title: SharedString = meta.title.clone().into();
        let meta_for_rename = meta.clone();
        let meta_for_delete = meta.clone();
        row.child(
            // 主行：busy 槽 + 标题（右缘渐隐覆盖条）+ 相对时间
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::SPACE_2))
                .children(busy.then(|| {
                    Spinner::new().small().into_any_element()
                }))
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(theme::font_size(theme::FONT_UI_BASE, zoom)))
                        // 标题不做 truncate：右缘渐变覆盖条承担「渐隐替代
                        // 省略号」（rendering.md C1；gpui 无 mask 能力，
                        // 模块注释有核实结论）
                        .child(title)
                        .child(fade_overlay(sidebar_bg, zoom)),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(theme::font_size(theme::FONT_UI_CAPTION, zoom)))
                        .text_color(theme.muted_foreground)
                        .child(time_label),
                ),
        )
        .child(
            // 副行：消息数 + 行操作小钮（重命名行内编辑；删除先开确认框）
            div()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(theme::SPACE_2))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_hidden()
                        .truncate()
                        .text_size(px(theme::font_size(theme::FONT_UI_CAPTION, zoom)))
                        .text_color(cx.craftwave().colors.dim)
                        .child(count_label),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("rename-{}", meta.id)))
                        .cursor_pointer()
                        .flex_shrink_0()
                        .px(px(theme::SPACE_1))
                        .rounded(px(theme::rounded_sm(zoom)))
                        .text_size(px(theme::font_size(theme::FONT_UI_CAPTION, zoom)))
                        .text_color(theme.muted_foreground)
                        .hover(|style| style.text_color(theme.foreground))
                        .debug_selector({
                            let id = meta_for_rename.id.clone();
                            move || format!("rename-btn-{}", id)
                        })
                        .on_click({
                            let meta = meta_for_rename.clone();
                            let entity = cx.entity();
                            move |_, window, cx| {
                                let (id, title) = (meta.id.clone(), meta.title.clone());
                                entity.update(cx, |sidebar, cx| {
                                    sidebar.begin_rename(id, title, window, cx);
                                });
                            }
                        })
                        .child(t("Rename")),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("delete-{}", meta.id)))
                        .cursor_pointer()
                        .flex_shrink_0()
                        .px(px(theme::SPACE_1))
                        .rounded(px(theme::rounded_sm(zoom)))
                        .text_size(px(theme::font_size(theme::FONT_UI_CAPTION, zoom)))
                        .text_color(theme.muted_foreground)
                        .hover(|style| style.text_color(theme.danger))
                        .debug_selector({
                            let id = meta_for_delete.id.clone();
                            move || format!("delete-btn-{}", id)
                        })
                        .on_click({
                            let meta = meta_for_delete.clone();
                            let entity = cx.entity();
                            move |_, _, cx| {
                                let meta = meta.clone();
                                entity.update(cx, |sidebar, cx| sidebar.request_delete(meta, cx));
                            }
                        })
                        .child(t("Delete")),
                ),
        )
        .into_any_element()
    }

    /// 「新会话」大行（顶部整宽）：与服务 ⌘N 同一条调用（一个逻辑命令只
    /// 建模一次，coding-guides §4）。
    fn render_new_session_row(&mut self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().colors;
        let zoom = cx.craftwave().zoom;
        let entity = cx.entity();
        div()
            .id("new-session")
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(theme::SPACE_3))
            .px(px(theme::SPACE_3))
            .py(px(theme::SPACE_2))
            .rounded(px(theme::rounded_lg(zoom)))
            .cursor_pointer()
            .text_size(px(theme::font_size(theme::FONT_UI_BASE, zoom)))
            .text_color(theme.foreground)
            .hover(|style| style.bg(theme.sidebar_accent))
            .debug_selector(|| "sidebar-new-session".to_owned())
            .on_click(move |_, _, cx| {
                entity.update(cx, |sidebar, cx| sidebar.new_session(cx));
            })
            .child(
                Icon::new(gpui_kit::assets::IconName::CirclePlus)
                    .with_size(px(theme::ICON_LG)),
            )
            .child(t("New Session"))
            .into_any_element()
    }

    /// 删除确认对话框（confirming_delete 在场时挂在渲染树即弹出；确认钮
    /// Danger 变体——不可逆操作的视觉权重）。
    fn render_delete_dialog(&self, cx: &mut Context<Self>) -> Option<gpui_kit::AnyElement> {
        self.confirming_delete.as_ref()?;
        let entity = cx.entity();
        Some(
            AlertDialog::new(cx)
                .confirm()
                .title(t("Delete session?"))
                .description(t(
                    "This permanently removes the conversation history. This cannot be undone.",
                ))
                .ok_text(t("Delete"))
                .ok_variant(ButtonVariant::Danger)
                .cancel_text(t("Cancel"))
                .on_ok({
                    let entity = entity.clone();
                    move |_, _, cx| {
                        entity.update(cx, |sidebar, cx| sidebar.confirm_delete(cx));
                        true
                    }
                })
                .on_cancel(move |_, _, cx| {
                    entity.update(cx, |sidebar, cx| sidebar.cancel_delete(cx));
                    true
                })
                .into_any_element(),
        )
    }
}

impl gpui_kit::Render for SidebarState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 缩放换档随动（spec 2.3 钩子）：字号档变化 → 行高缓存整体重测
        if crate::ui_zoom::changed(&mut self.zoom_seen, cx) {
            self.list.remeasure();
            cx.notify();
        }
        let theme = cx.theme().colors;
        let zoom = cx.craftwave().zoom;
        let sidebar_bg = theme.sidebar;
        let list_state = self.list.clone();
        let entity = cx.entity();
        // 列表滚动渐隐（旧栈 useScrollFade 四态）：上缘有内容 → 顶部渐隐；
        // 下缘有内容 → 底部渐隐（渐变覆盖条；模块注释的核实结论）
        let scrolled = self.list.logical_scroll_top();
        let at_top = scrolled.item_ix == 0 && scrolled.offset_in_item <= px(0.0);
        let at_bottom = self.list.is_scrolled_to_end() != Some(false);
        div()
            .size_full()
            .flex()
            .flex_col()
            .py(px(theme::SPACE_4))
            .gap(px(theme::SPACE_3))
            .text_color(theme.foreground)
            .child(self.render_new_session_row(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(
                        gpui_kit::list(list_state, move |ix, _window, cx| {
                            entity.update(cx, |sidebar, cx| sidebar.render_row_item(ix, cx))
                        })
                        .size_full(),
                    )
                    .when(!at_top, |area| {
                        area.child(fade_bar(sidebar_bg, true, zoom))
                    })
                    .when(!at_bottom, |area| {
                        area.child(fade_bar(sidebar_bg, false, zoom))
                    }),
            )
            .children((self.loaded && self.rows.iter().all(|row| matches!(row, RowItem::Group { .. }))).then(|| {
                // 清单已装载但没有任何可见会话（全空或全是空会话）
                div()
                    .px(px(theme::SPACE_3))
                    .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                    .text_color(cx.craftwave().colors.dim)
                    .child(t("No sessions yet"))
            }))
            .children(self.render_delete_dialog(cx))
    }
}

/// 组头：组标题（最淡前景）+ 项目组的 tilde 路径副行。
fn render_group(
    id: &str,
    label: &str,
    subtitle: Option<&str>,
    cx: &Context<SidebarState>,
) -> gpui_kit::AnyElement {
    let zoom = cx.craftwave().zoom;
    let subtlest = cx.craftwave().colors.foreground_subtlest;
    div()
        .id(SharedString::from(id.to_owned()))
        .mt(px(theme::SPACE_4))
        .px(px(theme::SPACE_2))
        .flex()
        .flex_col()
        .gap(px(theme::SPACE_1))
        .child(
            div()
                .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                .font_weight(FontWeight::MEDIUM)
                .text_color(subtlest)
                .overflow_hidden()
                .truncate()
                .child(SharedString::from(label.to_owned())),
        )
        .children(subtitle.map(|path| {
            div()
                .text_size(px(theme::font_size(theme::FONT_UI_2XS, zoom)))
                .text_color(cx.craftwave().colors.dim)
                .overflow_hidden()
                .truncate()
                .child(path.to_owned())
        }))
        .into_any_element()
}

/// 标题右缘的渐隐覆盖条（侧栏底色 透明→不透明，向右渐实）。
fn fade_overlay(bg: Hsla, zoom: f32) -> gpui_kit::AnyElement {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .right_0()
        .w(px(TITLE_FADE_WIDTH * zoom))
        .bg(linear_gradient(
            90.0,
            linear_color_stop(bg.opacity(0.0), 0.0),
            linear_color_stop(bg, 1.0),
        ))
        .into_any_element()
}

/// 列表滚动渐隐覆盖条：`top` = 顶缘（透明→底色向下渐实），否则底缘反向。
fn fade_bar(bg: Hsla, top: bool, zoom: f32) -> gpui_kit::AnyElement {
    let bar = div()
        .absolute()
        .left_0()
        .right_0()
        .h(px(LIST_FADE_HEIGHT * zoom));
    if top {
        bar.top_0()
            .bg(linear_gradient(
                180.0,
                linear_color_stop(bg.opacity(0.0), 0.0),
                linear_color_stop(bg, 1.0),
            ))
            .into_any_element()
    } else {
        bar.bottom_0()
            .bg(linear_gradient(
                180.0,
                linear_color_stop(bg, 0.0),
                linear_color_stop(bg.opacity(0.0), 1.0),
            ))
            .into_any_element()
    }
}

/// 测试观察面（生产行为由 render 与断言同一字段；这里只补只读口）。
#[cfg(test)]
impl SidebarState {
    pub(crate) fn rename_input(&self) -> &Entity<InputState> {
        &self.rename_input
    }

    pub(crate) fn delete_confirm_target(&self) -> Option<&SessionMeta> {
        self.confirming_delete.as_ref()
    }

    /// 行序列里的会话 id（顺序保持；稳定 ID 测试用）。
    pub(crate) fn row_session_ids(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter_map(|row| match row {
                RowItem::Session(meta) => Some(meta.id.clone()),
                RowItem::Group { .. } => None,
            })
            .collect()
    }
}
