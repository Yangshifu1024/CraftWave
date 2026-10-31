//! git 只读查询测试（26 号票）：tempfile 临时仓库构造（绝不触碰本仓库
//! 真实 .git），覆盖票面测试清单——status 各状态（修改/新增/删除/untracked/
//! ignore 尊重）、diff hunk 正确性（含二进制标记、CRLF）、log 排序与 limit、
//! 非仓库空结果、.git 异常容错、user info。

use super::{
    STATUS_ADDED, STATUS_CONFLICTED, STATUS_DELETED, STATUS_MODIFIED, STATUS_UNTRACKED, diff_core,
    discover_repo, log_core, status_core,
};
use crate::git::DiffLineKind;
use std::path::Path;

/// git 提交签名（测试固定值）。
fn test_signature(repo: &git2::Repository) -> git2::Signature<'static> {
    repo.signature()
        .unwrap_or_else(|_| git2::Signature::now("t", "t@t").expect("固定签名"))
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

/// 初始化临时仓库：init + 配置身份 + 首提交（README.md 与 a.txt）。
fn init_git_repo(path: &Path) -> git2::Repository {
    let repo = git2::Repository::init(path).expect("init 仓库");
    let mut config = repo.config().expect("repo config");
    config.set_str("user.name", "tester").ok();
    config.set_str("user.email", "tester@test").ok();
    std::fs::write(path.join("README.md"), "base\n").expect("写 README");
    std::fs::write(path.join("a.txt"), "line1\nline2\n").expect("写 a.txt");
    commit_all(&repo, "init");
    repo
}

/// status 结果里找某路径的状态值。
fn status_of<'a>(result: &'a super::GitStatusResult, path: &str) -> &'a str {
    result
        .files
        .iter()
        .find(|f| f.path == path)
        .map(|f| f.status.as_str())
        .unwrap_or_else(|| panic!("状态列表缺 {path}：{:?}", result.files))
}

// 断言（状态矩阵与降级路径）：真实仓库 status 各状态——工作区修改（M）、
// 删除（D）、未跟踪（untracked）、已暂存新文件（A）、.gitignore 尊重、
// user info 回读（FR-11-01）；主状态归一位标志表（冲突 > 删除 > 新增 >
// 修改 > 未跟踪，冲突态纯函数面核对）；非仓库 isRepo=false 空结果；空
// 仓库（无 HEAD）untracked 照常；写坏 HEAD 的脏仓库降级空结果不报错
// （status/log/diff 三面）。
#[test]
fn status_matrix_and_degraded_paths() {
    // 真实仓库各状态 + user info。
    let dir = tempfile::tempdir().expect("临时目录");
    init_git_repo(dir.path());
    // 工作区修改（M）
    std::fs::write(dir.path().join("a.txt"), "line1\nline2-changed\n").expect("改 a.txt");
    // 工作区删除（D）
    std::fs::remove_file(dir.path().join("README.md")).expect("删 README");
    // 未跟踪（untracked）
    std::fs::write(dir.path().join("new.txt"), "new\n").expect("写 new.txt");
    // 已暂存新文件（A）
    std::fs::write(dir.path().join("staged.txt"), "staged\n").expect("写 staged.txt");
    let repo = discover_repo(dir.path()).expect("仓库");
    let mut index = repo.index().expect("index");
    index.add_path(Path::new("staged.txt")).expect("add");
    index.write().expect("index write");
    drop(index);
    // 被忽略文件（.gitignore 尊重：不出现）
    std::fs::write(dir.path().join(".gitignore"), "ignored.txt\n").expect("写 ignore");
    std::fs::write(dir.path().join("ignored.txt"), "secret\n").expect("写 ignored");

    let result = status_core(dir.path());
    assert!(result.is_repo, "是仓库");
    assert_eq!(status_of(&result, "a.txt"), STATUS_MODIFIED);
    assert_eq!(status_of(&result, "README.md"), STATUS_DELETED);
    assert_eq!(status_of(&result, "new.txt"), STATUS_UNTRACKED);
    assert_eq!(status_of(&result, "staged.txt"), STATUS_ADDED);
    assert!(
        !result.files.iter().any(|f| f.path == "ignored.txt"),
        "忽略文件不进状态列表：{:?}",
        result.files
    );
    // user info（FR-11-01）：仓库配置身份回读
    let user = result.user.as_ref().expect("user 应有值");
    assert_eq!(user.name.as_deref(), Some("tester"));
    assert_eq!(user.email.as_deref(), Some("tester@test"));

    // 主状态归一的位标志表驱动（冲突 > 删除 > 新增 > 修改 > 未跟踪）；
    // 冲突态的仓库级构造依赖合并拓扑，纯函数面直接以位标志核对。
    assert_eq!(
        super::main_status(git2::Status::CONFLICTED),
        STATUS_CONFLICTED
    );
    assert_eq!(super::main_status(git2::Status::WT_DELETED), STATUS_DELETED);
    assert_eq!(
        super::main_status(git2::Status::INDEX_DELETED),
        STATUS_DELETED
    );
    // 已暂存新文件（含暂存后又有工作区改动）→ A
    assert_eq!(super::main_status(git2::Status::INDEX_NEW), STATUS_ADDED);
    assert_eq!(
        super::main_status(git2::Status::INDEX_NEW | git2::Status::WT_MODIFIED),
        STATUS_ADDED
    );
    // 未跟踪 → untracked
    assert_eq!(super::main_status(git2::Status::WT_NEW), STATUS_UNTRACKED);
    // 修改（工作区/暂存）→ M
    assert_eq!(
        super::main_status(git2::Status::WT_MODIFIED),
        STATUS_MODIFIED
    );
    assert_eq!(
        super::main_status(git2::Status::INDEX_MODIFIED),
        STATUS_MODIFIED
    );

    // 非仓库：isRepo=false + 空结果。
    let dir = tempfile::tempdir().expect("临时目录");
    let result = status_core(dir.path());
    assert!(!result.is_repo, "非仓库 isRepo=false");
    assert!(result.files.is_empty());
    assert!(result.user.is_none());

    // 空仓库（无 HEAD）：untracked 照常。
    let dir = tempfile::tempdir().expect("临时目录");
    git2::Repository::init(dir.path()).expect("init");
    std::fs::write(dir.path().join("only.txt"), "only\n").expect("写");
    let result = status_core(dir.path());
    assert!(result.is_repo);
    assert_eq!(status_of(&result, "only.txt"), STATUS_UNTRACKED);

    // 脏仓库容错：写坏 HEAD（引用格式非法）→ 打开成功但操作失败 → 降级
    // 空结果不报错（status/log/diff 三面）。
    let dir = tempfile::tempdir().expect("临时目录");
    init_git_repo(dir.path());
    std::fs::write(dir.path().join(".git").join("HEAD"), "not a valid ref\n").expect("写坏 HEAD");
    let status = status_core(dir.path());
    assert!(status.is_repo, "仓库打开成功 isRepo=true");
    assert!(
        status.files.is_empty(),
        "脏仓库降级空列表：{:?}",
        status.files
    );
    let log = log_core(dir.path(), None);
    assert!(log.is_repo);
    assert!(log.commits.is_empty(), "脏仓库降级空提交列表");
    let diff = diff_core(dir.path(), None);
    assert!(diff.is_repo);
    assert!(diff.patches.is_empty(), "脏仓库降级空补丁");
}

// 断言（diff 面）：修改一行 + 新增一行的 hunk 行数与 header（@@ 形态）、
// 行种类（删/增/上下文）与删除行文本（CRLF/换行已剥离）；untracked 文件
// 计入差异（全行为新增）；二进制文件（NUL 字节）binary 标记 + 无内容行；
// CRLF 文件行文本不带 \r；空仓库 diff 含 untracked 文件（全行为新增）且
// log 空列表；staged 参数本票不支持（票面边界）——传 true/false 返回同一
// 聚合视图。
#[test]
fn diff_hunks_binary_crlf_and_edge_cases() {
    // 修改一行 + 新增一行 + untracked 新文件。
    let dir = tempfile::tempdir().expect("临时目录");
    init_git_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "line1\nline2-changed\nline3\n").expect("改 a.txt");
    std::fs::write(dir.path().join("b.txt"), "brand new\n").expect("写 b.txt");

    let result = diff_core(dir.path(), None);
    assert!(result.is_repo);
    let a = result
        .patches
        .iter()
        .find(|p| p.path == "a.txt")
        .expect("a.txt 补丁");
    assert_eq!(a.add_lines, 2, "增行（line2-changed + line3）：{:?}", a);
    assert_eq!(a.del_lines, 1, "删行（line2）：{:?}", a);
    assert!(!a.binary);
    // hunk header 形如 @@ -1,2 +1,3 @@
    assert!(
        a.hunks.iter().any(|h| h.header.starts_with("@@")),
        "header 应为 @@ 形态：{:?}",
        a.hunks
    );
    let kinds: Vec<_> = a.hunks[0].lines.iter().map(|l| l.kind).collect();
    assert!(kinds.contains(&DiffLineKind::Del), "有删除行");
    assert!(kinds.contains(&DiffLineKind::Add), "有新增行");
    assert!(kinds.contains(&DiffLineKind::Ctx), "有上下文行");
    let del_text = a.hunks[0]
        .lines
        .iter()
        .find(|l| l.kind == DiffLineKind::Del)
        .map(|l| l.text.clone())
        .unwrap_or_default();
    assert_eq!(del_text, "line2", "删除行文本（CRLF/换行已剥离）");
    // untracked 文件计入差异（全行为新增）
    let b = result
        .patches
        .iter()
        .find(|p| p.path == "b.txt")
        .expect("b.txt 补丁");
    assert_eq!(b.add_lines, 1);
    assert_eq!(b.del_lines, 0);

    // 二进制文件：bytes 含 NUL → binary 标记；CRLF 文件：行文本不带 \r。
    let dir = tempfile::tempdir().expect("临时目录");
    init_git_repo(dir.path());
    let bytes: Vec<u8> = vec![0x00, 0xff, 0xfe, 0x01, 0x02];
    std::fs::write(dir.path().join("blob.bin"), &bytes).expect("写二进制");
    std::fs::write(dir.path().join("crlf.txt"), b"first\r\nsecond\r\n").expect("写 CRLF");
    let result = diff_core(dir.path(), None);
    let bin = result
        .patches
        .iter()
        .find(|p| p.path == "blob.bin")
        .expect("二进制补丁");
    assert!(bin.binary, "二进制标记：{:?}", bin);
    let crlf = result
        .patches
        .iter()
        .find(|p| p.path == "crlf.txt")
        .expect("CRLF 补丁");
    assert!(!crlf.binary);
    for line in crlf.hunks.iter().flat_map(|h| &h.lines) {
        assert!(
            !line.text.ends_with('\r'),
            "行文本不带 \\r：{:?}",
            line.text
        );
        assert!(!line.text.ends_with('\n'), "行文本不带 \\n");
    }
    assert_eq!(crlf.add_lines, 2, "CRLF 两行均新增");

    // 空仓库 diff：untracked 文件全行为新增；log 空仓库 → 空列表（无 HEAD）。
    let dir = tempfile::tempdir().expect("临时目录");
    git2::Repository::init(dir.path()).expect("init");
    std::fs::write(dir.path().join("only.txt"), "only\n").expect("写");
    let result = diff_core(dir.path(), None);
    assert!(result.is_repo);
    let only = result
        .patches
        .iter()
        .find(|p| p.path == "only.txt")
        .expect("空仓库 diff 应含 untracked 文件");
    assert_eq!(only.add_lines, 1);
    assert_eq!(only.del_lines, 0);
    let log = log_core(dir.path(), None);
    assert!(log.is_repo);
    assert!(log.commits.is_empty());

    // staged 参数本票不支持（票面边界）：传 true/false 返回同一聚合视图。
    let dir = tempfile::tempdir().expect("临时目录");
    init_git_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "changed\n").expect("改");
    let with_staged = diff_core(dir.path(), Some(true));
    let without = diff_core(dir.path(), None);
    assert_eq!(with_staged, without, "staged 参数忽略——两视图一致");
}

// 断言（log 面）：缺省全量、最新在前、sha 全量 40 位；limit 生效；超上限
// clamp 到 100（结构有效不 panic）。
#[test]
fn log_order_and_limit() {
    let dir = tempfile::tempdir().expect("临时目录");
    let repo = init_git_repo(dir.path());
    std::fs::write(dir.path().join("a.txt"), "v2\n").expect("改 a.txt");
    commit_all(&repo, "second commit");
    std::fs::write(dir.path().join("a.txt"), "v3\n").expect("再改 a.txt");
    commit_all(&repo, "third commit");

    let result = log_core(dir.path(), None);
    assert!(result.is_repo);
    assert_eq!(result.commits.len(), 3, "缺省全量 3 条");
    assert_eq!(result.commits[0].summary, "third commit", "最新在前");
    assert_eq!(result.commits[2].summary, "init");
    assert_eq!(result.commits[0].sha.len(), 40, "sha 全量 40 位");
    // limit 生效 + clamp
    let limited = log_core(dir.path(), Some(1));
    assert_eq!(limited.commits.len(), 1);
    assert_eq!(limited.commits[0].summary, "third commit");
    // 超上限 clamp 到 100（不 panic 即可，结构有效）
    let clamped = log_core(dir.path(), Some(usize::MAX));
    assert_eq!(clamped.commits.len(), 3);
}
