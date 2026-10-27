//! 项目域命令（§3.3 #10 `list_projects` / #11 `save_project` / #12
//! `delete_project`，FR-10-01，16 号票）。
//!
//! 删除项目时其名下会话转临时（projectId 置空、历史保留，16 号票钉死）；
//! 运行中的 run 不受影响（roots 是 start_chat 时解析的快照）。

use serde::Deserialize;
use tauri::State;

use crate::host::AppState;
use crate::host::IpcError;
use crate::storage::{ProjectError, ProjectMeta};

/// `save_project` 参数契约（§3.3 #11：`{ project: { id?, name, homeDir } }`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProjectArgs {
    pub project: NewProjectInput,
}

/// 项目新建/更新的入参形态。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewProjectInput {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    pub home_dir: String,
}

/// `delete_project` 参数契约（§3.3 #12）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteProjectArgs {
    pub project_id: String,
}

/// ProjectError → D14 错误码（§3.6）。
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

/// 项目列表（§3.3 #10）。
#[tauri::command]
pub(crate) async fn list_projects(app: State<'_, AppState>) -> Result<Vec<ProjectMeta>, IpcError> {
    app.projects.list().map_err(to_ipc_error)
}

/// 项目新建/更新（§3.3 #11）：homeDir 变更触发 project.json 重绑。
#[tauri::command]
pub(crate) async fn save_project(
    app: State<'_, AppState>,
    project: NewProjectInput,
) -> Result<ProjectMeta, IpcError> {
    // 契约命名单一事实源：平铺入参回组为文档契约结构
    let args = SaveProjectArgs { project };
    validate_input(&args.project.name, &args.project.home_dir)?;
    app.projects
        .save(
            args.project.id.as_deref(),
            args.project.name.trim(),
            args.project.home_dir.trim(),
        )
        .map_err(to_ipc_error)
}

/// 项目删除（§3.3 #12）：只删注册与 project.json；名下会话转临时。
#[tauri::command]
pub(crate) async fn delete_project(
    app: State<'_, AppState>,
    project_id: String,
) -> Result<(), IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = DeleteProjectArgs { project_id };
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
