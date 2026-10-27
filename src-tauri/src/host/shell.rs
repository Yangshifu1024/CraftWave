//! 宿主外壳（§2.1/§9.6，FR-13-06/07/10，17 号票）：托盘、关闭到托盘、
//! 退出拦截、单实例唤起。
//!
//! 后台运行语义：主窗 close 请求一律拦截为隐藏（关闭到托盘默认开，配置
//! 项随设置域打磨票）；真正的退出只经托盘菜单「退出」，且存在活跃 run
//! 时先经 `app:exit_requested` 事件征得前端确认，前端调
//! `resolve_exit_request(ack)` 放行或取消（§9.6 退出拦截）。

use serde_json::json;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

use crate::agent::RunManager;
use crate::host::AppState;
use crate::util::events::keys;

/// 主窗口标签（tauri.conf.json `app.windows[0].label`）。
pub const MAIN_WINDOW: &str = "main";

/// 唤起并聚焦主窗（单实例二次启动、托盘点击、通知点击共用）。
pub fn activate_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// 装配托盘（图标 + 「显示主窗口 / 退出」菜单；左键单击唤起主窗）。
/// macOS 左键默认弹菜单，单击唤起在 Windows/Linux 生效（平台差异随
/// 打包票打磨）。
pub fn install_tray(app: &AppHandle) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出 CraftWave", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &quit_item])?;
    TrayIconBuilder::with_id("craftwave-tray")
        .icon(app.default_window_icon().expect("应用图标缺失").clone())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => activate_main_window(app),
            "quit" => request_exit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
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

/// 退出请求（§9.6）：无活跃 run 直接退出；有则挂起等待前端确认。
pub fn request_exit(app: &AppHandle) {
    let has_active = app
        .try_state::<RunManager>()
        .is_some_and(|runs| runs.has_active());
    if !has_active {
        app.exit(0);
        return;
    }
    if let Some(state) = app.try_state::<AppState>() {
        *state
            .exit_pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    }
    let _ = app.emit(keys::APP_EXIT_REQUESTED, json!({ "hasActiveRuns": true }));
}
