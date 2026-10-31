//! 宿主外壳：托盘、关闭到托盘、
//! 退出拦截、单实例唤起。
//!
//! 后台运行语义：主窗 close 请求一律拦截为隐藏（关闭到托盘默认开，可在
//! 设置切换）；真正的退出只经托盘菜单「退出」，且存在活跃 run
//! 时先经 `app:exit_requested` 事件征得前端确认，前端调
//! `resolve_exit_request(ack)` 放行或取消（退出拦截）；前端超时
//! 未回执则到点兜底放行（[`EXIT_CONFIRM_TIMEOUT`]）——用户退出
//! 意图优先，活跃 run 数据由 append-only 崩溃恢复语义兜底。

use serde_json::json;
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

use crate::agent::RunManager;
use crate::host::AppState;
use crate::util::events::keys;

/// 主窗口标签（tauri.conf.json `app.windows[0].label`）。
pub const MAIN_WINDOW: &str = "main";

/// 退出确认超时：`app:exit_requested` 发出后前端在此时限
/// 内未回执即兜底放行退出。用户退出意图优先——退出流程不能因前端无响应
/// （webview 卡死/已崩溃）而永远退不出去；活跃 run 的数据安全由 append-only
/// 崩溃恢复语义兜底（重启后标「已中断」的既有行为），而非靠拦截退出。
pub const EXIT_CONFIRM_TIMEOUT: Duration = Duration::from_secs(10);

/// 退出确认状态机（挂起标志 + 超时兜底判定）。
/// 纯逻辑：计时入参显式传入（request/timed_out 各自带 now），可确定性单测
/// （见 shell_test.rs）；真实时间由 request_exit / resolve_exit_request 注入。
pub struct ExitConfirm {
    /// Some(发出 app:exit_requested 的时刻) = 等待前端回执中。
    pending_since: Option<Instant>,
    timeout: Duration,
}

impl ExitConfirm {
    pub fn new(timeout: Duration) -> Self {
        Self {
            pending_since: None,
            timeout,
        }
    }

    /// 记录一次退出请求（重复触发刷新计时起点，与「下次再触发」语义一致）。
    pub fn request(&mut self, now: Instant) {
        self.pending_since = Some(now);
    }

    /// 前端回执：解除挂起，返回 accepted（是否放行退出）。
    pub fn resolve(&mut self, accepted: bool) -> bool {
        self.pending_since = None;
        accepted
    }

    /// 超时兜底判定：挂起中且已超时 → 放行（同时解除挂起，防兜底重复放行）。
    pub fn timed_out(&mut self, now: Instant) -> bool {
        match self.pending_since {
            Some(since) if now.duration_since(since) >= self.timeout => {
                self.pending_since = None;
                true
            }
            _ => false,
        }
    }

    /// 是否挂起中（等待前端回执）。
    pub fn is_pending(&self) -> bool {
        self.pending_since.is_some()
    }
}

/// 唤起并聚焦主窗（单实例二次启动、托盘点击、通知点击共用）。
pub fn activate_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 全局热键的 toggle 主窗：可见即隐藏、隐藏即唤起
/// （show + unminimize + set_focus，复用 [`activate_main_window`]）。
/// 活跃 run 不受隐藏影响（run 在 Rust 侧继续推进，后台运行语义）。
pub fn toggle_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            activate_main_window(app);
        }
    }
}

/// 装配托盘（图标 + 「显示主窗口 / 退出」菜单）。
///
/// 三平台行为对齐：左键单击唤起主窗、右键弹菜单——
/// `show_menu_on_left_click(false)` 关闭「左键弹菜单」，点击事件经
/// `TrayIconEvent::Click`（Left + Up）走 [`activate_main_window`]。
/// 平台注记（依据 tauri 2.12.5 / tray-icon 0.25.1 源码核实）：
/// - macOS：`show_menu_on_left_click(false)` 由 tray-icon 落实为「菜单只在
///   右键时挂到 NSStatusItem」（`performClick` 按需弹出），左键只发 Click 事件；
/// - Windows：同上，菜单走 TrackPopupMenu（右键）；
/// - Linux：AppIndicator 不支持「点击」事件与左右键区分（tauri 文档明示
///   `show_menu_on_left_click` 在 Linux 不生效），点击只会弹菜单——这是
///   平台能力边界，唤起语义在 Linux 降级为菜单项「显示主窗口」。
pub fn install_tray(app: &AppHandle) -> tauri::Result<()> {
    let lang = current_ui_language(app);
    let show_item = MenuItem::with_id(
        app,
        "show",
        crate::host::strings::t(crate::host::strings::Str::TrayShow, lang),
        true,
        None::<&str>,
    )?;
    let quit_item = MenuItem::with_id(
        app,
        "quit",
        crate::host::strings::t(crate::host::strings::Str::TrayQuit, lang),
        true,
        None::<&str>,
    )?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;
    TrayIconBuilder::with_id("craftwave-tray")
        .icon(app.default_window_icon().expect("应用图标缺失").clone())
        .menu(&menu)
        // 左键不弹菜单（右键才弹）：三平台一致的「左键唤起」前提
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => activate_main_window(app),
            "quit" => request_exit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // 只认左键抬起（Click Left Up）：macOS/Windows 都会先发 Down 再发
            // Up，取 Up 作「单击完成」口径；右/中键不处理（右键由菜单接管）。
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                activate_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// 当前界面语言：读配置缓存；读不出按 Language 缺省（中文）。
/// 托盘装配与 save_config 钩子（[`apply_tray_language`]）共用。
fn current_ui_language(app: &AppHandle) -> crate::providers::types::Language {
    app.try_state::<crate::host::AppState>()
        .and_then(|state| state.config.load().ok().map(|c| c.ui.language))
        .unwrap_or_default()
}

/// 托盘菜单按当前界面语言重建（save_config 钩子调用）：语言切换
/// 后右键菜单两项即时换语言。托盘未装配（如无图标环境）时静默跳过。
pub fn apply_tray_language(app: &AppHandle) {
    let Some(tray) = app.tray_by_id("craftwave-tray") else {
        return;
    };
    let lang = current_ui_language(app);
    let show = MenuItem::with_id(
        app,
        "show",
        crate::host::strings::t(crate::host::strings::Str::TrayShow, lang),
        true,
        None::<&str>,
    );
    let quit = MenuItem::with_id(
        app,
        "quit",
        crate::host::strings::t(crate::host::strings::Str::TrayQuit, lang),
        true,
        None::<&str>,
    );
    let (Ok(show), Ok(quit)) = (show, quit) else {
        return;
    };
    if let Ok(menu) = Menu::with_items(app, &[&show, &quit]) {
        let _ = tray.set_menu(Some(menu));
    }
}

/// 退出请求：无活跃 run 直接退出；有则挂起等待前端确认，并排定
/// 超时兜底（前端无响应 EXIT_CONFIRM_TIMEOUT 后自动放行）。
pub fn request_exit(app: &AppHandle) {
    let has_active = app
        .try_state::<RunManager>()
        .is_some_and(|runs| runs.has_active());
    if !has_active {
        app.exit(0);
        return;
    }
    if let Some(state) = app.try_state::<AppState>() {
        state
            .exit_confirm
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .request(Instant::now());
    }
    let _ = app.emit(keys::APP_EXIT_REQUESTED, json!({ "hasActiveRuns": true }));
    // 超时兜底任务：到点复查挂起状态——前端已回执（resolve 置位解除）则
    // no-op；仍挂起即视为无响应，放行退出（与 resolve 竞态无害：两边都先
    // 解除挂起再退出，只有先到者生效）。
    let app_for_timer = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(EXIT_CONFIRM_TIMEOUT).await;
        let expired = app_for_timer.try_state::<AppState>().is_some_and(|state| {
            state
                .exit_confirm
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .timed_out(Instant::now())
        });
        if expired {
            app_for_timer.exit(0);
        }
    });
}

#[cfg(test)]
#[path = "shell_test.rs"]
mod shell_test;
