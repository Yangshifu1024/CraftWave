//! 会话生命周期域命令（§3.3 #13 `create_session` / #16 `delete_session` /
//! #17 `rename_session`，16 号票；#14 `list_sessions` 在 session_tree.rs）。
//!
//! 会话双形态（FR-01-02）：projectId 缺省 = 临时会话（roots=数据目录）；
//! projectId 给定 = 项目会话（roots=项目主目录，创建时校验项目存在）。

use serde::{Deserialize, Serialize};
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
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionArgs {
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
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
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DeleteSessionArgs {
    pub session_id: String,
}

/// 删除会话主体（公开给集成测试；命令体转调——Q43 用量联动在本函数）。
pub fn delete_session_core(
    app: &AppState,
    runs: &RunManager,
    session_id: &str,
) -> Result<(), IpcError> {
    if runs.get(session_id).is_some() {
        return Err(IpcError::new(
            crate::util::errors::E_SESSION_RUNNING,
            format!("会话运行中，先取消运行再删除：{session_id}"),
        ));
    }
    let existed = app
        .store
        .delete_session(session_id)
        .map_err(|e| IpcError::internal(format!("会话删除失败：{e:?}")))?;
    if !existed {
        return Err(IpcError::not_found(format!("会话不存在：{session_id}")));
    }
    // 用量明细与内存累计摘除（Q43：删除后明细不可查询；stats_cache 与
    // 聚合同构无会话身份，不需清理）
    app.ledger.detail.forget_session(session_id);
    app.session_usage
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(session_id);
    // 会话级审批记忆随删（23 号票，Q41：记忆不跨会话生命周期——删除后
    // 重建的同 id 会话须重新审批）
    app.approval_memory.forget(session_id);
    Ok(())
}

/// 删除会话（§3.3 #16）：运行中拒绝（E_SESSION_RUNNING）；index 条目 +
/// histories/prefs/toolres 一并清除；不存在 → E_NOT_FOUND。
/// 20 号票（Q43）：关联 run 用量明细（`<sid>.usage.json`）与内存累计表
/// 条目随删；日/模型匿名聚合（`stats/*.json`）保留——真实消耗数字不含
/// 会话身份（§7.7 Q43 匿名化定案）。
#[tauri::command]
pub(crate) async fn delete_session(
    app: State<'_, AppState>,
    runs: State<'_, RunManager>,
    session_id: String,
) -> Result<(), IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = DeleteSessionArgs { session_id };
    delete_session_core(&app, &runs, &args.session_id)
}

/// `rename_session` 参数契约（§3.3 #17）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
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

// ---- 会话偏好（§3.3 #24/#25，FR-14-05/FR-02-08，18 号票） ----

/// `get_session_prefs` 参数契约（§3.3 #24；文档命名锚点，命令形参按字段
/// 平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionPrefsArgs {
    pub session_id: String,
}

/// 会话存在于索引的校验（#24/#25 共用；不存在 → E_NOT_FOUND）。
fn ensure_session_exists(app: &AppState, session_id: &str) -> Result<(), IpcError> {
    let known = app.store.index().list().iter().any(|m| m.id == session_id);
    if known {
        Ok(())
    } else {
        Err(IpcError::not_found(format!("会话不存在：{session_id}")))
    }
}

/// `get_session_prefs` 返回契约（§3.3 #24）：modelId/thinking 未设置时省略
/// （前端按「跟随默认 / per-model reasoningEffort 回落」处理）。
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GetSessionPrefsResult {
    /// 会话级模型覆盖（ProviderModel.id）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub model_id: Option<String>,
    /// 会话 thinking 档（FR-02-08 五档）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub thinking: Option<crate::providers::reasoning::ThinkingLevel>,
}

/// `set_session_prefs` 参数契约（§3.3 #25）：merge 语义——**只传的字段被改**
/// （字段缺省 = 不动；`null` = 清除该字段回默认；设值 = 覆盖）。双 Option
/// （`Option<Option<T>>`）区分「未传」与「传了 null」；权限档明确不持久化
/// （FR-14-05，不在载荷面）。
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SetSessionPrefsArgs {
    pub session_id: String,
    /// 覆盖面：modelId 字段缺省 = 不动模型覆盖；null = 清除（回全局默认）；
    /// 字符串 = 设置覆盖。thinking 同构（null = 回落 per-model reasoningEffort）。
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub prefs: Option<SetSessionPrefsPayload>,
}

/// `set_session_prefs` 的覆盖载荷（§3.3 #25 `prefs` 字段；双 Option 语义见
/// [`SetSessionPrefsArgs`]）。
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SetSessionPrefsPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub model_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub thinking: Option<Option<crate::providers::reasoning::ThinkingLevel>>,
}

/// `get_session_prefs` 主体（公开给集成测试注入临时数据目录——不经 tauri
/// 运行时）。
pub(crate) fn get_session_prefs_core(
    app: &AppState,
    session_id: &str,
) -> Result<GetSessionPrefsResult, IpcError> {
    ensure_session_exists(app, session_id)?;
    let prefs = crate::storage::prefs::load_prefs(app.store.root(), session_id);
    Ok(GetSessionPrefsResult {
        model_id: prefs.model_id,
        thinking: prefs.thinking,
    })
}

/// `set_session_prefs` 主体（公开给集成测试；merge 语义见
/// [`SetSessionPrefsArgs`]）。
pub(crate) fn set_session_prefs_core(
    app: &AppState,
    session_id: &str,
    payload: SetSessionPrefsPayload,
) -> Result<(), IpcError> {
    ensure_session_exists(app, session_id)?;
    let mut current = crate::storage::prefs::load_prefs(app.store.root(), session_id);
    if let Some(model_id) = payload.model_id {
        // 空白归一为清除（与"null 清除"等价，宽容前端空串）
        current.model_id = model_id.filter(|id| !id.trim().is_empty());
    }
    if let Some(thinking) = payload.thinking {
        current.thinking = thinking;
    }
    crate::storage::prefs::save_prefs(app.store.root(), session_id, &current)
        .map_err(|e| IpcError::internal(format!("会话偏好写入失败：{e}")))
}

/// 读取会话偏好（§3.3 #24）：文件缺失/损坏 → 空偏好（不阻塞会话加载，
/// FR-14-05 容错口径）；会话不存在 → E_NOT_FOUND。
#[tauri::command]
pub(crate) async fn get_session_prefs(
    app: State<'_, AppState>,
    session_id: String,
) -> Result<GetSessionPrefsResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = SessionPrefsArgs { session_id };
    get_session_prefs_core(&app, &args.session_id)
}

/// 设置会话偏好（§3.3 #25）：merge 语义（只传的字段被改），原子写落
/// `<sid>.prefs.json`（§7.4）；写入即生效于运行中 run 的下一次模型请求
/// （§11.1 每拍刷新，FR-02-07 修正案）。会话不存在 → E_NOT_FOUND。
#[tauri::command]
pub(crate) async fn set_session_prefs(
    app: State<'_, AppState>,
    session_id: String,
    prefs: Option<SetSessionPrefsPayload>,
) -> Result<(), IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = SetSessionPrefsArgs { session_id, prefs };
    set_session_prefs_core(&app, &args.session_id, args.prefs.unwrap_or_default())
}

#[cfg(test)]
#[path = "session_prefs_test.rs"]
mod session_prefs_test;
