//! 会话域 API：会话树持久化、项目注册表、会话生命周期、会话偏好与图片附件。
//!
//! 平移来源（旧壳命令壳的框架无关 core，业务核心仍在 engine storage）：
//! - `host/commands/session_tree.rs`（save/load/list/outcomes）
//! - `host/commands/project.rs`（项目三命令）
//! - `host/commands/session.rs`（生命周期三命令 + 偏好两命令 + 删除盘点）
//! - `host/commands/image.rs`（图片附件两命令）
//!
//! 载荷类型沿用旧契约（camelCase serde）；`AppState` 参数传入（见模块文档）。

use std::collections::HashMap;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::app_state::AppState;
use crate::storage::{
    MessageEntry, ProjectError, ProjectMeta, SessionMeta, SessionTreeData, load_outcomes,
};
use crate::util::errors::{E_NOT_FOUND, E_STORAGE_IO, IpcError};

// ---- 会话树持久化（旧命令 save/load_session_tree、list_sessions、
// load_tool_outcomes） ----

/// `save_session_tree` 参数契约（文档命名锚点，与旧壳一字不差）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SaveSessionTreeArgs {
    pub session_id: String,
    pub messages: Vec<MessageEntry>,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub active_leaf_id: Option<String>,
    pub base_revision: u64,
}

/// `save_session_tree` 返回契约（`{ revision }`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SaveSessionTreeResult {
    pub revision: u64,
}

/// `load_session_tree` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct LoadSessionTreeArgs {
    pub session_id: String,
}

/// `list_sessions` 参数契约。
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ListSessionsArgs {
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub project_id: Option<String>,
}

/// `load_tool_outcomes` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct LoadToolOutcomesArgs {
    pub session_id: String,
    pub call_ids: Vec<String>,
}

/// 全树同步：调用方把整棵消息树交来，仅追加缺失节点，revision+1。
/// （GPUI 壳发送前先 save 新 user 拓扑、run 落盘器回写后重载，语义同旧栈。）
pub fn save_session_tree(
    app: &AppState,
    args: SaveSessionTreeArgs,
) -> Result<SaveSessionTreeResult, IpcError> {
    let revision = app.store.save_session_tree(
        &args.session_id,
        &args.messages,
        args.active_leaf_id.as_deref(),
        args.base_revision,
    )?;
    Ok(SaveSessionTreeResult { revision })
}

/// 会话树加载（定稿结构：messages 含 `status?` + activeLeafId + revision）。
pub fn load_session_tree(
    app: &AppState,
    args: LoadSessionTreeArgs,
) -> Result<SessionTreeData, IpcError> {
    Ok(app.store.load_session_tree(&args.session_id)?)
}

/// 会话列表：index.json 按 updatedAt 倒序；projectId 过滤
/// （None=临时会话；参数缺省返回全部）。
pub fn list_sessions(
    app: &AppState,
    args: ListSessionsArgs,
) -> Result<Vec<SessionMeta>, IpcError> {
    let sessions = app.store.index().list();
    Ok(match args.project_id {
        Some(project) => sessions
            .into_iter()
            .filter(|m| m.project_id.as_deref() == Some(project.as_str()))
            .collect(),
        None => sessions,
    })
}

/// 工具卡恢复：按 callId 批量拉边车；缺文件的调用不在返回表
/// （调用方以帧面 summary 降级渲染）。
pub fn load_tool_outcomes(
    app: &AppState,
    args: LoadToolOutcomesArgs,
) -> Result<HashMap<String, Value>, IpcError> {
    Ok(load_outcomes(
        app.store.root(),
        &args.session_id,
        &args.call_ids,
    ))
}

// ---- 项目（旧命令 list/save/delete_project） ----

/// `save_project` 参数契约（`{ project: { id?, name, homeDir } }`）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SaveProjectArgs {
    pub project: NewProjectInput,
}

/// 项目新建/更新的入参形态。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct NewProjectInput {
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub id: Option<String>,
    pub name: String,
    pub home_dir: String,
}

/// `delete_project` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DeleteProjectArgs {
    pub project_id: String,
}

/// ProjectError → IPC 错误码（旧壳 project.rs 同款映射）。
fn to_ipc_error(err: ProjectError) -> IpcError {
    match err {
        ProjectError::DuplicateHome(home) => {
            IpcError::invalid_arg(format!("目录已被其他项目注册：{home}"))
        }
        ProjectError::NotFound(id) => IpcError::not_found(format!("项目不存在：{id}")),
        ProjectError::Corrupt(msg) => IpcError::internal(format!("项目注册表损坏：{msg}")),
        ProjectError::Io(e) => IpcError::internal(format!("项目注册表 IO 失败：{e}")),
    }
}

/// 入参校验：名称非空；主目录必须是绝对路径（相对路径无法界定工具的
/// 读写边界，注册期直接拒绝）。
fn validate_input(name: &str, home_dir: &str) -> Result<(), IpcError> {
    if name.trim().is_empty() {
        return Err(IpcError::invalid_arg("项目名称不能为空"));
    }
    let path = std::path::Path::new(home_dir);
    if home_dir.trim().is_empty() || !path.is_absolute() {
        return Err(IpcError::invalid_arg(format!(
            "项目主目录必须为绝对路径：{home_dir}"
        )));
    }
    Ok(())
}

/// 项目列表。
pub fn list_projects(app: &AppState) -> Result<Vec<ProjectMeta>, IpcError> {
    app.projects.list().map_err(to_ipc_error)
}

/// 项目新建/更新：homeDir 变更触发 project.json 重绑。
pub fn save_project(app: &AppState, args: SaveProjectArgs) -> Result<ProjectMeta, IpcError> {
    validate_input(&args.project.name, &args.project.home_dir)?;
    app.projects
        .save(
            args.project.id.as_deref(),
            args.project.name.trim(),
            args.project.home_dir.trim(),
        )
        .map_err(to_ipc_error)
}

/// 项目删除：只删注册与 project.json；名下会话转临时（projectId 置空、
/// 历史保留——行为钉死）。运行中的 run 不受影响（roots 是 start_chat
/// 时解析的快照）。
pub fn delete_project(app: &AppState, args: DeleteProjectArgs) -> Result<(), IpcError> {
    let Some(removed) = app
        .projects
        .delete(&args.project_id)
        .map_err(to_ipc_error)?
    else {
        return Err(IpcError::not_found(format!(
            "项目不存在：{}",
            args.project_id
        )));
    };
    // 名下会话转临时（历史保留）；无会话时静默完成
    for meta in app.store.index().list() {
        if meta.project_id.as_deref() == Some(removed.id.as_str()) {
            app.store
                .index()
                .mutate(&meta.id, |m| m.project_id = None)
                .map_err(|e| IpcError::internal(format!("会话转临时失败：{e:?}")))?;
        }
    }
    Ok(())
}

// ---- 会话生命周期（旧命令 create/delete/rename_session） ----

/// 新会话默认标题（改名走 `rename_session`；自动起名由后台标题任务负责，
/// 平移随 start_chat 装配树走）。
pub const DEFAULT_SESSION_TITLE: &str = "新会话";

/// `create_session` 参数契约。
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionArgs {
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub project_id: Option<String>,
}

/// `delete_session` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DeleteSessionArgs {
    pub session_id: String,
}

/// `rename_session` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RenameSessionArgs {
    pub session_id: String,
    pub title: String,
}

/// 新建会话：index.json 建条目；历史文件随首次 save 产生。
/// projectId 给定时校验项目存在（项目会话 roots = 项目主目录；
/// 缺省 = 临时会话，roots = 数据目录）。
pub fn create_session(
    app: &AppState,
    args: CreateSessionArgs,
) -> Result<SessionMeta, IpcError> {
    if let Some(pid) = &args.project_id
        && app.projects.home_of(pid).is_none()
    {
        return Err(IpcError::not_found(format!("项目不存在：{pid}")));
    }
    let ts = crate::storage::trust::now_ms();
    let meta = SessionMeta {
        id: crate::util::ids::new_uuid(),
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

/// 删除会话边界（旧壳 `delete_session_core` 平移）：运行中拒绝
/// （E_SESSION_RUNNING）；index 条目 + histories/prefs/toolres 一并清除；
/// 不存在 → E_NOT_FOUND。关联 run 用量明细（`<sid>.usage.json`）与内存
/// 累计表条目随删；日/模型匿名聚合（`stats/*.json`）保留——真实消耗数字
/// 不含会话身份（匿名化）。
pub fn delete_session(app: &AppState, args: DeleteSessionArgs) -> Result<(), IpcError> {
    if app.runs.get(&args.session_id).is_some() {
        return Err(IpcError::new(
            crate::util::errors::E_SESSION_RUNNING,
            format!("session is running; cancel it before deleting: {}", args.session_id),
        ));
    }
    let existed = app
        .store
        .delete_session(&args.session_id)
        .map_err(|e| IpcError::internal(format!("session deletion failed: {e:?}")))?;
    if !existed {
        return Err(IpcError::not_found(format!(
            "session not found: {}",
            args.session_id
        )));
    }
    // 用量明细与内存累计摘除（删除后明细不可查询；stats_cache 与
    // 聚合同构无会话身份，不需清理）
    app.ledger.detail.forget_session(&args.session_id);
    app.session_usage
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&args.session_id);
    // 会话级审批记忆随删（记忆不跨会话生命周期——删除后
    // 重建的同 id 会话须重新审批）
    app.approval_memory.forget(&args.session_id);
    // 会话级资源信任集与计划范围授权桶随删（进程内内存面，
    // 删除后不残留——「本次会话允许」语义随会话销毁）
    app.session_trust.forget(&args.session_id);
    app.plan_scopes.forget(&args.session_id);
    // followUp 队列文件（会话专属副本随删；队列文件
    // 不删会残留正文副本）
    let queue_file = app
        .store
        .root()
        .join("sessions")
        .join(format!("{}.queue.json", args.session_id));
    match std::fs::remove_file(&queue_file) {
        // 不存在 = 本来就干净
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(IpcError::new(
                crate::util::errors::E_IO,
                format!("删除会话残留失败：{}：{e}", queue_file.display()),
            ));
        }
    }
    // 子代理历史盘点清理（`histories/subs/<parentRunId>/` 按
    // meta.json 的 sessionId 归属——命中即删整个父目录；meta 缺失/损坏
    // 的目录不误删（保守：宁保留不误删）。删除失败记账汇入失败清单）
    let mut all_failures = app.session_log.close_and_purge(&args.session_id);
    all_failures.extend(purge_sub_histories(app.store.root(), &args.session_id));
    // 删除失败必须报告（红线：不能声称全部删除成功）
    if !all_failures.is_empty() {
        return Err(IpcError::new(
            crate::util::errors::E_IO,
            format!(
                "会话已删除，但部分专属副本清理失败（残留清单）：{}",
                all_failures.join("；")
            ),
        ));
    }
    Ok(())
}

/// 子代理历史目录的会话归属盘点（旧壳 `purge_sub_histories` 平移）：
/// 遍历 `histories/subs/*/meta.json`，`session_id` 匹配则删除整个
/// `<parentRunId>/` 目录。返回失败清单（删除失败才报；目录不存在 =
/// 没有子历史，静默）。
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

/// 重命名会话（返回 SessionMeta）：index 更新标题与 updatedAt。
pub fn rename_session(app: &AppState, args: RenameSessionArgs) -> Result<SessionMeta, IpcError> {
    let trimmed = args.title.trim();
    if trimmed.is_empty() {
        return Err(IpcError::invalid_arg("session title must not be empty"));
    }
    app.store
        .index()
        .mutate(&args.session_id, |m| {
            m.title = trimmed.to_owned();
            m.updated_at = crate::storage::trust::now_ms();
        })
        .map_err(|e| IpcError::internal(format!("会话索引写入失败：{e:?}")))?
        .ok_or_else(|| IpcError::not_found(format!("session not found: {}", args.session_id)))
}

// ---- 会话偏好（旧命令 get/set_session_prefs） ----

/// `get_session_prefs` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionPrefsArgs {
    pub session_id: String,
}

/// `get_session_prefs` 返回契约：modelId/thinking 未设置时省略
/// （调用方按「跟随默认 / per-model reasoningEffort 回落」处理）。
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GetSessionPrefsResult {
    /// 会话级模型覆盖（ProviderModel.id）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub model_id: Option<String>,
    /// 会话 thinking 档（五档）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub thinking: Option<crate::providers::reasoning::ThinkingLevel>,
}

/// `set_session_prefs` 参数契约：merge 语义——**只传的字段被改**
/// （字段缺省 = 不动；`null` = 清除该字段回默认；设值 = 覆盖）。双 Option
/// （`Option<Option<T>>`）区分「未传」与「传了 null」；权限档明确不持久化
/// （不在载荷面）。
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SetSessionPrefsArgs {
    pub session_id: String,
    /// 覆盖面：modelId 字段缺省 = 不动模型覆盖；null = 清除（回全局默认）；
    /// 字符串 = 设置覆盖。thinking 同构（null = 回落 per-model reasoningEffort）。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub prefs: Option<SetSessionPrefsPayload>,
}

/// `set_session_prefs` 的覆盖载荷（`prefs` 字段；双 Option 语义见
/// [`SetSessionPrefsArgs`]）。
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SetSessionPrefsPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub model_id: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub thinking: Option<Option<crate::providers::reasoning::ThinkingLevel>>,
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

/// 读取会话偏好：文件缺失/损坏 → 空偏好（不阻塞会话加载，
/// 容错口径）；会话不存在 → E_NOT_FOUND。
pub fn get_session_prefs(
    app: &AppState,
    args: SessionPrefsArgs,
) -> Result<GetSessionPrefsResult, IpcError> {
    ensure_session_exists(app, &args.session_id)?;
    let prefs = crate::storage::prefs::load_prefs(app.store.root(), &args.session_id);
    Ok(GetSessionPrefsResult {
        model_id: prefs.model_id,
        thinking: prefs.thinking,
    })
}

/// 设置会话偏好：merge 语义（只传的字段被改），原子写落
/// `<sid>.prefs.json`；写入即生效于运行中 run 的下一次模型请求
/// （每拍刷新）。会话不存在 → E_NOT_FOUND。
pub fn set_session_prefs(app: &AppState, args: SetSessionPrefsArgs) -> Result<(), IpcError> {
    ensure_session_exists(app, &args.session_id)?;
    let mut current = crate::storage::prefs::load_prefs(app.store.root(), &args.session_id);
    let payload = args.prefs.unwrap_or_default();
    if let Some(model_id) = payload.model_id {
        // 空白归一为清除（与"null 清除"等价，宽容调用方空串）
        current.model_id = model_id.filter(|id| !id.trim().is_empty());
    }
    if let Some(thinking) = payload.thinking {
        current.thinking = thinking;
    }
    crate::storage::prefs::save_prefs(app.store.root(), &args.session_id, &current)
        .map_err(|e| IpcError::internal(format!("session prefs write failed: {e}")))
}

// ---- 图片附件（旧命令 attach_image / read_image_blob） ----

/// `attach_image` 参数契约：`dataBase64` 与 `filePath` 二选一
/// （附件按钮/粘贴传字节；拖拽入口只有绝对路径，传 filePath 由引擎读取）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AttachImageArgs {
    pub session_id: String,
    /// 图片字节的 base64 编码（与 filePath 二选一；解码后 ≤5MiB，png/jpeg/
    /// gif/webp 白名单按文件头魔数识别——mimeType 参数仅作声明，落盘 mime
    /// 以嗅探结果为准）。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub data_base64: Option<String>,
    /// 图片绝对路径（与 dataBase64 二选一；拖拽事件入口形态）。后端读取后
    /// 照走魔数嗅探与 5MiB 校验；**不做 roots 校验**：用户显式拖入与文件
    /// 选择器选图等价，模型侧文件可见性仍由 read 管线把关。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub file_path: Option<String>,
    /// 声明的 MIME 类型（须落在 png/jpeg/gif/webp 白名单内，防误传
    /// 非图片内容；filePath 形态时忽略——以魔数嗅探为准）。
    pub mime_type: String,
}

/// `attach_image` 返回契约：内容寻址哈希（SHA-256 前 32 hex）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AttachImageResult {
    pub blob_hash: String,
}

/// MIME 白名单（与 `storage::imgblob::sniff_image_mime` 同一份口径）。
const MIME_WHITELIST: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// 上传图片附件：dataBase64 解码或 filePath 读取 → 白名单与
/// 字节数校验 → 内容寻址落盘（同哈希幂等去重）。返回 blobHash 供消息 part
/// 引用。会话日志不记图片字节（日志纪律：仅元数据可入日志面）。
pub fn attach_image(app: &AppState, args: AttachImageArgs) -> Result<AttachImageResult, IpcError> {
    if args.session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    // 两种入口形态二选一（都空/都有按参数错误拒绝——防调用方歧义）
    let bytes: Vec<u8> = match (args.data_base64, args.file_path) {
        (Some(_), Some(_)) => {
            return Err(IpcError::invalid_arg("dataBase64 与 filePath 只能二选一"));
        }
        (None, None) => {
            return Err(IpcError::invalid_arg("dataBase64 与 filePath 至少给一个"));
        }
        (Some(b64), None) => {
            if !MIME_WHITELIST.contains(&args.mime_type.as_str()) {
                return Err(IpcError::invalid_arg(format!(
                    "不支持的图片类型：{}（仅 png/jpeg/gif/webp）",
                    args.mime_type
                )));
            }
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| IpcError::invalid_arg(format!("dataBase64 不是合法 base64：{e}")))?
        }
        (None, Some(path)) => {
            // 拖拽入口：后端读路径（用户显式拖入与文件选择器等价，不做
            // roots 校验），mime 按魔数嗅探（mimeType 声明忽略）
            std::fs::read(&path).map_err(|e| {
                IpcError::new(
                    E_NOT_FOUND,
                    format!("读取拖入图片失败：{path}（{e}）"),
                )
            })?
        }
    };
    let blob_hash = crate::storage::imgblob::put_blob(app.store.root(), &args.session_id, &bytes)
        .map_err(|e| match e {
            crate::storage::StorageError::InvalidArg(m) => IpcError::invalid_arg(m),
            other => IpcError::new(E_STORAGE_IO, other.to_string()),
        })?;
    Ok(AttachImageResult { blob_hash })
}

/// `read_image_blob` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ReadImageBlobArgs {
    pub session_id: String,
    pub blob_hash: String,
}

/// `read_image_blob` 返回契约：base64 字节 + 嗅探出的 MIME。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ReadImageBlobResult {
    pub data_base64: String,
    pub mime_type: String,
}

/// 读取图片 blob：渲染层 `<img>`（GPUI ImageSource）与历史恢复的取数通道。
/// root 归属：仅本会话 imgblob 可读；blob 缺失（会话目录被清等）按
/// E_NOT_FOUND 拒绝（调用方据此灰显占位）。
pub fn read_image_blob(
    app: &AppState,
    args: ReadImageBlobArgs,
) -> Result<ReadImageBlobResult, IpcError> {
    if args.session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    let (bytes, mime) = crate::storage::get_blob(app.store.root(), &args.session_id, &args.blob_hash)
        .map_err(|e| match e {
            crate::storage::StorageError::NotFound(m) => IpcError::new(E_NOT_FOUND, m),
            crate::storage::StorageError::InvalidArg(m) => IpcError::invalid_arg(m),
            other => IpcError::new(E_STORAGE_IO, other.to_string()),
        })?;
    Ok(ReadImageBlobResult {
        data_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        mime_type: mime.to_owned(),
    })
}
