//! UI 状态域命令（§3.3 #30 `get_ui_state` / #31 `set_ui_state`，16 号票）。
//!
//! ui-state.json 承载 Tab 布局等前端自管状态（载荷 Json 原样存取，
//! §3.3：结构由前端定义；写侧防抖由前端负责）。

use serde::Deserialize;
use serde_json::Value;
use tauri::State;

use crate::host::AppState;
use crate::host::IpcError;
use crate::storage::config_store::ConfigPayload;

/// UI 状态是前端自定义的任意 JSON：没有固定形状可归一化，原样透传即可
/// （实现该 trait 只是为了复用 ConfigStore 的读写与落盘通道）。
impl ConfigPayload for Value {
    fn normalize(&mut self) -> Result<(), IpcError> {
        Ok(())
    }
}

/// `get_ui_state` 参数契约（§3.3 #30：`{}`）。
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GetUiStateArgs {}

/// 读取 UI 状态（§3.3 #30：Json 原样；从未保存过 → null）。
#[tauri::command]
pub(crate) async fn get_ui_state(app: State<'_, AppState>) -> Result<Value, IpcError> {
    // 契约命名单一事实源：文档契约结构占位（空参命令）
    let _args = GetUiStateArgs {};
    Ok(app.ui_state.load()?.as_ref().clone())
}

/// `set_ui_state` 参数契约（§3.3 #31：`{ state: unknown }`）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SetUiStateArgs {
    pub state: Value,
}

/// 保存 UI 状态（§3.3 #31：整份替换；防抖由前端负责）。
/// 顺带提取 `activeId` 更新活跃会话标识（17 号票：系统通知防打扰精判）。
#[tauri::command]
pub(crate) async fn set_ui_state(app: State<'_, AppState>, state: Value) -> Result<(), IpcError> {
    let args = SetUiStateArgs { state };
    let active = args.state.get("activeId").and_then(|v| v.as_str());
    *app.active_session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = active.map(str::to_owned);
    app.ui_state.save(args.state).map(|_| ())
}
