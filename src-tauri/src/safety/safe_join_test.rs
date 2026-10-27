//! safe_join 测试：正常 join、`..` 越界、绝对路径越界、symlink 逃逸、
//! 不存在路径（write 场景）、控制字符、Windows 保留名。

use std::path::PathBuf;

use super::PathGuard;
use crate::util::errors::ToolError;

fn setup() -> (tempfile::TempDir, PathGuard) {
    let dir = tempfile::tempdir().expect("临时目录");
    let guard = PathGuard::new(vec![dir.path().to_path_buf()]).expect("guard");
    (dir, guard)
}

fn code_of(err: &ToolError) -> &'static str {
    err.code
}

#[test]
fn relative_path_resolves_inside_root() {
    let (_dir, guard) = setup();
    let p = guard.resolve("src/main.rs").expect("应通过");
    assert!(p.starts_with(guard_roots_first(&guard)));
}

#[test]
fn absolute_path_inside_root_passes() {
    let (dir, guard) = setup();
    std::fs::write(dir.path().join("a.txt"), "x").expect("写文件");
    let abs = dir.path().join("a.txt");
    let p = guard.resolve(abs.to_str().expect("utf8")).expect("应通过");
    assert_eq!(
        std::fs::canonicalize(&abs).expect("存在"),
        p,
        "已存在路径应 canonicalize"
    );
}

#[test]
fn parent_escape_is_rejected() {
    let (_dir, guard) = setup();
    let err = guard.resolve("a/../../etc/passwd").unwrap_err();
    assert_eq!(code_of(&err), "E_PATH_OUTSIDE_ROOTS");
}

#[test]
fn absolute_outside_root_is_rejected() {
    let (_dir, guard) = setup();
    let err = guard.resolve("/etc/passwd").unwrap_err();
    assert_eq!(code_of(&err), "E_PATH_OUTSIDE_ROOTS");
    let err = guard.resolve("/usr/local/bin").unwrap_err();
    assert_eq!(code_of(&err), "E_PATH_OUTSIDE_ROOTS");
}

#[test]
fn symlink_escape_is_rejected() {
    let (dir, guard) = setup();
    // root/link → /tmp（root 外）。经 link 的相对逃逸必须被拦。
    #[cfg(target_os = "macos")]
    let outside = PathBuf::from("/tmp");
    #[cfg(not(target_os = "macos"))]
    let outside = PathBuf::from("/tmp");
    std::os::unix::fs::symlink(&outside, dir.path().join("link")).expect("建 symlink");
    let err = guard.resolve("link/../etc").unwrap_err();
    assert_eq!(code_of(&err), "E_PATH_OUTSIDE_ROOTS");
    // 直接指向外面的 symlink 文件本身也越界。
    let err = guard.resolve("link").unwrap_err();
    assert_eq!(code_of(&err), "E_PATH_OUTSIDE_ROOTS");
}

#[test]
fn nonexistent_path_canonically_resolves_for_write() {
    let (dir, guard) = setup();
    // write 新建场景：文件与父目录都不存在，仍须解析并通过前缀检查。
    let p = guard
        .resolve("deep/nested/dir/new-file.txt")
        .expect("不存在路径也应通过");
    assert_eq!(
        p,
        std::fs::canonicalize(dir.path())
            .expect("root")
            .join("deep/nested/dir/new-file.txt")
    );
}

#[test]
fn empty_and_control_chars_are_rejected() {
    let (_dir, guard) = setup();
    assert_eq!(code_of(&guard.resolve("").unwrap_err()), "E_INVALID_ARG");
    assert_eq!(
        code_of(&guard.resolve("a\0b").unwrap_err()),
        "E_INVALID_ARG"
    );
    assert_eq!(
        code_of(&guard.resolve("a\nb").unwrap_err()),
        "E_INVALID_ARG"
    );
}

#[test]
fn windows_reserved_names_are_rejected_cross_platform() {
    let (_dir, guard) = setup();
    for name in ["CON", "nul.txt", "COM1", "aux", "LPT9.cfg"] {
        let err = guard.resolve(name).unwrap_err();
        assert_eq!(code_of(&err), "E_INVALID_ARG", "{name} 应被拒");
    }
    // 非 stem 命中不算：concourse 目录是合法名字。
    guard.resolve("concourse/data").expect("concourse 合法");
}

#[test]
fn multi_root_second_root_is_honored() {
    let proj = tempfile::tempdir().expect("项目目录");
    let data = tempfile::tempdir().expect("数据目录");
    let guard =
        PathGuard::new(vec![proj.path().to_path_buf(), data.path().to_path_buf()]).expect("guard");
    // 相对路径相对首个 root（项目主目录）。
    let p = guard.resolve("x.txt").expect("项目内");
    assert!(p.starts_with(std::fs::canonicalize(proj.path()).expect("c")));
    // 数据目录是第二 root。
    let abs = data.path().join("sessions/abc.jsonl");
    let p = guard
        .resolve(abs.to_str().expect("utf8"))
        .expect("数据目录内");
    assert!(p.starts_with(std::fs::canonicalize(data.path()).expect("c")));
    // 两者之外仍拒绝。
    assert_eq!(
        code_of(&guard.resolve("/var/log/system.log").unwrap_err()),
        "E_PATH_OUTSIDE_ROOTS"
    );
}

fn guard_roots_first(guard: &PathGuard) -> PathBuf {
    // 测试辅助：guard 的首个 root（canonical 后的项目主目录）。
    // resolve(""): 用一次越界错误拿不到 root，直接 canonicalize 临时目录
    // 不可得——改为通过 resolve(".") 取得。
    guard.resolve(".").expect("root 本身必然合法")
}
