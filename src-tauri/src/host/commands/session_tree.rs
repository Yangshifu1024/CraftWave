//! 会话树与持久化域命令（`save/load_session_tree`，
//! `list_sessions`、`load_tool_outcomes`）。
//!
//! 命令层只做参数平铺与转调（host = 校验/转调），逻辑全在 storage 引擎。
//! 负载字段 camelCase、命令名 snake_case 约定；Tauri 自动完成 snake_case 形参映射。

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

use crate::host::IpcError;
use crate::storage::{MessageEntry, SessionMeta, SessionStore, SessionTreeData, load_outcomes};

/// `save_session_tree` 参数契约（文档命名锚点，命令形参按字段平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SaveSessionTreeArgs {
    pub session_id: String,
    pub messages: Vec<MessageEntry>,
    #[cfg_attr(test, ts(optional))]
    pub active_leaf_id: Option<String>,
    pub base_revision: u64,
}

/// `save_session_tree` 返回契约（`{ revision }`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SaveSessionTreeResult {
    pub revision: u64,
}

/// `load_session_tree` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct LoadSessionTreeArgs {
    pub session_id: String,
}

/// 全树同步：前端 `thread.export()` 全树 → 仅追加缺失节点 → revision+1。
#[tauri::command]
pub(crate) async fn save_session_tree(
    store: State<'_, Arc<SessionStore>>,
    session_id: String,
    messages: Vec<MessageEntry>,
    active_leaf_id: Option<String>,
    base_revision: u64,
) -> Result<SaveSessionTreeResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = SaveSessionTreeArgs {
        session_id,
        messages,
        active_leaf_id,
        base_revision,
    };
    let revision = store.save_session_tree(
        &args.session_id,
        &args.messages,
        args.active_leaf_id.as_deref(),
        args.base_revision,
    )?;
    Ok(SaveSessionTreeResult { revision })
}

/// 会话树加载（定稿结构：messages 含 `status?` + activeLeafId + revision）。
#[tauri::command]
pub(crate) async fn load_session_tree(
    store: State<'_, Arc<SessionStore>>,
    session_id: String,
) -> Result<SessionTreeData, IpcError> {
    let args = LoadSessionTreeArgs { session_id };
    Ok(store.load_session_tree(&args.session_id)?)
}

/// `list_sessions` 参数契约。
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsArgs {
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub project_id: Option<String>,
}

/// 会话列表：index.json 按 updatedAt 倒序；projectId 过滤
/// （None=临时会话；参数缺省返回全部，单会话前端不传）。
#[tauri::command]
pub(crate) async fn list_sessions(
    store: State<'_, Arc<SessionStore>>,
    project_id: Option<String>,
) -> Result<Vec<SessionMeta>, IpcError> {
    let args = ListSessionsArgs { project_id };
    let sessions = store.index().list();
    Ok(match args.project_id {
        Some(project) => sessions
            .into_iter()
            .filter(|m| m.project_id.as_deref() == Some(project.as_str()))
            .collect(),
        None => sessions,
    })
}

/// `load_tool_outcomes` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct LoadToolOutcomesArgs {
    pub session_id: String,
    pub call_ids: Vec<String>,
}

/// 工具卡恢复：按 callId 批量拉边车；缺文件的调用不在返回表
/// （前端以帧面 summary 降级渲染）。
#[tauri::command]
pub(crate) async fn load_tool_outcomes(
    store: State<'_, Arc<SessionStore>>,
    session_id: String,
    call_ids: Vec<String>,
) -> Result<HashMap<String, Value>, IpcError> {
    let args = LoadToolOutcomesArgs {
        session_id,
        call_ids,
    };
    Ok(load_outcomes(
        store.root(),
        &args.session_id,
        &args.call_ids,
    ))
}
