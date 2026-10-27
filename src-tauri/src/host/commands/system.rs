//! 系统域命令（17 号票）：退出确认回执。
//!
//! §9.6 退出拦截的收口：托盘「退出」在存在活跃 run 时挂起并广播
//! `app:exit_requested`，前端弹确认框后调 `resolve_exit_request`——
//! `accepted=true` 放行真正退出，`false` 取消退出回到托盘常驻。

use serde::Deserialize;
use tauri::State;

use crate::host::AppState;
use crate::host::IpcError;

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
    let mut pending = app
        .exit_pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let was_pending = *pending;
    *pending = false;
    if accepted && was_pending {
        // 活跃 run 由进程退出自然终止（取消传播全链在内核/运行时收口）
        handle.exit(0);
    }
    Ok(())
}
