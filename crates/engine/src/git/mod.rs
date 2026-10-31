//! git 集成域：
//!
//! - **只读面板数据面**：`git_status`（工作区状态 + user
//!   info）/ `git_diff`（工作区 vs HEAD 聚合差异）/ `git_recent_log`（最近
//!   提交）——变更面板（Inspector「变更」tab 打开的宽弹窗）的数据源；
//! - **写策略不在这里**：模型 git 写命令的禁止/审批由 command
//!   围栏与审批门承担（`safety::fence::ast::git_classify` 分类表 +
//!   `backend::gate::ApprovalPolicy.git_allow_stage_commit` 开关）——
//!   应用受控 worktree 管理（agent/worktree.rs）与模型 git 命令分开授权，
//!   本模块是纯只读查询，两者互不感知。
//!
//! 错误纪律：非 git 仓库 → `isRepo: false` 空结果（前端空态，不
//! 报错）；git2 操作失败 → warn 日志 + 降级空结果，命令恒成功返回。

pub mod status;

use serde::{Deserialize, Serialize};

// hunk 载体复用 backend::ask 中审批 diff（AskPayload.approval.diff）的
// 同源类型（单一事实源，TS 生成物已由审批面导出，这里只消费）。
use crate::backend::ask::{DiffHunk, DiffLine, DiffLineKind};

/// `git_status` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GitStatusArgs {
    pub session_id: String,
}

/// `git_diff` 参数契约。`staged` 暂不支持（staged diff 视图不做）——参数
/// 保留在契约面但被忽略，恒返回「工作区 vs HEAD」聚合视图。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GitDiffArgs {
    pub session_id: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub staged: Option<bool>,
}

/// `git_recent_log` 参数契约。`limit` 缺省 20，合法区间 1–100。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GitLogArgs {
    pub session_id: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub limit: Option<usize>,
}

/// 状态条目（`{ path, status }`）。status 取值域：`"M"` 修改 / `"A"` 新增 /
/// `"D"` 删除 / `"U"` 冲突 / `"untracked"`
/// 未跟踪（复合变化按优先级取单一主状态，面板徽标一文件一枚）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GitStatusFile {
    pub path: String,
    pub status: String,
}

/// git 用户信息（随 `git_status` 一并返回，无独立命令）：
/// 仓库配置的 user.name / user.email（键缺失 → None 字段；两者都缺 →
/// 整个 user 为 None）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GitUser {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub email: Option<String>,
}

/// `git_status` 返回：`isRepo` 区分「不是 git 仓库」
/// （前端空态文案）与「仓库但无变更」（空列表）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GitStatusResult {
    pub is_repo: bool,
    pub files: Vec<GitStatusFile>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub user: Option<GitUser>,
}

/// 单文件聚合 diff 补丁：结构复用
/// backend::ask 中审批 diff（AskPayload.approval.diff）的 `{ path, hunks }`
/// （hunk = header + 逐行
/// kind/text），增统计字段——`addLines`/`delLines` 增删行数、`binary` 二进制
/// 标记（无内容行）、`truncated` 单文件行数超预算截断标记（3000 行/文件）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DiffPatch {
    pub path: String,
    pub add_lines: usize,
    pub del_lines: usize,
    pub binary: bool,
    pub truncated: bool,
    pub hunks: Vec<DiffHunk>,
}

/// `git_diff` 返回：聚合补丁集（工作区含暂存 vs HEAD；untracked 计入）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GitDiffResult {
    pub is_repo: bool,
    pub patches: Vec<DiffPatch>,
}

/// 提交条目（`{ sha, summary, time }`）——sha 全量 40 位十六进
/// 制，summary 首行，time Unix 秒（前端格式化）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GitCommitInfo {
    pub sha: String,
    pub summary: String,
    pub time: i64,
}

/// `git_recent_log` 返回：最近提交列表（HEAD 起时间倒序）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GitLogResult {
    pub is_repo: bool,
    pub commits: Vec<GitCommitInfo>,
}
