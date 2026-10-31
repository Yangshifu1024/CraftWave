//! 会话生命周期域命令（`create_session` / `delete_session` /
//! `rename_session`，`list_sessions` 在 session_tree.rs）。
//!
//! 会话双形态：projectId 缺省 = 临时会话（roots=数据目录）；
//! projectId 给定 = 项目会话（roots=项目主目录，创建时校验项目存在）。

use serde::{Deserialize, Serialize};
use tauri::State;

use craftwave_engine::agent::RunManager;
use craftwave_engine::app_state::AppState;
use craftwave_engine::util::errors::IpcError;
use craftwave_engine::storage::SessionMeta;
use craftwave_engine::util::ids::new_uuid;

/// 新会话默认标题（改名走 rename_session；自动起名由后台标题任务负责）。
pub(crate) const DEFAULT_SESSION_TITLE: &str = "新会话";

/// `create_session` 参数契约。
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

/// 新建会话：index.json 建条目；历史文件随首次 save 产生。
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
        authorized_external_dirs: Vec::new(),
    };
    app.store
        .index()
        .upsert(meta.clone())
        .map_err(|e| IpcError::internal(format!("session index write failed: {e:?}")))?;
    Ok(meta)
}

/// `delete_session` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DeleteSessionArgs {
    pub session_id: String,
}

/// 删除会话主体（公开给集成测试；命令体转调——用量联动在本函数）。
/// 删除边界：会话专属日志与轮转副本同步删除
/// （`close_and_purge` 失败清单非空 = 有残留，**必须报告**，不得声称
/// 全部删除成功）；followUp 队列文件、子代理历史目录一并盘点清理；
/// 会话级内存桶（审批记忆/资源信任/计划范围授权）随删。
pub fn delete_session_core(
    app: &AppState,
    runs: &RunManager,
    session_id: &str,
) -> Result<(), IpcError> {
    if runs.get(session_id).is_some() {
        return Err(IpcError::new(
            craftwave_engine::util::errors::E_SESSION_RUNNING,
            format!("session is running; cancel it before deleting: {session_id}"),
        ));
    }
    let existed = app
        .store
        .delete_session(session_id)
        .map_err(|e| IpcError::internal(format!("session deletion failed: {e:?}")))?;
    if !existed {
        return Err(IpcError::not_found(format!(
            "session not found: {session_id}"
        )));
    }
    // 用量明细与内存累计摘除（删除后明细不可查询；stats_cache 与
    // 聚合同构无会话身份，不需清理）
    app.ledger.detail.forget_session(session_id);
    app.session_usage
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(session_id);
    // 会话级审批记忆随删（记忆不跨会话生命周期——删除后
    // 重建的同 id 会话须重新审批）
    app.approval_memory.forget(session_id);
    // 会话级资源信任集与计划范围授权桶随删（盘点：进程内内存面，
    // 删除后不残留——「本次会话允许」语义随会话销毁）
    app.session_trust.forget(session_id);
    app.plan_scopes.forget(session_id);
    // followUp 队列文件（盘点：会话专属副本随删；队列文件
    // 不删会残留正文副本）
    let queue_file = app
        .store
        .root()
        .join("sessions")
        .join(format!("{session_id}.queue.json"));
    match std::fs::remove_file(&queue_file) {
        // 不存在 = 本来就干净
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(IpcError::new(
                craftwave_engine::util::errors::E_IO,
                format!("删除会话残留失败：{}：{e}", queue_file.display()),
            ));
        }
    }
    // 子代理历史盘点清理（`histories/subs/<parentRunId>/` 按
    // meta.json 的 sessionId 归属——命中即删整个父目录；meta 缺失/损坏
    // 的目录不误删（保守：宁保留不误删，不误伤原则）。删除失败记账
    // 汇入失败清单）
    let mut all_failures = app.session_log.close_and_purge(session_id);
    all_failures.extend(purge_sub_histories(app.store.root(), session_id));
    // 删除失败必须报告（红线：不能声称全部删除成功）
    if !all_failures.is_empty() {
        return Err(IpcError::new(
            craftwave_engine::util::errors::E_IO,
            format!(
                "会话已删除，但部分专属副本清理失败（残留清单）：{}",
                all_failures.join("；")
            ),
        ));
    }
    Ok(())
}

/// 子代理历史目录的会话归属盘点：遍历 `histories/subs/*/meta.json`，
/// `session_id` 匹配则删除整个 `<parentRunId>/` 目录。返回失败清单
/// （删除失败才报；目录不存在 = 没有子历史，静默）。
fn purge_sub_histories(data_root: &std::path::Path, session_id: &str) -> Vec<String> {
    let subs_root = data_root.join("histories").join("subs");
    let Ok(entries) = std::fs::read_dir(&subs_root) else {
        return Vec::new();
    };
    let mut failures = Vec::new();
    for entry in entries.flatten() {
        let run_dir = entry.path();
        let meta_path = run_dir.join("meta.json");
        let Ok(bytes) = std::fs::read(&meta_path) else {
            continue; // meta 缺失/不可读：无法归属，保守保留
        };
        let Ok(meta) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue; // meta 损坏：无法归属，保守保留
        };
        if meta.get("sessionId").and_then(|v| v.as_str()) == Some(session_id)
            && let Err(e) = std::fs::remove_dir_all(&run_dir)
        {
            failures.push(format!("{}: {e}", run_dir.display()));
        }
    }
    failures
}

/// 删除会话：运行中拒绝（E_SESSION_RUNNING）；index 条目 +
/// histories/prefs/toolres 一并清除；不存在 → E_NOT_FOUND。
/// 关联 run 用量明细（`<sid>.usage.json`）与内存累计表
/// 条目随删；日/模型匿名聚合（`stats/*.json`）保留——真实消耗数字不含
/// 会话身份（匿名化）。
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

/// `rename_session` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RenameSessionArgs {
    pub session_id: String,
    pub title: String,
}

/// 重命名会话（返回 SessionMeta）：index 更新标题与 updatedAt。
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
        return Err(IpcError::invalid_arg("session title must not be empty"));
    }
    app.store
        .index()
        .mutate(&args.session_id, |m| {
            m.title = trimmed.to_owned();
            m.updated_at = now_ms();
        })
        .map_err(|e| IpcError::internal(format!("会话索引写入失败：{e:?}")))?
        .ok_or_else(|| IpcError::not_found(format!("session not found: {}", args.session_id)))
}

// ---- 会话偏好 ----

/// `get_session_prefs` 参数契约（文档命名锚点，命令形参按字段
/// 平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionPrefsArgs {
    pub session_id: String,
}

/// 会话存在于索引的校验（共用；不存在 → E_NOT_FOUND）。
fn ensure_session_exists(app: &AppState, session_id: &str) -> Result<(), IpcError> {
    let known = app.store.index().list().iter().any(|m| m.id == session_id);
    if known {
        Ok(())
    } else {
        Err(IpcError::not_found(format!(
            "session not found: {session_id}"
        )))
    }
}

/// `get_session_prefs` 返回契约：modelId/thinking 未设置时省略
/// （前端按「跟随默认 / per-model reasoningEffort 回落」处理）。
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GetSessionPrefsResult {
    /// 会话级模型覆盖（ProviderModel.id）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub model_id: Option<String>,
    /// 会话 thinking 档（五档）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub thinking: Option<craftwave_engine::providers::reasoning::ThinkingLevel>,
}

/// `set_session_prefs` 参数契约：merge 语义——**只传的字段被改**
/// （字段缺省 = 不动；`null` = 清除该字段回默认；设值 = 覆盖）。双 Option
/// （`Option<Option<T>>`）区分「未传」与「传了 null」；权限档明确不持久化
/// （不在载荷面）。
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

/// `set_session_prefs` 的覆盖载荷（`prefs` 字段；双 Option 语义见
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
    pub thinking: Option<Option<craftwave_engine::providers::reasoning::ThinkingLevel>>,
}

/// `get_session_prefs` 主体（公开给集成测试注入临时数据目录——不经 tauri
/// 运行时）。
pub(crate) fn get_session_prefs_core(
    app: &AppState,
    session_id: &str,
) -> Result<GetSessionPrefsResult, IpcError> {
    ensure_session_exists(app, session_id)?;
    let prefs = craftwave_engine::storage::prefs::load_prefs(app.store.root(), session_id);
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
    let mut current = craftwave_engine::storage::prefs::load_prefs(app.store.root(), session_id);
    if let Some(model_id) = payload.model_id {
        // 空白归一为清除（与"null 清除"等价，宽容前端空串）
        current.model_id = model_id.filter(|id| !id.trim().is_empty());
    }
    if let Some(thinking) = payload.thinking {
        current.thinking = thinking;
    }
    craftwave_engine::storage::prefs::save_prefs(app.store.root(), session_id, &current)
        .map_err(|e| IpcError::internal(format!("session prefs write failed: {e}")))
}

/// 读取会话偏好：文件缺失/损坏 → 空偏好（不阻塞会话加载，
/// 容错口径）；会话不存在 → E_NOT_FOUND。
#[tauri::command]
pub(crate) async fn get_session_prefs(
    app: State<'_, AppState>,
    session_id: String,
) -> Result<GetSessionPrefsResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = SessionPrefsArgs { session_id };
    get_session_prefs_core(&app, &args.session_id)
}

/// 设置会话偏好：merge 语义（只传的字段被改），原子写落
/// `<sid>.prefs.json`；写入即生效于运行中 run 的下一次模型请求
/// （每拍刷新，修正案）。会话不存在 → E_NOT_FOUND。
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
#[path = "session_delete_test.rs"]
mod session_delete_test;
#[cfg(test)]
#[path = "session_prefs_test.rs"]
mod session_prefs_test;
