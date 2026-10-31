//! 写入子代理的独立 worktree 管理（ADR-0001）。
//!
//! 名词解释：worktree（工作树）是 git 的原生能力——同一个仓库可以同时检出
//! 多个工作目录，各自独立修改互不影响。写入子代理在自己的 worktree 里改
//! 文件、跑局部测试，与主工作区（用户正在用的目录）天然隔离。
//!
//! 生命周期：
//! - **派发起点**：子代理获得派发时主工作区的当前文件状态——以 HEAD 提交为
//!   基线建 worktree，再把主工作区相对 HEAD 的全部差异（已暂存/未暂存修改
//!   与未被忽略的新文件；不含被忽略文件）以文件为单位复制进 worktree。
//!   基线差量（相对 HEAD 的起点内容指纹）记入 lease 元数据，供回收时判定
//!   冲突。快照创建失败不静默退回旧 HEAD（直接报错交由调用方回喂）。
//! - **差量回收**：子代理结束时把 worktree 相对起点（HEAD + 起点快照）的
//!   改动按文件汇回主工作区：无冲突（主工作区文件仍等于起点内容）直接
//!   应用；有冲突（主工作区在子代理运行期间被改过）保留双方——主工作区
//!   文件不动，子代理版本写到旁边的 `*.cw-conflict-<subId8>` 副本，由主
//!   代理结合当前代码解决并验证。全过程不自动 commit/push、不改用户暂存
//!   状态（直接写工作区文件，不经 index）。
//! - **保留与清理**：失败/取消的 worktree 连同修改保留（lease 标注
//!   abandoned），重启不自动续跑、不自动汇回；成功汇回并验证后才清理
//!   （prune + 删目录 + 删 lease）。
//!
//! 依赖准备：子代理在 worktree 内按项目现有方式安装依赖、生成构建
//! 产物；宿主不批量复制主工作区被忽略的文件（node_modules/target 等由模型
//! 按需在 worktree 内重建），缺失本地配置由模型在汇报中交回主代理协调。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use git2::{Repository, StatusOptions, WorktreePruneOptions};
use serde::{Deserialize, Serialize};

use crate::safety::safe_join::PathGuard;

/// worktree 存储根（生命周期目录）：`<数据目录>/worktrees/`。
pub fn worktrees_dir(data_root: &Path) -> PathBuf {
    data_root.join("worktrees")
}

/// lease 元数据文件：`<数据目录>/worktrees/<subId>.lease.json`（原子写）。
pub fn lease_path(data_root: &Path, sub_id: &str) -> PathBuf {
    worktrees_dir(data_root).join(format!("{sub_id}.lease.json"))
}

/// worktree 工作区目录：`<数据目录>/worktrees/<subId>/`。
pub fn worktree_root(data_root: &Path, sub_id: &str) -> PathBuf {
    worktrees_dir(data_root).join(sub_id)
}

/// lease 的生命周期状态（恢复索引用；重启后仍可按 lease 找回成果）。
pub const LEASE_ACTIVE: &str = "active";
/// 已成功汇回（含验证）并清理前的终态标注（清理后 lease 文件一并删除）。
pub const LEASE_RECOVERED: &str = "recovered";
/// 失败/取消：保留未汇回 worktree 与修改，标注未完成。
pub const LEASE_ABANDONED: &str = "abandoned";

/// worktree 操作错误（统一字符串面，调用方映射 E_IO 回喂）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeError(pub String);

impl std::fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for WorktreeError {}

fn wt_err(context: impl Into<String>, e: impl std::fmt::Display) -> WorktreeError {
    WorktreeError(format!("{}：{e}", context.into()))
}

/// 派发起点租约（恢复索引）：记录 worktree 位置、基线提交与起点差量
/// 快照（相对 HEAD 的路径 → HEAD blob 指纹；新增文件无基线记空串）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeLease {
    pub version: u32,
    pub sub_id: String,
    pub session_id: String,
    pub role: String,
    /// 项目主目录（canonical 绝对路径）。
    pub project_root: String,
    /// worktree 工作区目录（canonical 绝对路径）。
    pub worktree_root: String,
    /// 基线提交（HEAD 的 40 位 oid hex）。
    pub base_commit: String,
    /// 起点快照：相对项目主目录的路径 → HEAD blob oid hex（起点新增文件为
    /// 空串 = 无基线）。回收时的冲突判定基准：主工作区当前内容 == 起点
    /// 内容 → 直接应用子代理版本；否则冲突保留双方。
    pub snapshot: BTreeMap<String, String>,
    /// 项目主目录相对仓库根的路径前缀（monorepo 场景；顶层项目为空）。
    pub repo_rel_prefix: String,
    pub created_at: i64,
    /// 生命周期状态：active / recovered / abandoned。
    pub status: String,
    /// 终态备注（outcome 四象限或失败原因）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

/// 差量回收报告：applied=直接应用的文件；deleted=子代理删除且
/// 主工作区未动、已同步删除的文件；conflicts=冲突文件清单（子版本已写到
/// 冲突副本路径，主代理负责解决并验证）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub applied: Vec<String>,
    pub deleted: Vec<String>,
    /// (相对路径, 冲突副本绝对路径)。
    pub conflicts: Vec<(String, String)>,
}

/// 项目主目录是否是 git 仓库（含位于某个 git 仓库子目录的情形）。
pub fn project_is_git(project_root: &Path) -> bool {
    Repository::discover(project_root).is_ok()
}

/// 当前时间的 Unix 毫秒时间戳（取不到兜底 0；仅元数据展示用）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 读 lease 元数据（不存在 → None；损坏 → 错误——恢复索引不静默吞配置）。
pub fn load_lease(data_root: &Path, sub_id: &str) -> Result<Option<WorktreeLease>, WorktreeError> {
    let path = lease_path(data_root, sub_id);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| wt_err("读 worktree lease 失败", e))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| wt_err("worktree lease 解析失败", e))
}

/// 写 lease 元数据（原子写）。
pub fn save_lease(lease: &WorktreeLease, data_root: &Path) -> Result<(), WorktreeError> {
    let path = lease_path(data_root, &lease.sub_id);
    let value = serde_json::to_vec_pretty(lease).map_err(|e| wt_err("lease 序列化失败", e))?;
    crate::util::atomic::write_atomic(&path, &value)
        .map_err(|e| wt_err("写 worktree lease 失败", e))
}

/// 列出数据目录下全部 lease（重启后的恢复索引：保留的 worktree 成果据此
/// 可找回；不自动续跑、不自动汇回）。
pub fn list_leases(data_root: &Path) -> Vec<WorktreeLease> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(worktrees_dir(data_root)) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(sub_id) = name.strip_suffix(".lease.json")
            && let Ok(Some(lease)) = load_lease(data_root, sub_id)
        {
            out.push(lease);
        }
    }
    out.sort_by(|a, b| a.sub_id.cmp(&b.sub_id));
    out
}

/// HEAD 基线树里取一个路径的 blob 内容（路径不存在 → None）。
fn head_blob(repo: &Repository, rel_to_repo: &str) -> Result<Option<Vec<u8>>, WorktreeError> {
    let head = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| wt_err("读取仓库 HEAD 失败", e))?;
    let tree = head.tree().map_err(|e| wt_err("读取 HEAD 树失败", e))?;
    let entry = match tree.get_path(Path::new(rel_to_repo)) {
        Ok(entry) => entry,
        Err(_) => return Ok(None),
    };
    let blob = repo
        .find_blob(entry.id())
        .map_err(|e| wt_err("读取基线 blob 失败", e))?;
    Ok(Some(blob.content().to_vec()))
}

/// worktree 的 git 名（subId 前 12 位足够区分且可读）。
fn worktree_name(sub_id: &str) -> String {
    format!(
        "cw-sub-{}",
        sub_id.get(..12).unwrap_or(sub_id).replace('-', "")
    )
}

/// 创建派发快照：返回 lease。失败直接报错——不静默退回旧 HEAD。
///
/// 步骤：HEAD 基线 worktree → checkout HEAD 内容 → 复制主工作区相对 HEAD
/// 的全部差异（含未忽略新文件、含删除差异）→ 起点 blob 指纹快照 → lease
/// 原子落盘。
pub fn create_lease(
    project_root: &Path,
    data_root: &Path,
    sub_id: &str,
    session_id: &str,
    role: &str,
) -> Result<WorktreeLease, WorktreeError> {
    let repo =
        Repository::discover(project_root).map_err(|e| wt_err("项目不是可用的 git 仓库", e))?;
    let repo_workdir = repo
        .workdir()
        .ok_or_else(|| WorktreeError("裸仓库没有工作区，无法创建独立工作区".into()))?
        .to_path_buf();
    let head_commit = repo.head().and_then(|h| h.peel_to_commit()).map_err(|e| {
        WorktreeError(format!(
            "项目仓库没有可用提交（{e}）：无法为独立工作区建立基线；\
                 请先完成一次初始提交再派写入子代理"
        ))
    })?;
    // 项目主目录相对仓库根的前缀（monorepo：statuses 的路径相对仓库根）
    let repo_rel_prefix = project_root
        .strip_prefix(&repo_workdir)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();

    // ① 以 HEAD 建 worktree（元数据 + 工作区目录；目录由 git 创建——预创建
    //    会触发 git_worktree_add 的 Exists 冲突，父目录则先行建好）
    let wt_path = worktree_root(data_root, sub_id);
    std::fs::create_dir_all(worktrees_dir(data_root))
        .map_err(|e| wt_err("创建 worktrees 根目录失败", e))?;
    let name = worktree_name(sub_id);
    // 重名残留防御：同名 worktree 已存在（上次异常残留）→ 先 prune 元数据再建
    if repo.find_worktree(&name).is_ok() {
        prune_worktree(&repo, &name, &wt_path)?;
    }
    repo.worktree(&name, &wt_path, None)
        .map_err(|e| wt_err("git worktree 创建失败", e))?;
    // ② checkout HEAD 内容（git_worktree_add 默认不填充文件，显式补齐基线）
    {
        let wt_repo = Repository::open(&wt_path).map_err(|e| wt_err("打开独立工作区失败", e))?;
        let mut checkout_opts = git2::build::CheckoutBuilder::new();
        checkout_opts.force();
        wt_repo
            .checkout_head(Some(&mut checkout_opts))
            .map_err(|e| wt_err("独立工作区基线检出失败", e))?;
    }

    // ③ 起点差异复制 + blob 指纹快照（statuses 面相对仓库根，套前缀换算为
    //    相对项目主目录的路径）
    let guard = PathGuard::new(vec![project_root.to_path_buf()])
        .map_err(|e| WorktreeError(format!("项目路径不可用：{e}")))?;
    let mut statuses = StatusOptions::new();
    statuses
        .include_untracked(true)
        .include_ignored(false)
        .exclude_submodules(true);
    let status_entries = repo
        .statuses(Some(&mut statuses))
        .map_err(|e| wt_err("读取工作区状态失败", e))?;
    let mut snapshot = BTreeMap::new();
    for entry in status_entries.iter() {
        let Ok(raw_path) = entry.path() else {
            continue;
        };
        let rel = match strip_repo_prefix(raw_path, &repo_rel_prefix) {
            Some(rel) => rel,
            None => continue, // 差异在项目主目录之外：不属于本项目起点
        };
        let current = project_root.join(&rel);
        let status = entry.status();
        // 冲突判定基准 = 派发时主工作区内容的指纹：回收时主工作区
        // 仍等于起点内容 → 直接应用子代理版本；否则视为冲突保留双方。
        // 起点已删除/不存在的文件记空串（基线 = 不存在）。
        let base_hex = if status.is_wt_deleted() || status.is_index_deleted() {
            String::new()
        } else if current.is_file() {
            let bytes =
                std::fs::read(&current).map_err(|e| wt_err(format!("读起点文件 {rel} 失败"), e))?;
            hex_sha256(&bytes)
        } else {
            String::new()
        };
        snapshot.insert(rel.clone(), base_hex);
        if status.is_wt_deleted() || status.is_index_deleted() {
            // 主工作区已删除：worktree 同步删除（基线检出刚写过）
            let _ = std::fs::remove_file(wt_path.join(&rel));
            continue;
        }
        if !current.is_file() {
            continue; // 类型变化等非常规差异：保留基线内容，不复制
        }
        let bytes =
            std::fs::read(&current).map_err(|e| wt_err(format!("读起点文件 {rel} 失败"), e))?;
        write_file_bytes(&wt_path.join(&rel), &bytes)?;
    }

    let lease = WorktreeLease {
        version: 1,
        sub_id: sub_id.to_owned(),
        session_id: session_id.to_owned(),
        role: role.to_owned(),
        project_root: project_root.to_string_lossy().into_owned(),
        worktree_root: wt_path.to_string_lossy().into_owned(),
        base_commit: head_commit.id().to_string(),
        snapshot,
        repo_rel_prefix,
        created_at: now_ms(),
        status: LEASE_ACTIVE.to_owned(),
        outcome: None,
    };
    save_lease(&lease, data_root)?;
    let _ = guard; // PathGuard 仅作 roots 可用性校验（statuses 面天然相对路径）
    Ok(lease)
}

/// 相对仓库根路径剥去项目前缀（差异在项目主目录之外 → None）。
fn strip_repo_prefix(raw: &str, prefix: &str) -> Option<String> {
    if prefix.is_empty() {
        return Some(raw.replace('\\', "/"));
    }
    raw.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(str::to_owned)
}

/// 相对项目主目录路径拼回相对仓库根路径。
fn join_repo_prefix(prefix: &str, rel: &str) -> String {
    if prefix.is_empty() {
        rel.to_owned()
    } else {
        format!("{prefix}/{rel}")
    }
}

/// 差量回收：把 worktree 相对起点的改动汇回主工作区。
/// 冲突保留双方：主工作区文件不动，子代理版本写到 `*.cw-conflict-<sub8>`。
/// 不自动 commit/push、不改暂存状态（直接写工作区文件）。
pub fn recover(lease: &WorktreeLease) -> Result<RecoveryReport, WorktreeError> {
    let wt_path = PathBuf::from(&lease.worktree_root);
    let project_root = PathBuf::from(&lease.project_root);
    let wt_repo = Repository::open(&wt_path).map_err(|e| wt_err("打开独立工作区失败", e))?;
    let guard = PathGuard::new(vec![project_root.clone()])
        .map_err(|e| WorktreeError(format!("项目路径不可用：{e}")))?;
    let mut statuses = StatusOptions::new();
    statuses
        .include_untracked(true)
        .include_ignored(false)
        .exclude_submodules(true);
    let status_entries = wt_repo
        .statuses(Some(&mut statuses))
        .map_err(|e| wt_err("读取独立工作区状态失败", e))?;
    let mut report = RecoveryReport::default();
    let conflict_suffix = format!(
        ".cw-conflict-{}",
        lease.sub_id.get(..8).unwrap_or(&lease.sub_id)
    );
    for entry in status_entries.iter() {
        let Ok(raw_path) = entry.path() else {
            continue;
        };
        let rel = match strip_repo_prefix(raw_path, &lease.repo_rel_prefix) {
            Some(rel) => rel,
            None => continue,
        };
        let status = entry.status();
        let target_in_wt = wt_path.join(&rel);
        // 冲突判定基准：起点内容指纹。快照有记录 → 用快照值（空串 =
        // 起点不存在）；快照无记录（起点相对 HEAD 无差异的文件）→ 基线 =
        // HEAD 内容（worktree 的 HEAD 即基线提交）。
        let rel_to_repo = join_repo_prefix(&lease.repo_rel_prefix, &rel);
        let base_digest: Option<String> = match lease.snapshot.get(&rel).map(String::as_str) {
            Some("") => None, // 起点新增/删除：基线 = 不存在
            Some(hex) => Some(hex.to_owned()),
            None => head_blob(&wt_repo, &rel_to_repo)?.map(|bytes| hex_sha256(&bytes)),
        };
        let current = read_if_exists(&project_root.join(&rel));
        let current_matches_base = match (&current, &base_digest) {
            (None, None) => true,
            (Some(bytes), Some(digest)) => hex_sha256(bytes) == *digest,
            _ => false,
        };
        if status.is_wt_deleted() || (status.is_index_deleted() && !target_in_wt.exists()) {
            // 子代理删除：主工作区仍等于起点内容 → 同步删除；否则冲突保留
            if current.is_none() {
                continue; // 主工作区本来就没有：无事可做
            }
            if current_matches_base {
                match std::fs::remove_file(project_root.join(&rel)) {
                    Ok(()) => report.deleted.push(rel.clone()),
                    Err(e) => return Err(wt_err(format!("删除 {rel} 失败"), e)),
                }
            } else {
                report.conflicts.push((
                    rel.clone(),
                    conflict_copy_path(&project_root, &rel, &conflict_suffix)
                        .to_string_lossy()
                        .into_owned(),
                ));
            }
            continue;
        }
        if !target_in_wt.is_file() {
            continue;
        }
        let sub_bytes = std::fs::read(&target_in_wt)
            .map_err(|e| wt_err(format!("读子工作区 {rel} 失败"), e))?;
        if current_matches_base {
            // 无冲突：直接应用（覆盖或新建；守卫校验路径在项目根内）
            let resolved = guard
                .resolve(&rel)
                .map_err(|e| WorktreeError(format!("回收路径越界 {rel}：{e}")))?;
            write_file_bytes(&resolved, &sub_bytes)?;
            report.applied.push(rel.clone());
        } else {
            // 冲突：主工作区在子代理运行期间被改过 → 保留双方
            let copy_path = conflict_copy_path(&project_root, &rel, &conflict_suffix);
            write_file_bytes(&copy_path, &sub_bytes)?;
            report
                .conflicts
                .push((rel.clone(), copy_path.to_string_lossy().into_owned()));
        }
    }
    Ok(report)
}

/// 冲突副本路径：`<dir>/<file>.cw-conflict-<sub8>[.<ext 顺延>]`。
fn conflict_copy_path(project_root: &Path, rel: &str, suffix: &str) -> PathBuf {
    let p = project_root.join(rel);
    let file = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = p.parent().unwrap_or(project_root);
    let candidate = dir.join(format!("{file}{suffix}"));
    if !candidate.exists() {
        return candidate;
    }
    for n in 1..100u32 {
        let candidate = dir.join(format!("{file}{suffix}.{n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{file}{suffix}.overflow"))
}

/// 主工作区文件内容（不存在 → None）。
fn read_if_exists(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

/// 写文件（逐级建目录；二进制安全）。
fn write_file_bytes(path: &Path, bytes: &[u8]) -> Result<(), WorktreeError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| wt_err("创建目录失败", e))?;
    }
    std::fs::write(path, bytes).map_err(|e| wt_err("写文件失败", e))
}

/// 字节内容的 sha256 前 32 hex（起点指纹比对用；与 edit 版本指纹同算法族）。
fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()[..32]
        .to_owned()
}

/// 清理 worktree（成功汇回验证后）：prune 元数据（含未提交修改须先删目录）、
/// 删工作区目录、删 lease 文件。
pub fn cleanup(lease: &WorktreeLease, data_root: &Path) -> Result<(), WorktreeError> {
    let wt_path = PathBuf::from(&lease.worktree_root);
    // 1) 删除工作区目录（含未提交修改——清理即放弃现场）
    if wt_path.exists() {
        std::fs::remove_dir_all(&wt_path).map_err(|e| wt_err("删除独立工作区目录失败", e))?;
    }
    // 2) prune 主仓库的 worktree 元数据（目录已删，working_tree 标志不再需要）
    if let Ok(repo) = Repository::discover(Path::new(&lease.project_root)) {
        let name = worktree_name(&lease.sub_id);
        if let Err(e) = prune_worktree(&repo, &name, &wt_path) {
            #[cfg(debug_assertions)]
            eprintln!("[craftwave] worktree prune 失败（{}）：{e}", lease.sub_id);
            #[cfg(not(debug_assertions))]
            let _ = e;
        }
    }
    // 3) 删 lease（成果已汇回，恢复索引不再保留）
    let _ = std::fs::remove_file(lease_path(data_root, &lease.sub_id));
    Ok(())
}

/// prune 一个 worktree（元数据残留清理；容错——find_worktree 未命中直接成功）。
fn prune_worktree(repo: &Repository, name: &str, wt_path: &Path) -> Result<(), WorktreeError> {
    let Ok(wt) = repo.find_worktree(name) else {
        return Ok(());
    };
    let mut opts = WorktreePruneOptions::new();
    opts.working_tree(true).valid(true).locked(true);
    match wt.is_prunable(Some(&mut opts)) {
        Ok(true) => {
            wt.prune(Some(&mut opts))
                .map_err(|e| wt_err(format!("prune worktree {} 失败", wt_path.display()), e))?;
        }
        Ok(false) => {}
        Err(_) => {} // prunable 判定失败（元数据已失效等）：跳过 prune
    }
    Ok(())
}

/// 把 lease 标注为指定状态并回写（失败/取消 → abandoned 保留现场；成功
/// 汇回 → recovered 后随即 cleanup 删除）。
pub fn mark_status(
    lease: &mut WorktreeLease,
    data_root: &Path,
    status: &str,
    outcome: Option<String>,
) -> Result<(), WorktreeError> {
    lease.status = status.to_owned();
    lease.outcome = outcome;
    save_lease(lease, data_root)
}
