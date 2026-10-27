//! 会话生命周期域命令（§3.3 #13 `create_session` / #16 `delete_session` /
//! #17 `rename_session`，16 号票；#14 `list_sessions` 在 session_tree.rs）。
//!
//! 会话双形态（FR-01-02）：projectId 缺省 = 临时会话（roots=数据目录）；
//! projectId 给定 = 项目会话（roots=项目主目录，创建时校验项目存在）。

use serde::Deserialize;
use tauri::State;

use crate::agent::RunManager;
use crate::host::AppState;
use crate::host::IpcError;
use crate::storage::SessionMeta;
use crate::util::ids::new_uuid;

/// 新会话默认标题（改名走 rename_session；自动起名属 M2）。
pub(crate) const DEFAULT_SESSION_TITLE: &str = "新会话";

/// `create_session` 参数契约（§3.3 #13）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionArgs {
    #[serde(default)]
    pub project_id: Option<String>,
}

/// 当前时间的 Unix 毫秒时间戳（取不到时兜底 0）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 新建会话（§3.3 #13）：index.json 建条目；历史文件随首次 save 产生。
#[tauri::command]
pub(crate) async fn create_session(
    app: State<'_, AppState>,
    project_id: Option<String>,
) -> Result<SessionMeta, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = CreateSessionArgs { project_id };
    if let Some(pid) = &args.project_id
        && app.projects.home_of(pid).is_none()
    {
        return Err(IpcError::not_found(format!("项目不存在：{pid}")));
    }
    let ts = now_ms();
    let meta = SessionMeta {
        id: new_uuid(),
        project_id: args.project_id,
        title: DEFAULT_SESSION_TITLE.to_owned(),
        created_at: ts,
        updated_at: ts,
        revision: 0,
        message_count: 0,
        total_tokens: 0,
    };
    app.store
        .index()
        .upsert(meta.clone())
        .map_err(|e| IpcError::internal(format!("会话索引写入失败：{e:?}")))?;
    Ok(meta)
}

/// `delete_session` 参数契约（§3.3 #16）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteSessionArgs {
    pub session_id: String,
}

/// 删除会话（§3.3 #16）：运行中拒绝（E_SESSION_RUNNING）；index 条目 +
/// histories/prefs/toolres 一并清除；不存在 → E_NOT_FOUND。
#[tauri::command]
pub(crate) async fn delete_session(
    app: State<'_, AppState>,
    runs: State<'_, RunManager>,
    session_id: String,
) -> Result<(), IpcError> {
    let args = DeleteSessionArgs { session_id };
    if runs.get(&args.session_id).is_some() {
        return Err(IpcError::new(
            crate::util::errors::E_SESSION_RUNNING,
            format!("会话运行中，先取消运行再删除：{}", args.session_id),
        ));
    }
    let existed = app
        .store
        .delete_session(&args.session_id)
        .map_err(|e| IpcError::internal(format!("会话删除失败：{e:?}")))?;
    if !existed {
        return Err(IpcError::not_found(format!(
            "会话不存在：{}",
            args.session_id
        )));
    }
    Ok(())
}

/// `rename_session` 参数契约（§3.3 #17）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameSessionArgs {
    pub session_id: String,
    pub title: String,
}

/// 重命名会话（§3.3 #17：返回 SessionMeta）：index 更新标题与 updatedAt。
#[tauri::command]
pub(crate) async fn rename_session(
    app: State<'_, AppState>,
    session_id: String,
    title: String,
) -> Result<SessionMeta, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = RenameSessionArgs { session_id, title };
    let trimmed = args.title.trim();
    if trimmed.is_empty() {
        return Err(IpcError::invalid_arg("会话标题不能为空"));
    }
    app.store
        .index()
        .mutate(&args.session_id, |m| {
            m.title = trimmed.to_owned();
            m.updated_at = now_ms();
        })
        .map_err(|e| IpcError::internal(format!("会话索引写入失败：{e:?}")))?
        .ok_or_else(|| IpcError::not_found(format!("会话不存在：{}", args.session_id)))
}
