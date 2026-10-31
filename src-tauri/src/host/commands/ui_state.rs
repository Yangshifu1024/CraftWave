//! UI 状态域命令（`get_ui_state` / `set_ui_state`）。
//!
//! ui-state.json 承载 Tab 布局等前端自管状态（载荷 Json 原样存取，
//! 结构由前端定义；写侧防抖由前端负责）。

use serde::Deserialize;
use serde_json::Value;
use tauri::State;

use craftwave_engine::app_state::AppState;
use craftwave_engine::util::errors::IpcError;

/// `get_ui_state` 参数契约（`{}`）。
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GetUiStateArgs {}

/// 读取 UI 状态（Json 原样；从未保存过 → null）。
#[tauri::command]
pub(crate) async fn get_ui_state(app: State<'_, AppState>) -> Result<Value, IpcError> {
    // 契约命名单一事实源：文档契约结构占位（空参命令）
    let _args = GetUiStateArgs {};
    Ok(app.ui_state.load()?.as_ref().clone())
}

/// `set_ui_state` 参数契约（`{ state: unknown }`）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SetUiStateArgs {
    pub state: Value,
}

/// 保存 UI 状态（整份替换；防抖由前端负责）。
/// 顺带提取 `activeId` 更新活跃会话标识（系统通知防打扰精判）。
#[tauri::command]
pub(crate) async fn set_ui_state(app: State<'_, AppState>, state: Value) -> Result<(), IpcError> {
    let args = SetUiStateArgs { state };
    let active = args.state.get("activeId").and_then(|v| v.as_str());
    *app.active_session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = active.map(str::to_owned);
    app.ui_state.save(args.state).map(|_| ())
}
