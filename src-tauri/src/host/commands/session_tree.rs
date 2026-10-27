//! 会话树命令（§3.3 #26 `save_session_tree`；本切片按票面补充 `load_session_tree`）。
//!
//! 命令层只做参数平铺与转调（§2.2：host = 校验/转调），逻辑全在 storage 引擎。
//! 负载字段 camelCase、命令名 snake_case（D14）；Tauri 自动完成 snake_case 形参映射。

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::host::IpcError;
use crate::storage::{MessageEntry, SessionStore, SessionTreeData};

/// `save_session_tree` 参数契约（§3.3 #26；文档命名锚点，命令形参按字段平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSessionTreeArgs {
    pub session_id: String,
    pub messages: Vec<MessageEntry>,
    pub active_leaf_id: Option<String>,
    pub base_revision: u64,
}

/// `save_session_tree` 返回契约（§3.3 #26：`{ revision }`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSessionTreeResult {
    pub revision: u64,
}

/// `load_session_tree` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadSessionTreeArgs {
    pub session_id: String,
}

/// 全树同步（§6.3）：前端 `thread.export()` 全树 → 仅追加缺失节点 → revision+1。
#[tauri::command]
pub(crate) async fn save_session_tree(
    store: State<'_, SessionStore>,
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

/// 会话树加载（承载 §6.1 定稿结构：messages 含 `status?` + activeLeafId + revision）。
#[tauri::command]
pub(crate) async fn load_session_tree(
    store: State<'_, SessionStore>,
    session_id: String,
) -> Result<SessionTreeData, IpcError> {
    let args = LoadSessionTreeArgs { session_id };
    Ok(store.load_session_tree(&args.session_id)?)
}
