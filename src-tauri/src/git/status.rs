//! git 只读查询核心：git2-rs（libgit2）
//! 纯本地只读绑定——status（工作区状态）/ diff（工作区 vs HEAD 聚合差异）/
//! recent log（最近提交）与 user info（当前 git 用户名/邮箱）。
//!
//! 纪律：
//! - **只读**：本模块绝不 commit/push/merge，绝不改写 index 暂存状态与
//!   工作区文件——git 写操作只经 command 围栏走审批；
//! - **非 git 仓库不报错**：仓库根解析失败（`Repository::discover` 找不到
//!   `.git`）返回 `isRepo: false` 的空结果（前端据此显示「当前项目不是
//!   git 仓库」空态）；
//! - **git2 错误降级**：仓库打开成功但操作失败（脏索引/对象损坏等）→
//!   warn 日志 + 降级为空结果（`isRepo: true`），不阻塞会话；
//! - **防炸预算**：diff 单文件行数与文件数上限、状态条目数上限（超大
//!   仓库不拖垮 UI），超限截断并以 `truncated` 标记。

use std::path::Path;

use super::{
    DiffHunk, DiffLine, DiffLineKind, DiffPatch, GitCommitInfo, GitDiffResult, GitLogResult,
    GitStatusFile, GitStatusResult, GitUser,
};

/// diff 单文件行数上限（hunk 内容行累计；超出即截断该文件并标记 truncated）。
const MAX_DIFF_LINES_PER_FILE: usize = 3000;
/// diff 文件数上限（防超大仓库一次拉全量拖垮 UI）。
const MAX_DIFF_FILES: usize = 200;
/// recent log 条数缺省值（默认 20）。
pub const DEFAULT_LOG_LIMIT: usize = 20;
/// recent log 条数上限（防误传超大 limit 拖垮 UI）。
pub const MAX_LOG_LIMIT: usize = 100;
/// 状态文件条目数上限（防超大仓库全量扫描拖垮 UI；超出即截断）。
const MAX_STATUS_FILES: usize = 2000;

/// 状态值：`"M"` 修改 / `"A"` 新增（含已
/// 暂存新文件）/ `"D"` 删除 / `"U"` 未合并（冲突）/ `"untracked"` 未跟踪。
/// 一个文件同时存在暂存与工作区两类变化时按「冲突 > 删除 > 新增 > 修改 >
/// 未跟踪」优先级取单一主状态（面板徽标一文件一枚）。
pub const STATUS_MODIFIED: &str = "M";
pub const STATUS_ADDED: &str = "A";
pub const STATUS_DELETED: &str = "D";
pub const STATUS_CONFLICTED: &str = "U";
pub const STATUS_UNTRACKED: &str = "untracked";

/// 仓库根解析（sessionId → 会话 roots → 项目主目录向上找 .git）：
/// `Repository::discover` 从给定目录沿父目录向上查找——项目主目录本身或
/// 其子目录是仓库内路径都命中；找不到 → None（非 git 仓库，前端空态）。
pub fn discover_repo(root: &Path) -> Option<git2::Repository> {
    git2::Repository::discover(root).ok()
}

/// `git_status` 主体：状态列表 + user info。
///
/// - `files`：含 untracked（目录递归展开），尊重 .gitignore（不列忽略
///   文件），排除子模块；路径为仓库内相对路径；
/// - `user`：仓库配置的 `user.name` / `user.email`
///   （repo > global > system 叠加语义与 `git config` 一致），键缺失 → None。
pub fn status_core(root: &Path) -> GitStatusResult {
    let Some(repo) = discover_repo(root) else {
        return GitStatusResult {
            is_repo: false,
            files: Vec::new(),
            user: None,
        };
    };
    // 打开成功后的任何失败都降级为空结果（isRepo: true）+ warn 日志。
    let files = read_status_files(&repo);
    let user = read_user(&repo);
    GitStatusResult {
        is_repo: true,
        files,
        user,
    }
}

/// 状态条目读取（内部：失败返回空并 warn，不向上抛）。
fn read_status_files(repo: &git2::Repository) -> Vec<GitStatusFile> {
    let read = || -> Result<Vec<GitStatusFile>, git2::Error> {
        let mut opts = git2::StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .include_ignored(false)
            .exclude_submodules(true);
        let statuses = repo.statuses(Some(&mut opts))?;
        let mut files = Vec::new();
        for entry in statuses.iter().take(MAX_STATUS_FILES) {
            // git2 0.21 的 path() 返回 Result（非 UTF-8 路径带错），失败跳过。
            let Ok(path) = entry.path() else {
                continue;
            };
            if path.is_empty() {
                continue;
            }
            files.push(GitStatusFile {
                path: path.to_owned(),
                status: main_status(entry.status()).to_owned(),
            });
        }
        Ok(files)
    };
    read().unwrap_or_else(|e| {
        eprintln!("[craftwave] git status 读取失败，降级为空列表：{e}");
        Vec::new()
    })
}

/// git2 位标志 → 单一主状态（优先级：冲突 > 删除 > 新增 > 修改 > 未跟踪）。
fn main_status(s: git2::Status) -> &'static str {
    // 未合并（两边都改了同一处等冲突态）最优先警示。
    if s.contains(git2::Status::CONFLICTED) {
        return STATUS_CONFLICTED;
    }
    // 删除优先于新增/修改：工作区删了文件比「暂存区还留着旧内容」更值得警示。
    if s.intersects(git2::Status::WT_DELETED | git2::Status::INDEX_DELETED) {
        return STATUS_DELETED;
    }
    if s.contains(git2::Status::WT_NEW) {
        // 工作区新文件：已暂存（index 里也有它）→ "A"；未跟踪 → "untracked"。
        return if s.contains(git2::Status::INDEX_NEW) {
            STATUS_ADDED
        } else {
            STATUS_UNTRACKED
        };
    }
    if s.contains(git2::Status::INDEX_NEW) {
        // index 新增但工作区文件消失且未判删除（罕见边缘）→ 仍按新增展示。
        return STATUS_ADDED;
    }
    // 其余一律按修改（含 index/worktree 修改与重命名——重命名未开启检测，
    // libgit2 默认以删除+新增成对出现，各自有独立条目）。
    STATUS_MODIFIED
}

/// user info：仓库配置快照（repo > global > system）读
/// user.name / user.email；键缺失或读取失败 → None 字段（绝不报错）。
fn read_user(repo: &git2::Repository) -> Option<GitUser> {
    let Ok(config) = repo.config() else {
        return None;
    };
    let read = |key: &str| -> Option<String> {
        config
            .get_string(key)
            .ok()
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let name = read("user.name");
    let email = read("user.email");
    if name.is_none() && email.is_none() {
        return None;
    }
    Some(GitUser { name, email })
}

/// `git_diff` 主体：工作区（含 index 暂存）vs HEAD 的聚合差异。
///
/// - HEAD 不存在（空仓库无首提交）→ 与空树对比 = 全部文件都是新增；
/// - untracked 文件计入差异（内容行完整展示）；
/// - `staged` 参数不支持（staged 视图不做，参数忽略）——
///   返回的恒为「工作区 vs HEAD」聚合视图。
pub fn diff_core(root: &Path, _staged: Option<bool>) -> GitDiffResult {
    let Some(repo) = discover_repo(root) else {
        return GitDiffResult {
            is_repo: false,
            patches: Vec::new(),
        };
    };
    let patches = read_diff_patches(&repo);
    GitDiffResult {
        is_repo: true,
        patches,
    }
}

/// diff 补丁集读取（内部：失败返回空并 warn，不向上抛）。
fn read_diff_patches(repo: &git2::Repository) -> Vec<DiffPatch> {
    let read = || -> Result<Vec<DiffPatch>, git2::Error> {
        // HEAD 树：空仓库（unborn HEAD）→ None = 与空树对比，全部文件视为
        // 新增；其余 HEAD 错误（损坏引用等）→ 整体降级空结果（不当作空
        // 仓库伪造「全文件新增」）。
        let head_tree = match repo.head() {
            Ok(head) => head.peel_to_commit().ok().and_then(|c| c.tree().ok()),
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => None,
            Err(e) => {
                eprintln!("[craftwave] git diff 读取失败，降级为空列表：{e}");
                return Ok(Vec::new());
            }
        };
        let mut opts = git2::DiffOptions::new();
        opts.context_lines(3)
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);
        let diff = repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?;
        let total = diff.deltas().len().min(MAX_DIFF_FILES);
        let mut patches = Vec::with_capacity(total);
        for i in 0..total {
            // git2 0.21 的 get_delta 返回 Option（越界 None），缺失跳过。
            let Some(delta) = diff.get_delta(i) else {
                continue;
            };
            let path = delta
                .new_file()
                .path()
                .or_else(|| delta.old_file().path())
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            if path.is_empty() {
                continue;
            }
            // 二进制判定：libgit2 的 delta BINARY 标志对 untracked 内容不
            // 置位，按 git 同款启发式自嗅探（文件头 8000 字节含 NUL）。
            let binary = looks_binary(repo, &path);
            let (hunks, add_lines, del_lines, truncated) = extract_hunks(&diff, i);
            patches.push(DiffPatch {
                path,
                add_lines,
                del_lines,
                binary,
                truncated,
                hunks,
            });
        }
        Ok(patches)
    };
    read().unwrap_or_else(|e| {
        eprintln!("[craftwave] git diff 读取失败，降级为空列表：{e}");
        Vec::new()
    })
}

/// 二进制嗅探（git 启发式）：工作区文件头 8000 字节含 NUL 即二进制。
/// 只读面：读不到文件（删除/权限）按文本处理（binary=false）。
fn looks_binary(repo: &git2::Repository, rel_path: &str) -> bool {
    let Some(workdir) = repo.workdir() else {
        return false;
    };
    let Ok(bytes) = std::fs::read(workdir.join(rel_path)) else {
        return false;
    };
    let head = &bytes[..bytes.len().min(8000)];
    head.contains(&0u8)
}

/// 单文件 hunk 提取：行类别映射（'+' → add，'-' → del，' ' → ctx）+
/// 增删行计数 + 截断标记。文件头/hunk 头/EOF 标记行不进内容行；二进制
/// 文件无内容行（hunks 为空，binary 标记承载形态）。
fn extract_hunks(diff: &git2::Diff, index: usize) -> (Vec<DiffHunk>, usize, usize, bool) {
    let mut hunks: Vec<DiffHunk> = Vec::new();
    let mut add_lines = 0usize;
    let mut del_lines = 0usize;
    let mut collected = 0usize;
    let mut truncated = false;
    // from_diff 失败（个别 delta 无法生成补丁）按空补丁处理，不拖垮整批。
    let Ok(Some(mut patch)) = git2::Patch::from_diff(diff, index) else {
        return (hunks, 0, 0, false);
    };
    let _ = patch.print(&mut |_delta, hunk, line| {
        if truncated {
            return false; // 已到单文件行数预算，中断收集
        }
        // 内容行三类；'F' 文件头 / 'H' hunk 头 / '='·'>'·'<' EOF 标记行跳过。
        let kind = match line.origin() {
            '+' => DiffLineKind::Add,
            '-' => DiffLineKind::Del,
            ' ' => DiffLineKind::Ctx,
            _ => return true,
        };
        match line.origin() {
            '+' => add_lines += 1,
            '-' => del_lines += 1,
            _ => {}
        }
        let text = String::from_utf8_lossy(line.content())
            .trim_end_matches(['\n', '\r'])
            .to_owned();
        // hunk 归段：header 与当前末段一致 → 追加；新 header（或无 hunk）
        // 开新段（无 hunk 的内容行属文件级噪声，丢弃）。header 是原始字节
        // （git2 0.21 返回 &[u8]），按 UTF-8 损失转换取展示文本。
        let header = hunk
            .map(|h| {
                String::from_utf8_lossy(h.header())
                    .trim_end_matches(['\n', '\r'])
                    .to_owned()
            })
            .unwrap_or_default();
        if header.is_empty() {
            return true;
        }
        match hunks.last_mut() {
            Some(existing) if existing.header == header => {
                existing.lines.push(DiffLine { kind, text });
            }
            _ => {
                hunks.push(DiffHunk {
                    header,
                    lines: vec![DiffLine { kind, text }],
                });
            }
        }
        collected += 1;
        if collected >= MAX_DIFF_LINES_PER_FILE {
            truncated = true;
        }
        true
    });
    (hunks, add_lines, del_lines, truncated)
}

/// `git_recent_log` 主体：HEAD 起按时间倒序的最近提交（sha 全量 40 位
/// 十六进制 / summary 首行 / time Unix 秒）。空仓库（无 HEAD）→ 空列表。
pub fn log_core(root: &Path, limit: Option<usize>) -> GitLogResult {
    let Some(repo) = discover_repo(root) else {
        return GitLogResult {
            is_repo: false,
            commits: Vec::new(),
        };
    };
    let limit = limit.unwrap_or(DEFAULT_LOG_LIMIT).clamp(1, MAX_LOG_LIMIT);
    let commits = read_recent_log(&repo, limit);
    GitLogResult {
        is_repo: true,
        commits,
    }
}

/// 提交列表读取（内部：失败返回空并 warn，不向上抛）。
fn read_recent_log(repo: &git2::Repository, limit: usize) -> Vec<GitCommitInfo> {
    let read = || -> Result<Vec<GitCommitInfo>, git2::Error> {
        let mut walk = repo.revwalk()?;
        walk.push_head()?;
        walk.set_sorting(git2::Sort::TIME | git2::Sort::TOPOLOGICAL)?;
        let mut commits = Vec::new();
        for oid in walk.take(limit) {
            let oid = oid?;
            let commit = repo.find_commit(oid)?;
            // git2 0.21 的 summary() 返回 Result<Option<&str>>（非 UTF-8
            // 提交摘要带错），失败按空摘要降级。
            let summary = commit.summary().ok().flatten().unwrap_or("").to_owned();
            commits.push(GitCommitInfo {
                sha: oid.to_string(),
                summary,
                time: commit.time().seconds(),
            });
        }
        Ok(commits)
    };
    read().unwrap_or_else(|e| {
        eprintln!("[craftwave] git log 读取失败，降级为空列表：{e}");
        Vec::new()
    })
}

#[cfg(test)]
#[path = "status_test.rs"]
mod status_test;
