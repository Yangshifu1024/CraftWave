//! git 只读面板命令壳（gpui-rewrite 阶段 1 自 engine/git/mod.rs 剥出）。
//!
//! 校验/装配薄壳：roots 解析 + `*_core` 转调；契约类型（Args/Result）
//! 与核心逻辑（`status::{status_core,diff_core,log_core}`）在
//! `craftwave_engine::git`。

use tauri::State;

use craftwave_engine::app_state::AppState;
use craftwave_engine::git::{
    GitDiffArgs, GitDiffResult, GitLogArgs, GitLogResult, GitStatusArgs, GitStatusResult,
};
use craftwave_engine::session_roots::resolve_session_roots;
use craftwave_engine::util::errors::IpcError;

use craftwave_engine::git::status;

/// `git_status`：仓库根 = sessionId → 会话 roots → 项目主目录
/// 向上找 .git；临时会话的 workspace 根通常非仓库（返回 isRepo: false）。
#[tauri::command]
pub(crate) async fn git_status(
    app: State<'_, AppState>,
    args: GitStatusArgs,
) -> Result<GitStatusResult, IpcError> {
    let root = resolve_session_roots(&app, &args.session_id).project_root;
    Ok(status::status_core(&root))
}

/// `git_diff`：staged 参数忽略（聚合视图，见 Args 说明）。
#[tauri::command]
pub(crate) async fn git_diff(
    app: State<'_, AppState>,
    args: GitDiffArgs,
) -> Result<GitDiffResult, IpcError> {
    let root = resolve_session_roots(&app, &args.session_id).project_root;
    Ok(status::diff_core(&root, args.staged))
}

/// `git_recent_log`。
#[tauri::command]
pub(crate) async fn git_recent_log(
    app: State<'_, AppState>,
    args: GitLogArgs,
) -> Result<GitLogResult, IpcError> {
    let root = resolve_session_roots(&app, &args.session_id).project_root;
    Ok(status::log_core(&root, args.limit))
}
