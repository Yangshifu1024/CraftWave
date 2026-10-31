//! CraftWave GPUI 应用壳（gpui-rewrite 阶段 2.B「应用骨架」，spec 2.1）。
//!
//! Wake 式骨架（craftwave-spike 已验证的模式直接沿用）：
//!
//! - `gpui_kit::application().run(...)` 入口，run 闭包首行 `gpui_kit::init`
//!   （组件层全局初始化——缺了它任何走 gpui-component 渲染的路径都会
//!   panic，spike 已踩）；
//! - `TitleBar::window_options()` 窗参：红绿灯悬浮、拖拽归自绘标题带
//!   （spike 踩平的坑：必须用这组窗参，标题带拖拽/双击才归 TitleBar）；
//! - 深浅色跟随系统（appearance 订阅 → `theme::sync_system_appearance`）；
//! - `on_reopen`（macOS Dock 点击重开窗）+ `activate_or_reopen`
//!   （spike 模式：窗活着拉前台，关了重建）；
//! - 键位绑定 + 应用菜单（os_action 风格：关于/设置/编辑标准项/窗口/帮助）；
//! - 空三栏布局：左栏（侧栏占位 260px）/ 中栏（对话流占位）/ 右栏（折叠
//!   为占位条），颜色全部走 ActiveTheme / 自建主题槽位，禁止硬编码色值
//!   （spike 踩过撞色坑）。
//!
//! 阶段边界：托盘/全局热键/退出确认是阶段 4（本 crate 依赖里就没有
//! tray-icon/global-hotkey）；关闭即退出进程是 4.6 之前的默认行为；
//! 引擎（craftwave-engine）的帧流在阶段 2.C 接入，本 crate 目前只用到
//! 它的 util::atomic（原子写盘）。

mod chat;
mod engine;
mod i18n;
mod prefs;
mod sidebar;
// token 表全量照抄 theme.md（其 §11「照抄单」），消费方从阶段 2.C 起逐批
// 接入；bin crate 里暂无消费方的 pub 项也报 dead_code，故模块级放行
#[allow(dead_code)]
mod theme;
// 文本工具四件套（clip_display/one_line/smart_time/tilde_path）：smart_time
// 与 tilde_path 已由 3.1 侧栏消费（会话时间/组头路径），one_line/clip_display
// 的消费方在 chat——greeting 由 B1 空态消费；测试面已钉住，模块级放行兜底
#[allow(dead_code)]
mod text_util;
mod ui_zoom;
mod window_state;

#[cfg(test)]
#[path = "text_util_test.rs"]
mod text_util_test;

#[cfg(test)]
#[path = "ui_test.rs"]
mod ui_test;

use gpui_kit::base::Root;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::TitleBar;
use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
// gpui_kit::* 即 gpui 全量（spike 风格）：styled 方法、AppContext、菜单类型
// （Menu/MenuItem/OsAction 在 platform::* 根导出里）经 glob 全部进作用域
use gpui_kit::*;
use i18n::t;
use theme::ActiveCraftwave as _;

// 三栏骨架的两个占位宽度（阶段 3.1 侧栏 / 3.5 面板族接入前只立骨架，
// 接入时连同真布局一起重订；token 派生口径见编码指南 §3）。
// pub(crate)：中栏会话区按「视口 - 两栏占位」反算阅读区实际宽度（工具簇
// 参数摘要的格数反算，chat/mod.rs Render 每帧执行）。
/// 左栏（会话侧栏）占位宽度。
pub(crate) const SIDEBAR_PLACEHOLDER_WIDTH: f32 = 260.0;
/// 右栏（面板区折叠态）占位宽度。
pub(crate) const PANEL_PLACEHOLDER_WIDTH: f32 = 4.0;

// ---- 图标资产面（2.C-4：chevron 旋转 / 复制 / 下载 / 图片降级卡 / 子代理
// 跳转占位）——2.C-1 起图标面未接，占位字符本批退役 ----

// 组件默认子集（gpui-kit-assets Assets：chevron/copy 等组件自绘用图标）
// 之外的三个自选图标，随本批部件入包（RustEmbed 静态内嵌，运行时零 IO）。
gpui_kit::assets::icon_assets!(CraftwaveIcons, [Download, ImageOff, CornerDownRight]);

/// 组合资产源：组件默认子集优先，自选图标补充；双缺返回 Ok(None)（gpui
/// 对 None 静默跳过绘制——测试无头环境即此路径，不 panic）。
struct AppAssets;

impl gpui_kit::AssetSource for AppAssets {
    fn load(
        &self,
        path: &str,
    ) -> gpui_kit::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(bytes) = CraftwaveIcons.load(path)? {
            return Ok(Some(bytes));
        }
        Ok(gpui_kit::assets::Assets.load(path).ok().flatten())
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<gpui_kit::SharedString>> {
        let mut out = CraftwaveIcons.list(path)?;
        out.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(out)
    }
}

actions!(
    craftwave,
    [
        Quit,
        CloseWindow,
        Hide,
        HideOthers,
        ShowAll,
        Minimize,
        Maximize,
        ToggleFullScreen,
        OpenAbout,
        OpenSettings,
        NewSession,
        ShowMainWindow,
        OpenHelp,
    ]
);

/// 主窗句柄：Dock 重开、activate_or_reopen、窗口级操作据此找主窗。
/// 根视图包 base `Root`（gpui_kit::open_window 的同款包装：挂组件层
/// tooltip/浮层挂载点与 rem 同步插件；Root 本身无泛型参数）。
struct MainWindow(WindowHandle<Root>);

impl Global for MainWindow {}

/// 主窗根视图：自绘标题带 + 三栏骨架。
/// `zoom_seen` 是 ui_zoom 换档检测的上次档位（render 每帧比对，见其注释）；
/// `chat` 是中栏对话流（阶段 2.C-1 接入：消息列表 + Composer）；`sidebar`
/// 是左栏会话侧栏（阶段 3.1；None = 无引擎桥的回声测试装置——侧栏依赖
/// 引擎清单与服务，回声装配以占位文案运行）。
struct MainWindowView {
    zoom_seen: f32,
    chat: Entity<chat::ConversationState>,
    sidebar: Option<Entity<sidebar::SidebarState>>,
}

impl gpui_kit::Render for MainWindowView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // 换档随动：traffic light 挪位（消息列表的重测钩子在 chat 模块自己的
        // render 里，见其 zoom_seen 注释）
        ui_zoom::sync_zoom(&mut self.zoom_seen, window, cx);
        // 颜色全部走主题（gpui-component ActiveTheme 槽位 + 自建槽位），
        // 亮暗切换与缩放换档经窗口刷新重绘取新值，无硬编码色值
        let theme_colors = cx.theme();
        let cw = cx.craftwave();
        let (background, foreground, sidebar, border) = (
            theme_colors.background,
            theme_colors.foreground,
            theme_colors.sidebar,
            theme_colors.border,
        );
        let (main_surface, subtlest) =
            (cw.colors.main_surface, cw.colors.foreground_subtlest);
        let zoom = cw.zoom;

        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(foreground)
            .bg(background)
            // 自绘标题带：拖拽区（TitleBar 组件自带）+ 应用名。traffic light
            // 悬浮其上，位置随缩放档（开窗时与换档后都经 ui_zoom 计算）
            .child(
                TitleBar::new()
                    .text_size(px(theme::font_size(theme::FONT_UI_SM, zoom)))
                    .child(t("CraftWave")),
            )
            // 三栏骨架：左栏 260px 定宽（会话侧栏，3.1）/ 中栏对话流 /
            // 右栏折叠占位条
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    // 左栏：会话侧栏（清单装载/分组/行操作在 sidebar 模块）；
                    // 无引擎桥的回声测试装置保持占位文案
                    .child(
                        div()
                            .w(px(SIDEBAR_PLACEHOLDER_WIDTH))
                            .h_full()
                            .flex_shrink_0()
                            .bg(sidebar)
                            .border_r_1()
                            .border_color(border)
                            .overflow_hidden()
                            .p(px(theme::SPACE_4))
                            .text_size(px(theme::font_size(
                                theme::FONT_UI_CAPTION,
                                zoom,
                            )))
                            .text_color(subtlest)
                            .children(match &self.sidebar {
                                Some(sidebar) => {
                                    let _ = subtlest;
                                    Some(sidebar.clone().into_any_element())
                                }
                                None => Some(div().child(t("Sidebar placeholder")).into_any_element()),
                            }),
                    )
                    // 中栏：对话流（阶段 2.C-1 消息流底座 + Composer；
                    // 历史由 chat::install 装假数据，真会话装载随 3.1）
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .bg(main_surface)
                            .overflow_hidden()
                            .child(self.chat.clone()),
                    )
                    // 右栏：折叠为占位条（阶段 3.5 面板族接入前只立骨架）
                    .child(
                        div()
                            .w(px(PANEL_PLACEHOLDER_WIDTH))
                            .h_full()
                            .flex_shrink_0()
                            .bg(sidebar)
                            .border_l_1()
                            .border_color(border),
                    ),
            )
    }
}

/// 开主窗：恢复几何 → 按当前缩放档算 traffic light 位置 → 构造视图
/// （几何跟踪、appearance 订阅、关窗即退出都挂在这里）。启动、Dock 重开、
/// activate_or_reopen 共用。
fn open_main_window(cx: &mut App) -> WindowHandle<Root> {
    let bounds = window_state::restore(cx);
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(bounds),
                window_min_size: Some(size(
                    px(window_state::MAIN_MIN_SIZE.0),
                    px(window_state::MAIN_MIN_SIZE.1),
                )),
                // TitleBar 组件的配套窗参 + traffic light 位置随缩放档
                titlebar: Some(gpui_kit::TitlebarOptions {
                    traffic_light_position: Some(ui_zoom::traffic_lights_now(cx)),
                    ..TitleBar::title_bar_options()
                }),
                ..TitleBar::window_options()
            },
            |window, cx| {
                // 跟随系统深浅色切换（macOS 外观变化 → 模式同步 → 重套 token）
                window
                    .observe_window_appearance(|window, cx| {
                        theme::sync_system_appearance(Some(window), cx);
                    })
                    .detach();
                theme::sync_system_appearance(Some(window), cx);
                // 关窗收尾：先取消进行中 run（优雅收尾——退出拦截本身
                // 阶段 4.6 接管），再强写几何、退进程
                window.on_window_should_close(cx, |_window, cx| {
                    // clone 结束 try_global 对 cx 的借用，才能把 cx 以
                    // &mut 交给 service.update（activate_or_reopen 同款）
                    if let Some(service) = cx.try_global::<chat::EngineService>().map(|s| s.0.clone()) {
                        service.update(cx, |service, cx| service.shutdown(cx));
                    }
                    window_state::flush(cx);
                    cx.quit();
                    true
                });
                // 中栏对话流装配（composer + 输入事件订阅 + 真通道服务，
                // 都需要 window —— 必须在窗口构造闭包里做；引擎桥已在
                // main 装配，服务实体在此处创建并挂 Global）
                let engine = crate::engine::bridge(cx);
                let chat_entity = chat::install(window, cx, engine.as_ref());
                // 左栏侧栏装配（须在 chat::install 后——依赖 EngineService
                // Global；引擎桥缺失（回声测试装置）时不装，左栏保持占位文案）
                let sidebar_entity = engine.as_ref().map(|bridge| {
                    sidebar::install(window, cx, bridge)
                });
                // 根视图；几何记忆（观察 bounds + 防抖落盘）挂在它的构造
                // Context 上，再包 base Root（浮层挂载点 + rem 同步插件）
                let view = cx.new(|cx| {
                    window_state::track(window, cx);
                    MainWindowView {
                        // 从 0 起步：首帧 changed 必真，灯位按当前档对齐一次
                        zoom_seen: 0.0,
                        chat: chat_entity,
                        sidebar: sidebar_entity,
                    }
                });
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("开主窗失败");
    // WindowHandle 是 Copy（句柄即 id），set_global 存副本即可
    cx.set_global(MainWindow(handle));
    handle
}

/// 主窗还在就拉到前台；窗已关（update Err）就重建。spike 模式：
/// 已关窗时 handle.update 失败是唯一的「窗没了」信号。
/// on_reopen / ShowMainWindow / 占位菜单项共用。
fn activate_or_reopen(cx: &mut App) {
    // clone 结束 try_global 对 cx 的借用，才能把 cx 以 &mut 交给 handle.update
    let Some(handle) = cx.try_global::<MainWindow>().map(|m| m.0) else {
        let h = open_main_window(cx);
        let _ = h.update(cx, |_, window, _| window.activate_window());
        return;
    };
    if handle
        .update(cx, |_, window, _| window.activate_window())
        .is_err()
    {
        let h = open_main_window(cx);
        let _ = h.update(cx, |_, window, _| window.activate_window());
    }
}

/// 窗口级操作（关/最小化/最大化/全屏）从全局监听器发起时，目标窗口正处在
/// 派发这个 action 的 update 期间（键位与菜单两条路都如此），被借出的窗口
/// 不能再次 update——推迟到本轮 update 结束再动（Wake with_active_window 模式）。
fn with_active_window(cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
    let Some(w) = cx.active_window() else {
        return;
    };
    cx.defer(move |cx| {
        w.update(cx, |_, window, cx| f(window, cx)).ok();
    });
}

/// 键位绑定。macOS 标准的应用/窗口键位 gpui 不会自动处理，没有绑定就是死键。
///
/// shortcut: 前缀用 `cmd-` 而非跨平台别名 `secondary-`——gpui-pre 0.3.10 的
/// `KeyBinding::new` 走无 OS 参数的 `Keystroke::parse`，`secondary-` 在该路径
/// 直接 Err（构造即 panic，启动冒烟已踩：`InvalidKeystrokeError`）；带 OS 的
/// `parse_with_os` 只在平台适配层用。⌘/Ctrl 的跨平台分流在阶段 5 三平台 CI
/// 接入时按 target_os 构造键位表。
fn key_bindings() -> Vec<KeyBinding> {
    let mut keys = vec![
        // 设置 / 新会话（本阶段占位 action，阶段 3 接面板）
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-n", NewSession, None),
        // 权限档快切（3.4：Shift+Tab 循环，旧栈 permission-tier 同键位；
        // 派发沿 focus path 冒泡，ConversationState 的 on_action 处理）
        KeyBinding::new("shift-tab", chat::CyclePermissionTier, None),
        // 应用与窗口标准键位
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-m", Minimize, None),
    ];
    // 界面缩放四档（⌘1–⌘4 直选，键位表在 ui_zoom 与其 action 同源）
    keys.extend(ui_zoom::key_bindings());
    if cfg!(target_os = "macos") {
        keys.extend([
            KeyBinding::new("cmd-h", Hide, None),
            KeyBinding::new("cmd-alt-h", HideOthers, None),
            KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None),
        ]);
    }
    keys
}

/// 菜单栏。gpui 只按这里给的生成，不补任何标准项——Edit/Window/Hide 一族
/// 在 macOS 上必须自己给，否则对应快捷键全是死键。
fn app_menus() -> Vec<Menu> {
    let mac = cfg!(target_os = "macos");
    let app_menu: Vec<MenuItem> = [
        Some(MenuItem::action(t("About CraftWave"), OpenAbout)),
        Some(MenuItem::separator()),
        Some(MenuItem::action(t("Settings…"), OpenSettings)),
        Some(MenuItem::separator()),
        mac.then(|| MenuItem::action(t("Hide CraftWave"), Hide)),
        mac.then(|| MenuItem::action(t("Hide Others"), HideOthers)),
        mac.then(|| MenuItem::action(t("Show All"), ShowAll)),
        mac.then(MenuItem::separator),
        Some(MenuItem::action(t("Quit CraftWave"), Quit)),
    ]
    .into_iter()
    .flatten()
    .collect();
    // File：新会话（⌘N 占位）+ 关窗
    let file = vec![
        MenuItem::action(t("New Session"), NewSession),
        MenuItem::separator(),
        MenuItem::action(t("Close Window"), CloseWindow),
    ];
    // os_action：菜单项挂原生 cut:/copy:/paste:/selectAll: 选择器。gpui 窗口
    // 在前时由 gpui 接住、派发成右侧的 action（输入框消费）；系统面板在前时
    // 走原生响应链——没有这组菜单项，面板里的 ⌘C/⌘V 无人应答
    let edit = mac.then(|| Menu {
        name: t("Edit").into(),
        items: vec![
            MenuItem::os_action(t("Undo"), Undo, OsAction::Undo),
            MenuItem::os_action(t("Redo"), Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action(t("Cut"), Cut, OsAction::Cut),
            MenuItem::os_action(t("Copy"), Copy, OsAction::Copy),
            MenuItem::os_action(t("Paste"), Paste, OsAction::Paste),
            MenuItem::os_action(t("Select All"), SelectAll, OsAction::SelectAll),
        ],
        disabled: false,
    });
    // 界面缩放子菜单：⌘1–⌘4 四档直选（action 全局监听，见 ui_zoom::register）
    // 名字必须**逐字**是 "Window"，不能翻译：gpui 的 macOS 后端按菜单名
    // `== "Window"` 才调 setWindowsMenu_，译成「窗口」后 AppKit 不再把它当
    // 系统窗口菜单，自动维护的已打开窗口列表随之消失（Wake 同款结论）。
    let window = mac.then(|| Menu {
        name: "Window".into(),
        items: vec![
            MenuItem::action(t("Minimize"), Minimize),
            MenuItem::action(t("Zoom"), Maximize),
            MenuItem::separator(),
            MenuItem::action(t("Main Window"), ShowMainWindow),
        ],
        disabled: false,
    });
    [
        Some(Menu {
            name: "CraftWave".into(),
            items: app_menu,
            disabled: false,
        }),
        Some(Menu {
            name: t("File").into(),
            items: file,
            disabled: false,
        }),
        edit,
        Some(Menu {
            name: t("View").into(),
            items: vec![
                MenuItem::action(ui_zoom::level_label(0), ui_zoom::ZoomSmall),
                MenuItem::action(ui_zoom::level_label(1), ui_zoom::ZoomStandard),
                MenuItem::action(ui_zoom::level_label(2), ui_zoom::ZoomLarge),
                MenuItem::action(ui_zoom::level_label(3), ui_zoom::ZoomXLarge),
            ],
            disabled: false,
        }),
        window,
        Some(Menu {
            name: t("Help").into(),
            items: vec![MenuItem::action(t("CraftWave Help"), OpenHelp)],
            disabled: false,
        }),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn main() {
    // 图标资产源（Icon/svg 的 path 解析走它；见 AppAssets 注释）
    let app = gpui_kit::application().with_assets(AppAssets);
    // macOS：用户关掉最后一个窗后进程仍留 Dock，点 Dock 图标走
    // applicationShouldHandleReopen → on_reopen（系统认为没有可见窗口时触发）。
    // 与 Window → Main Window 同一条路
    app.on_reopen(activate_or_reopen);
    app.run(move |cx: &mut App| {
        // 组件层全局状态初始化（gpui-base/gpui-component 的 Global、主题、
        // reduce-motion 系统读数、Root 插件注册）——必须最先
        gpui_kit::init(cx);
        // 主题 Global 挂载 → 读缩放偏好（赶在套主题与开窗之前，字号与
        // traffic light 位置都从 zoom 算出）→ 套 CraftWave 色
        theme::CraftwaveTheme::mount(cx);
        ui_zoom::init(cx);
        theme::apply(cx);
        ui_zoom::register(cx);

        // 引擎桥（阶段 2.C-2b）：常驻 tokio 运行时 + AppState 装配
        // （~/.craftwave）。会话服务实体随主窗构造（chat::install）创建，
        // 这里先把引擎侧就位。
        engine::install(cx);

        // 应用退出统一收口：退出前强写主窗几何（节流写盘可能还没触发；
        // Dock 退出、注销关机都走这里，不只 ⌘Q）
        cx.on_app_quit(|cx| {
            window_state::flush(cx);
            async {}
        })
        .detach();

        // —— action 处理器 ——
        cx.on_action(|_: &Quit, cx| {
            window_state::flush(cx);
            cx.quit();
        });
        cx.on_action(|_: &CloseWindow, cx| {
            with_active_window(cx, |window, cx| {
                window_state::flush(cx);
                window.remove_window()
            });
        });
        cx.on_action(|_: &Minimize, cx| {
            with_active_window(cx, |w, _| w.minimize_window());
        });
        cx.on_action(|_: &Maximize, cx| {
            with_active_window(cx, |w, _| w.zoom_window());
        });
        cx.on_action(|_: &ToggleFullScreen, cx| {
            with_active_window(cx, |w, _| w.toggle_fullscreen());
        });
        cx.on_action(|_: &Hide, cx| cx.hide());
        cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
        cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
        cx.on_action(|_: &ShowMainWindow, cx| activate_or_reopen(cx));
        // —— 占位 action（阶段 3 接面板/会话面；先激活主窗 + 日志留痕） ——
        cx.on_action(|_: &OpenSettings, cx| {
            eprintln!("[craftwave] 占位：设置页（阶段 3.7 接入）");
            activate_or_reopen(cx);
        });
        cx.on_action(|_: &OpenAbout, cx| {
            eprintln!("[craftwave] 占位：关于页（阶段 3 接入）");
            activate_or_reopen(cx);
        });
        cx.on_action(|_: &OpenHelp, cx| {
            eprintln!("[craftwave] 占位：帮助（阶段 3 接入）");
            activate_or_reopen(cx);
        });
        // 新会话（⌘N / 菜单 / 侧栏大行同一逻辑命令——侧栏行走服务的同一条
        // 调用，coding-guides §4）：服务收摊回「新会话」空态（首条消息发送
        // 时才建会话索引条目——send_message 既有语义）
        cx.on_action(|_: &NewSession, cx| {
            if let Some(service) = cx.try_global::<chat::EngineService>().map(|s| s.0.clone()) {
                service.update(cx, |service, cx| service.reset_to_new(cx));
            }
            activate_or_reopen(cx);
        });

        cx.bind_keys(key_bindings());
        cx.set_menus(app_menus());

        open_main_window(cx);
        cx.activate(true);
    });
}
