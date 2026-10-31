//! ⌘K 命令面板与全局搜索（gpui-rewrite 阶段 3.2，spec 表 3.2「SearchDelegate
//! 模式；FTS + 跳转高亮」）。
//!
//! ## 组件选型（先标准组件后自造，coding-guides §0）
//!
//! gpui-kit 0.8.1 自带命令面板组件 [`gpui_kit::component::command`]：
//! `Command`（RenderOnce 模型装载）+ `CommandState`（查询/焦点/选中/滚动的
//! 交互态）+ 分组渲染 + Action 键位提示 + 方向键/Enter/Escape 完整键盘导航
//! （上下键循环跳过禁用项、Escape 先清查询再冒泡给宿主关闭）。**直接采用
//! 组件**，本模块只补宿主职责：开合与焦点往返、会话搜索数据面、Home/End
//! 导航（组件未绑这两个键，宿主容器接 SelectFirst/SelectLast）与浮层挂载。
//!
//! ## 搜索面（engine API 源码核实结论）
//!
//! engine 的 `cache/index.db` 只有 sessions 缓存表 + stats 缓存表，
//! **没有 FTS5 内容索引**；API 面（api/mod.rs 映射表）无内容搜索命令
//! （`search_test` 是网络搜索通道连通测试，与内容搜索无关）。因此本切片
//! 的全局搜索 = **会话标题搜索**（`list_sessions` 清单 + 大小写不敏感
//! 子串匹配，FR-01-15 的「索引不可用时降级为标题搜索」形态）；会话内容
//! 搜索与「内容条目跳转定位到命中消息」暂缺，随 engine FTS 索引切片补。
//!
//! ## 查询防抖 + 代次对账（2.C-2b 补全对账同款手法）
//!
//! 查询变化 → 重置代次 + 起后台任务（120ms 防抖定时 → 后台线程池过滤
//! 清单 → 回写）；任务句柄存 owner，新查询 drop 旧任务（定时即取消），
//! 回写时比对代次，期间又发起过查询的旧结果直接丢弃。
//!
//! ## 条目与分组
//!
//! - **命令组**（固定三条，直接挂既有 action——一个逻辑命令只建模一次）：
//!   新会话（⌘N 同款）/ 会话内查找（⌘F 同款）/ 设置（⌘, 同款）；
//! - **会话组**：空查询给最近会话（清单 updatedAt 倒序前 N 条），有查询
//!   给标题命中（`messageCount = 0` 的会话不进结果——侧栏 listed 同纪律）；
//!   确认经 `SessionService::open_session` 定位会话（与侧栏行同一条调用）。

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::actions::{Cancel, SelectFirst, SelectLast};
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::{ActiveTheme as _, Icon};
use gpui_kit::prelude::*;
use gpui_kit::component::IndexPath;
use gpui_kit::{
    App, Context, Entity, FocusHandle, SharedString, Task, WeakEntity, Window, div, px, rems,
};

use craftwave_engine::api::session::{self, ListSessionsArgs};
use craftwave_engine::app_state::AppState;
use craftwave_engine::storage::SessionMeta;

use crate::i18n::t;
use crate::theme::{self, ActiveCraftwave as _};

use crate::chat::{EngineService, SessionService};

#[cfg(test)]
#[path = "palette_test.rs"]
mod palette_test;

// 面板开关 action（⌘K；菜单与键位同一逻辑命令，coding-guides §4）。
gpui_kit::actions!(craftwave, [OpenCommandPalette]);

/// 查询防抖时长（连打不逐字触发过滤；一个字母的过滤在后台是亚毫秒级，
/// 防抖只削掉中间态的任务数）。
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);
/// 会话结果条数上限（单屏容量；Wake PALETTE_SESSION_LIMIT 同语义）。
const SESSION_HIT_LIMIT: usize = 8;
/// 空查询态的最近会话条数。
const RECENT_SESSIONS: usize = 8;
/// 命令组条数（固定三条；Home/End 的 IndexPath 计算基准）。
const COMMAND_ITEMS: usize = 3;
/// 面板浮层宽度（40rem，标准档 14px 基准下 560px）。
const PALETTE_WIDTH_REMS: f32 = 40.0;

/// 命令面板状态：开合 + 会话清单/命中 + 查询任务对账 + 焦点往返。
pub struct PaletteState {
    /// engine 应用状态（清单读面；install 装配注入）。
    app: Arc<AppState>,
    /// 会话服务（确认会话条目时定位；弱引用——服务在 Global 常驻）。
    service: WeakEntity<SessionService>,
    /// gpui-kit 命令面板组件态（查询输入框/选中/滚动都在它身上）。
    command: Entity<CommandState>,
    /// 是否打开（False 时不渲染浮层）。
    open: bool,
    /// 会话清单（engine updatedAt 倒序；open 时后台刷新）。
    sessions: Vec<SessionMeta>,
    /// 清单刷新代次（过期回写丢弃——sidebar 同款）。
    list_revision: usize,
    /// 搜索代次（防抖任务回写的对账键）。
    search_generation: usize,
    /// 防抖搜索任务句柄（存 owner：新查询 drop 旧任务即取消定时）。
    search_task: Option<Task<()>>,
    /// 当前查询的标题命中（后台过滤回写；空查询时不用）。
    hits: Vec<SessionMeta>,
    /// 本帧会话组展示的条目（渲染时记录，确认时按 IndexPath 取回——
    /// 命中与最近会话共用一个组槽位）。
    visible_sessions: Vec<SessionMeta>,
    /// 打开前的焦点（关闭时恢复——overlay 开时转移焦点、关时恢复）。
    return_focus: Option<FocusHandle>,
}

/// 装配（开窗闭包内调用，与 chat::install / sidebar::install 并列）：
/// 建 CommandState（内部查询输入框要 `&mut Window`）+ 面板实体。
pub fn install(
    window: &mut Window,
    cx: &mut App,
    engine: &crate::engine::EngineBridge,
) -> Entity<PaletteState> {
    let command = cx.new(|cx| CommandState::new(window, cx));
    let service = cx.global::<EngineService>().0.clone();
    cx.new(|_| PaletteState {
        app: engine.state.clone(),
        service: service.downgrade(),
        command,
        open: false,
        sessions: Vec::new(),
        list_revision: 0,
        search_generation: 0,
        search_task: None,
        hits: Vec::new(),
        visible_sessions: Vec::new(),
        return_focus: None,
    })
}

impl PaletteState {
    /// ⌘K 开合：开 → 刷新清单 + 清查询 + 聚焦查询框（记住来处焦点）；
    /// 再按一次 → 关。
    pub(crate) fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(window, cx);
            return;
        }
        self.open = true;
        self.return_focus = window.focused(cx);
        self.refresh_sessions(cx);
        // 清查询走 set_query（组件把它当输入处理：重过滤 + on_query 回调
        // 归零命中面）
        self.command
            .update(cx, |command, cx| command.set_query("", window, cx));
        self.command.update(cx, |command, cx| command.focus(window, cx));
        cx.notify();
    }

    /// 关闭浮层：焦点交还来处（composer 输入框/侧栏行——开前的焦点）。
    pub(crate) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open && self.return_focus.is_none() {
            return;
        }
        self.open = false;
        if let Some(focus) = self.return_focus.take()
            && !focus.is_focused(window)
        {
            window.focus(&focus, cx);
        }
        cx.notify();
    }

    /// 刷新会话清单（open 时）：engine 同步读命令跑后台线程池，回写按
    /// 刷新代次对账；清单晚于查询到达时按当前查询词现滤一遍。
    fn refresh_sessions(&mut self, cx: &mut Context<Self>) {
        let app = self.app.clone();
        self.list_revision += 1;
        let revision = self.list_revision;
        cx.spawn(async move |this, cx| {
            let sessions = cx
                .background_spawn(async move {
                    session::list_sessions(&app, ListSessionsArgs::default()).ok()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.list_revision != revision {
                    return; // 过期回写：期间又发起过刷新
                }
                if let Some(sessions) = sessions {
                    this.sessions = sessions;
                }
                // 清单到了，按当前查询词现滤（清单装载晚于首键的场景）
                let query = this.command.read(cx).query(cx).trim().to_lowercase();
                this.search_task = None;
                this.hits = if query.is_empty() {
                    Vec::new()
                } else {
                    filter_sessions(&this.sessions, &query)
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// 查询变化（组件 on_query 回调）：重置代次 + 防抖后台过滤。
    /// 空查询清空命中面（回到最近会话态）。
    fn schedule_search(&mut self, query: &str, cx: &mut Context<Self>) {
        self.search_generation += 1;
        let generation = self.search_generation;
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            self.search_task = None;
            self.hits.clear();
            cx.notify();
            return;
        }
        let sessions = self.sessions.clone();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            // 防抖：新查询 drop 本任务（Task drop 即取消定时），只有
            // 最后一键的定时能跑完
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            let hits = cx
                .background_spawn(async move { filter_sessions(&sessions, &query) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return; // 过期回写：期间又发起过查询
                }
                this.hits = hits;
                cx.notify();
            });
        }));
    }

    /// 确认会话条目：走服务 `open_session`（与侧栏行同一条调用——
    /// 一个逻辑命令只建模一次），随后关面板。defer 推迟到本轮 update 外
    /// （服务链路会回写会话实体，嵌套 update 禁止）。
    fn open_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        let Some(service) = self.service.upgrade() else {
            return;
        };
        cx.defer(move |cx| {
            service.update(cx, |service, cx| service.open_session(session_id, cx));
        });
    }

    /// Home/End 的落点（组件未内建这两个键，宿主容器接）：
    /// Home = 命令组第一条；End = 会话组最后一条（无会话时命令组末条）。
    /// 有查询时组件的本地过滤是关的（外部搜索源供条目），路径即当前
    /// 展示序——命中面在防抖窗口内可能短暂超前/滞后一拍，选中落空时
    /// `set_selected_index` 清除高亮，组件语义兜底。
    fn select_edge(&mut self, last: bool, window: &mut Window, cx: &mut Context<Self>) {
        let path = if last && !self.visible_sessions.is_empty() {
            IndexPath::new(self.visible_sessions.len() - 1).section(1)
        } else if last {
            IndexPath::new(COMMAND_ITEMS - 1).section(0)
        } else {
            IndexPath::new(0).section(0)
        };
        self.command.update(cx, |command, cx| {
            command.set_selected_index(Some(path), window, cx);
        });
    }
}

/// 标题过滤（大小写不敏感子串；`messageCount = 0` 的会话不进结果——
/// 新建未发送的会话不可见，侧栏 listed 同纪律）。倒序入参保序返回。
fn filter_sessions(sessions: &[SessionMeta], query: &str) -> Vec<SessionMeta> {
    sessions
        .iter()
        .filter(|meta| meta.message_count > 0 && meta.title.to_lowercase().contains(query))
        .take(SESSION_HIT_LIMIT)
        .cloned()
        .collect()
}

impl gpui_kit::Render for PaletteState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let theme = cx.theme().colors;
        let cw = cx.craftwave();
        let (zoom, scrim) = (cw.zoom, cw.colors.scrim);
        let entity = cx.entity();
        // 本帧会话组内容：空查询给最近会话，有查询给标题命中
        let query = self.command.read(cx).query(cx).trim().to_string();
        let sessions = if query.is_empty() {
            self.sessions
                .iter()
                .filter(|meta| meta.message_count > 0)
                .take(RECENT_SESSIONS)
                .cloned()
                .collect()
        } else {
            self.hits.clone()
        };
        self.visible_sessions = sessions.clone();
        let sessions_label = if query.is_empty() {
            t("Recent Sessions")
        } else {
            t("Sessions")
        };
        let session_items = sessions.iter().map(|meta| {
            CommandItem::new()
                .label(meta.title.clone())
                .icon(Icon::new(IconName::MessageSquare))
        });
        // 命令组：直接挂既有 action（⌘N/⌘F/⌘, 的同一条命令——组件确认时
        // dispatch action，应用级处理器接住，无第二份逻辑）
        let commands = CommandGroup::new()
            .label(t("Commands"))
            .item(
                CommandItem::new()
                    .label(t("New Session"))
                    .icon(Icon::new(IconName::SquarePen))
                    .action(Box::new(crate::NewSession)),
            )
            .item(
                CommandItem::new()
                    .label(t("Find in Session…"))
                    .icon(Icon::new(IconName::Search))
                    .action(Box::new(crate::chat::FindInSession)),
            )
            .item(
                CommandItem::new()
                    .label(t("Settings…"))
                    .icon(Icon::new(IconName::Settings))
                    .action(Box::new(crate::OpenSettings)),
            );
        let sessions_group =
            CommandGroup::new().label(sessions_label).items(session_items);
        // 空结果面（查询无命中且命令恒在场——只可能是会话组空，给一句
        // 明示而不是装作没有查询）
        let empty_label: SharedString = t("No matching sessions").into();
        let overlay = div()
            .absolute()
            .inset_0()
            .occlude()
            .bg(scrim)
            .flex()
            .flex_col()
            .items_center()
            .id("command-palette")
            .debug_selector(|| "command-palette".to_owned())
            // 背景点击收场（点卡片内不冒泡——下面 stop_propagation）
            .on_click(cx.listener(|this: &mut Self, _, window, cx| {
                this.close(window, cx);
            }))
            .child(
                div()
                    .id("command-palette-card")
                    .debug_selector(|| "command-palette-card".to_owned())
                    .w(rems(PALETTE_WIDTH_REMS))
                    .mt(rems(6.0))
                    .rounded(px(theme::rounded_xl(zoom)))
                    .overflow_hidden()
                    .shadow_lg()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .on_action(cx.listener(|this: &mut Self, _: &Cancel, window, cx| {
                        // Escape：组件先清非空查询；空查询 Cancel 冒泡到这
                        // 里收场（组件文档的宿主分工）
                        this.close(window, cx);
                    }))
                    .on_action(cx.listener(
                        |this: &mut Self, _: &SelectFirst, window, cx| {
                            this.select_edge(false, window, cx);
                        },
                    ))
                    .on_action(cx.listener(|this: &mut Self, _: &SelectLast, window, cx| {
                        this.select_edge(true, window, cx);
                    }))
                    .child(
                        Command::new(&self.command)
                            .bordered(false)
                            .placeholder(t("Search sessions or run a command…"))
                            .filterable(false)
                            .group(commands)
                            .group(sessions_group)
                            .empty(move |_, _, _| {
                                div()
                                    .p_2()
                                    .text_size(px(theme::font_size(
                                        theme::FONT_UI_SM,
                                        zoom,
                                    )))
                                    .text_color(theme.muted_foreground)
                                    .child(empty_label.clone())
                            })
                            .on_query({
                                let entity = entity.downgrade();
                                move |query, _, cx| {
                                    entity
                                        .update(cx, |state, cx| state.schedule_search(query, cx))
                                        .ok();
                                }
                            })
                            .on_confirm({
                                let entity = entity.downgrade();
                                move |index, window, cx| {
                                    let Ok(target) = entity.update(cx, |state, _cx| {
                                        // 会话组（section 1）按本帧展示序取回目标；
                                        // 命令条目由组件 dispatch action（应用级
                                        // 处理器接住，一个逻辑命令只建模一次）
                                        (index.section == 1)
                                            .then(|| {
                                                state.visible_sessions.get(index.row).cloned()
                                            })
                                            .flatten()
                                    }) else {
                                        return;
                                    };
                                    // 任何确认都收场（命令与会话一致）
                                    let _ = entity.update(cx, |state, cx| {
                                        if let Some(meta) = target {
                                            state.open_session(meta.id, cx);
                                        }
                                        state.close(window, cx);
                                    });
                                }
                            }),
                    ),
            );
        overlay.into_any_element()
    }
}
