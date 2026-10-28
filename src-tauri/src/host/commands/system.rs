//! 系统域命令（17 号票）：退出确认回执；19 号票：通知授权态查询与
//! 系统设置深链。
//!
//! §9.6 退出拦截的收口：托盘「退出」在存在活跃 run 时挂起并广播
//! `app:exit_requested`，前端弹确认框后调 `resolve_exit_request`——
//! `accepted=true` 放行真正退出，`false` 取消退出回到托盘常驻。
//! 19 号票补超时兜底（`shell::EXIT_CONFIRM_TIMEOUT`）：前端无响应时
//! 到点自动放行，用户退出意图优先，退出流程不因前端挂死而悬死。
//!
//! 通知引导（19 号票）：macOS 通知授权被拒/未决时设置页给引导——
//! `notification_permission_status` 返回真实授权态（插件自带的
//! permission_state 桌面端恒 Granted，不可用），`open_notification_settings`
//! 打开系统设置的通知面板。

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::host::AppState;
use crate::host::IpcError;
use crate::util::errors::E_INTERNAL;

/// `resolve_exit_request` 参数契约（§9.6：`{ accepted: boolean }`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveExitRequestArgs {
    /// true = 确认退出（结束活跃 run 并退出）；false = 取消退出。
    pub accepted: bool,
}

/// 退出确认回执（§9.6）。无挂起的退出请求时调用为无操作。
#[tauri::command]
pub(crate) async fn resolve_exit_request(
    app: State<'_, AppState>,
    handle: tauri::AppHandle,
    accepted: bool,
) -> Result<(), IpcError> {
    // 契约命名单一事实源：文档契约结构占位
    let _args = ResolveExitRequestArgs { accepted };
    // 解除挂起（19 号票超时兜底定时任务看到已解除即 no-op），再按回执放行
    let was_pending = {
        let mut confirm = app
            .exit_confirm
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let was = confirm.is_pending();
        confirm.resolve(accepted);
        was
    };
    if accepted && was_pending {
        // 活跃 run 由进程退出自然终止（取消传播全链在内核/运行时收口；
        // 数据安全由 append-only 崩溃恢复语义兜底，重启标「已中断」）
        handle.exit(0);
    }
    Ok(())
}

/// `notification_permission_status` 返回契约（§9.6，19 号票）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPermissionStatus {
    /// 授权态：granted（可送达）/ denied（被拒）/ prompt（未决定）/
    /// unknown（查不出，如 dev 裸进程无 bundle id）。
    pub status: String,
    /// 平台是否有「打开系统设置」入口（macOS/Windows 有；Linux 无统一入口，
    /// 前端据此决定是否渲染引导按钮）。
    pub has_settings_entry: bool,
}

/// 通知授权态查询（19 号票）：macOS 实查 UNUserNotificationCenter；
/// Windows/Linux 桌面通知无授权门，恒 granted。
#[tauri::command]
pub(crate) async fn notification_permission_status(
    app: tauri::AppHandle,
) -> Result<NotificationPermissionStatus, IpcError> {
    #[cfg(target_os = "macos")]
    {
        // 发起查询须在主线程（UNUserNotificationCenter 约束）；结果经
        // channel 回传，完成回调在系统队列（毫秒级）——阻塞等待收在
        // spawn_blocking，不占 async worker；2s 等不到按 unknown 口径。
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = app.run_on_main_thread(move || {
            crate::host::notify::query_macos_authorization(tx);
        });
        let status = tauri::async_runtime::spawn_blocking(move || {
            rx.recv_timeout(std::time::Duration::from_secs(2))
                .unwrap_or(crate::host::notify::AUTH_UNKNOWN)
        })
        .await
        .map_err(|e| IpcError::new(E_INTERNAL, format!("授权态查询失败：{e}")))?;
        Ok(NotificationPermissionStatus {
            status: status.to_owned(),
            has_settings_entry: true,
        })
    }
    #[cfg(windows)]
    {
        let _ = app;
        Ok(NotificationPermissionStatus {
            status: "granted".to_owned(),
            has_settings_entry: true,
        })
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = app;
        Ok(NotificationPermissionStatus {
            status: "granted".to_owned(),
            has_settings_entry: false,
        })
    }
}

/// 打开系统通知设置（19 号票设置页引导动作）。macOS 深链直达「通知」面板；
/// Windows 走 ms-settings:；Linux 无统一入口，报错由前端兜底（正常不触达：
/// 前端只在 hasSettingsEntry=true 时渲染按钮）。spawn 后不等待设置应用退出。
#[tauri::command]
pub(crate) fn open_notification_settings() -> Result<(), IpcError> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.notifications")
            .spawn()
            .map(|_| ())
            .map_err(|e| IpcError::new(E_INTERNAL, format!("打开系统设置失败：{e}")))
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：不闪控制台窗口
        std::process::Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:notifications"])
            .creation_flags(0x0800_0000)
            .spawn()
            .map(|_| ())
            .map_err(|e| IpcError::new(E_INTERNAL, format!("打开系统设置失败：{e}")))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Err(IpcError::new(E_INTERNAL, "该平台没有统一的通知设置入口"))
    }
}
