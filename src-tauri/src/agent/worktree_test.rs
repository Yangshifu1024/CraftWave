//! worktree 管理测试（10 号票，ADR-0001 / §5.9 Q9–Q12）：派发起点快照
//! （含已暂存/未暂存修改与未忽略新文件）、主工作区变化不自动同步、差量
//! 回收（新增/修改/删除）、冲突保留双方、不自动 commit、不改暂存状态、
//! 生命周期保留（abandoned 跨重启索引）与成功清理。

use super::*;
use crate::agent::worktree::{cleanup, mark_status, recover};
use std::path::Path;

use crate::util::test_support::TestWorkspace;

/// git 提交签名（测试固定值）。
fn test_signature(repo: &git2::Repository) -> git2::Signature<'static> {
    repo.signature()
        .unwrap_or_else(|_| git2::Signature::now("t", "t@t").expect("固定签名"))
}

/// 初始化临时 git 仓库：init + 首提交（README.md = "base\n"）。
fn init_git_repo(path: &Path) -> git2::Repository {
    let repo = git2::Repository::init(path).expect("init 仓库");
    let mut config = repo.config().expect("repo config");
    config.set_str("user.name", "tester").ok();
    config.set_str("user.email", "tester@test").ok();
    std::fs::write(path.join("README.md"), "base\n").expect("写 README");
    commit_all(&repo, "init");
    repo
}

/// 把工作区全部变更提交到 HEAD（测试辅助；生产路径绝不提交）。
fn commit_all(repo: &git2::Repository, message: &str) {
    let mut index = repo.index().expect("index");
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .expect("add all");
    index.write().expect("index write");
    let tree_id = index.write_tree().expect("tree");
    let tree = repo.find_tree(tree_id).expect("find tree");
    let sig = test_signature(repo);
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .expect("commit");
}

/// 主仓库当前 HEAD 指纹（不自动 commit 的断言基准）。
fn head_id(repo: &git2::Repository) -> String {
    repo.head()
        .expect("HEAD")
        .peel_to_commit()
        .expect("commit")
        .id()
        .to_string()
}

/// index 中某路径的 blob oid（「不改暂存状态」的精确断言基准：暂存面 =
/// index 内容，回收只写工作区文件、绝不触碰 index）。
fn index_blob_id(repo: &git2::Repository, path: &str) -> String {
    let index = repo.index().expect("index");
    let entry = index.get_path(Path::new(path), 0).expect("index entry");
    entry.id.to_string()
}

/// 数据目录替身（lease/worktree 落点；TempDir 保活）。
struct WtFixture {
    workspace: TestWorkspace,
}

impl WtFixture {
    fn new() -> Self {
        Self {
            workspace: TestWorkspace::new(),
        }
    }

    fn project_root(&self) -> std::path::PathBuf {
        self.workspace.project.path().to_path_buf()
    }

    fn data_root(&self) -> std::path::PathBuf {
        self.workspace.data.path().to_path_buf()
    }
}

// 断言（派发起点快照，Q9/Q12）：快照含已暂存+未暂存修改与未跟踪新文件
// （基线 = 派发时主工作区内容指纹，worktree 内容与主工作区当前文件状态
// 逐字节一致，基线 commit = 派发时 HEAD）；被忽略文件不进快照不进
// worktree；派发后主工作区变化不自动同步；非 git 目录判定为非 git（写入
// 子代理据此拒绝修改）；空仓库（无提交）创建快照直接报错不静默回退旧 HEAD。
#[test]
fn dispatch_snapshot_semantics() {
    // 快照含未提交修改与未跟踪文件，worktree 内容一致。
    let fx = WtFixture::new();
    let root = fx.project_root();
    let repo = init_git_repo(&root);
    // 已暂存修改（README 的 index 版本先进）、未暂存修改、未跟踪新文件
    std::fs::write(root.join("README.md"), "staged change\n").unwrap();
    {
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("README.md")).unwrap();
        index.write().unwrap();
    }
    std::fs::write(root.join("README.md"), "staged+unstaged\n").unwrap();
    std::fs::write(root.join("notes.txt"), "untracked note\n").unwrap();

    let lease = create_lease(&root, &fx.data_root(), "sub-1", "sess-1", "implementer")
        .expect("创建派发快照");
    // 快照记录了两个差异文件；基线 = 派发时主工作区内容指纹（冲突判定基准）
    assert_eq!(
        lease.snapshot.len(),
        2,
        "快照应记录 README + notes：{:?}",
        lease.snapshot
    );
    let fingerprint = |content: &str| {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(content.as_bytes());
        h.finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()[..32]
            .to_owned()
    };
    assert_eq!(
        lease.snapshot["README.md"],
        fingerprint("staged+unstaged\n"),
        "README 基线 = 派发时工作区内容（含未暂存部分）"
    );
    assert_eq!(
        lease.snapshot["notes.txt"],
        fingerprint("untracked note\n"),
        "未跟踪新文件基线 = 派发时内容"
    );

    // worktree 内容 = 主工作区当前状态（未提交修改与新文件都进来了）。
    let wt = std::path::PathBuf::from(&lease.worktree_root);
    assert_eq!(
        std::fs::read_to_string(wt.join("README.md")).unwrap(),
        "staged+unstaged\n",
        "worktree 应含已暂存+未暂存修改的当前内容"
    );
    assert_eq!(
        std::fs::read_to_string(wt.join("notes.txt")).unwrap(),
        "untracked note\n",
        "worktree 应含未被忽略的新文件"
    );
    assert_eq!(
        std::fs::read_to_string(wt.join(".gitignore")).unwrap_or_default(),
        std::fs::read_to_string(root.join(".gitignore")).unwrap_or_default(),
    );
    assert_eq!(lease.base_commit, head_id(&repo), "基线 = 派发时 HEAD");

    // 被忽略文件不进快照（Q12：不批量复制主工作区忽略文件）。
    let fx = WtFixture::new();
    let root = fx.project_root();
    let repo = init_git_repo(&root);
    std::fs::write(root.join(".gitignore"), "ignored-dir/\n").unwrap();
    commit_all(&repo, "add gitignore");
    std::fs::create_dir_all(root.join("ignored-dir")).unwrap();
    std::fs::write(root.join("ignored-dir").join("deps.bin"), "junk").unwrap();
    let lease =
        create_lease(&root, &fx.data_root(), "sub-2", "sess-1", "implementer").expect("创建快照");
    let wt = std::path::PathBuf::from(&lease.worktree_root);
    assert!(
        !wt.join("ignored-dir").join("deps.bin").exists(),
        "被忽略文件不得复制进 worktree（依赖按项目现有方式在 worktree 内重建）"
    );
    assert!(lease.snapshot.is_empty(), "忽略文件不进起点快照");

    // 派发后主工作区变化不自动同步到 worktree（Q9：后续不自动同步）。
    let fx = WtFixture::new();
    let root = fx.project_root();
    init_git_repo(&root);
    let lease = create_lease(&root, &fx.data_root(), "sub-3", "sess-1", "implementer").unwrap();
    std::fs::write(root.join("README.md"), "user edit after dispatch\n").unwrap();
    let wt = std::path::PathBuf::from(&lease.worktree_root);
    assert_eq!(
        std::fs::read_to_string(wt.join("README.md")).unwrap(),
        "base\n",
        "worktree 保持派发起点状态，不自动同步主工作区后续变化"
    );

    // 非 git 项目判定：非 git 目录返回 false（写入子代理据此拒绝修改）。
    let fx = WtFixture::new();
    assert!(
        !project_is_git(&fx.project_root()),
        "临时项目目录默认不是 git 仓库"
    );
    let root = fx.project_root();
    init_git_repo(&root);
    assert!(project_is_git(&root), "init 后判定为 git 仓库");

    // 快照创建失败不静默退回旧 HEAD（Q9）：空仓库（无提交）直接报错。
    let fx = WtFixture::new();
    let root = fx.project_root();
    git2::Repository::init(&root).expect("init 空仓库（无任何提交）");
    let err = create_lease(&root, &fx.data_root(), "sub-9", "sess-1", "implementer")
        .expect_err("无提交仓库必须失败，不得静默回退");
    assert!(err.0.contains("提交"), "错误信息说明原因：{err}");
}

// 断言（差量回收与生命周期，Q10/Q11）：worktree 的新增/修改/删除按文件
// 汇回主工作区；创建与回收都不自动 commit、不改用户已有暂存状态（回收只
// 写工作区文件，index 内容原样，相对 index 呈现未暂存修改）；回收标注后
// 清理成功（worktree 目录与 lease 一并消失）；失败/取消的 worktree 保留
// 未汇回修改并标注 abandoned，重启（重新 list_leases）仍可找回、不自动
// 汇回主工作区，事后人工回收有价部分。
#[test]
fn recover_applies_add_modify_delete_and_lifecycle() {
    let fx = WtFixture::new();
    let root = fx.project_root();
    let repo = init_git_repo(&root);
    std::fs::write(root.join("legacy.txt"), "to be deleted\n").unwrap();
    commit_all(&repo, "add legacy");
    // 用户有一个已暂存的修改（「不改暂存状态」的断言基准）。
    std::fs::write(root.join("README.md"), "user staged\n").unwrap();
    {
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("README.md")).unwrap();
        index.write().unwrap();
    }
    let before_head = head_id(&repo);
    let before_index = index_blob_id(&repo, "README.md");

    let mut lease = create_lease(&root, &fx.data_root(), "sub-4", "sess-1", "implementer").unwrap();
    // 子代理在 worktree 内：改 README、加 feature.rs、删 legacy.txt。
    let wt = std::path::PathBuf::from(&lease.worktree_root);
    std::fs::write(wt.join("README.md"), "base + subagent edit\n").unwrap();
    std::fs::write(wt.join("feature.rs"), "fn main() {}\n").unwrap();
    std::fs::remove_file(wt.join("legacy.txt")).unwrap();

    let report = recover(&lease).expect("回收成功");
    assert_eq!(report.conflicts.len(), 0, "无冲突场景");
    assert!(
        report.applied.contains(&"README.md".to_owned())
            && report.applied.contains(&"feature.rs".to_owned()),
        "修改与新增被回收：{:?}",
        report.applied
    );
    assert!(report.deleted.contains(&"legacy.txt".to_owned()));
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "base + subagent edit\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("feature.rs")).unwrap(),
        "fn main() {}\n"
    );
    assert!(!root.join("legacy.txt").exists(), "删除被同步");
    // 不自动 commit、不改暂存状态（用户只暂存未改工作区时 = 无冲突）。
    assert_eq!(head_id(&repo), before_head, "创建/回收不自动 commit");
    assert_eq!(
        index_blob_id(&repo, "README.md"),
        before_index,
        "用户暂存状态不被回收改变（回收只写工作区文件，index 内容原样）"
    );
    // 工作区文件 = 子代理版本（回收应用的结果），相对 index 呈现未暂存修改。
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true).include_ignored(false);
    let statuses: std::collections::BTreeSet<String> = repo
        .statuses(Some(&mut opts))
        .expect("statuses")
        .iter()
        .map(|e| format!("{:?}:{}", e.status(), e.path().unwrap_or("?")))
        .collect();
    assert!(
        statuses
            .iter()
            .any(|s| s.contains("README.md") && s.contains("WT_MODIFIED")),
        "回收应用后工作区内容为子代理版本（index 面未动，呈现为未暂存修改）"
    );
    // 生命周期：回收标注后清理成功（worktree 目录与 lease 一并消失）。
    mark_status(&mut lease, &fx.data_root(), LEASE_RECOVERED, None).unwrap();
    cleanup(&lease, &fx.data_root()).unwrap();
    assert!(!std::path::PathBuf::from(&lease.worktree_root).exists());
    assert!(load_lease(&fx.data_root(), "sub-4").unwrap().is_none());
    assert_eq!(head_id(&repo), before_head, "清理也不 commit");

    // 生命周期（Q11）：失败/取消的 worktree 保留未汇回修改并标注未完成；
    // 重启（重新 list_leases）仍可找回，且不自动汇回主工作区。
    let fx = WtFixture::new();
    let root = fx.project_root();
    init_git_repo(&root);
    let mut lease = create_lease(&root, &fx.data_root(), "sub-8", "sess-1", "implementer").unwrap();
    let wt = std::path::PathBuf::from(&lease.worktree_root);
    std::fs::write(wt.join("unfinished.txt"), "precious work\n").unwrap();
    // 取消 → 标注 abandoned（不汇回）。
    mark_status(
        &mut lease,
        &fx.data_root(),
        LEASE_ABANDONED,
        Some("cancelled".into()),
    )
    .unwrap();
    // 模拟重启：重新扫描恢复索引。
    let leases = list_leases(&fx.data_root());
    assert_eq!(leases.len(), 1, "重启后仍可找回保留的成果索引");
    assert_eq!(leases[0].status, LEASE_ABANDONED);
    assert_eq!(leases[0].sub_id, "sub-8");
    assert!(
        std::fs::read_to_string(wt.join("unfinished.txt")).unwrap() == "precious work\n",
        "未汇回修改完整保留"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "base\n",
        "abandoned 成果不自动汇入主工作区"
    );
    // 事后人工回收（主代理检查后回收有价部分的路径）。
    let report = recover(&leases[0]).unwrap();
    assert!(report.applied.contains(&"unfinished.txt".to_owned()));
    assert_eq!(
        std::fs::read_to_string(root.join("unfinished.txt")).unwrap(),
        "precious work\n"
    );
    let lease_after = list_leases(&fx.data_root()).into_iter().next().unwrap();
    cleanup(&lease_after, &fx.data_root()).unwrap();
}

// 断言（冲突保留双方，Q10）：主工作区在子代理运行期间被改过 → 主文件
// 不动，子版本写到带 `.cw-conflict-` 标记的冲突副本，未冲突文件正常回收；
// 主工作区在子代理删除同名文件期间被用户改过 → 冲突保留（不执行删除）。
#[test]
fn recover_conflicts_keep_both_versions() {
    // 修改冲突：主文件保留用户修改，子版本进冲突副本。
    let fx = WtFixture::new();
    let root = fx.project_root();
    init_git_repo(&root);
    let lease = create_lease(&root, &fx.data_root(), "sub-5", "sess-1", "implementer").unwrap();
    // 主工作区（用户）先改 README。
    std::fs::write(root.join("README.md"), "user concurrent edit\n").unwrap();
    // 子代理随后也改 README（相对起点 = 冲突）。
    let wt = std::path::PathBuf::from(&lease.worktree_root);
    std::fs::write(wt.join("README.md"), "subagent edit\n").unwrap();
    // 另一个文件无冲突地新增。
    std::fs::write(wt.join("clean.txt"), "no conflict\n").unwrap();

    let report = recover(&lease).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "user concurrent edit\n",
        "冲突时保留用户已有修改（主文件不动）"
    );
    assert_eq!(report.conflicts.len(), 1, "README 冲突");
    let (conflict_path_rel, copy_path) = &report.conflicts[0];
    assert_eq!(conflict_path_rel, "README.md");
    assert!(
        copy_path.contains(".cw-conflict-"),
        "冲突副本路径带子代理标记：{copy_path}"
    );
    assert_eq!(
        std::fs::read_to_string(copy_path).unwrap(),
        "subagent edit\n",
        "子代理版本完整保留在冲突副本（双方内容都在）"
    );
    assert!(
        report.applied.contains(&"clean.txt".to_owned()),
        "无冲突文件正常应用"
    );

    // 删除冲突：用户改了文件、子代理删了它 → 冲突保留（用户版本不动）。
    let fx = WtFixture::new();
    let root = fx.project_root();
    let repo = init_git_repo(&root);
    std::fs::write(root.join("target.txt"), "origin\n").unwrap();
    commit_all(&repo, "add target");
    let lease = create_lease(&root, &fx.data_root(), "sub-6", "sess-1", "implementer").unwrap();
    std::fs::write(root.join("target.txt"), "user edit\n").unwrap();
    std::fs::remove_file(std::path::PathBuf::from(&lease.worktree_root).join("target.txt"))
        .unwrap();
    let report = recover(&lease).unwrap();
    assert!(
        report.conflicts.iter().any(|(p, _)| p == "target.txt"),
        "删除遇用户修改 → 冲突保留：{:?}",
        report.conflicts
    );
    assert_eq!(
        std::fs::read_to_string(root.join("target.txt")).unwrap(),
        "user edit\n",
        "用户版本不动"
    );
}
