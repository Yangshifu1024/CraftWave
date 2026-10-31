//! safe_join 测试：正常 join、`..` 越界、绝对路径越界、symlink 逃逸、
//! 不存在路径（write 场景）、控制字符、Windows 保留名。

use std::path::PathBuf;

use super::PathGuard;
use crate::util::errors::ToolError;

/// 测试辅助：临时目录作为唯一 root 构造 PathGuard（返回临时目录以便保活）。
fn setup() -> (tempfile::TempDir, PathGuard) {
    let dir = tempfile::tempdir().expect("临时目录");
    let guard = PathGuard::new(vec![dir.path().to_path_buf()]).expect("guard");
    (dir, guard)
}

/// 测试辅助：取错误的错误码字符串。
fn code_of(err: &ToolError) -> &'static str {
    err.code
}

/// 测试辅助：guard 的首个 root（canonical 后的项目主目录，resolve(".") 取得）。
fn guard_roots_first(guard: &PathGuard) -> PathBuf {
    guard.resolve(".").expect("root 本身必然合法")
}

// 断言：界内路径放行——相对路径解析到 root 内；root 内已存在的绝对路径
// 通过且返回 canonicalize 后的路径；write 场景下文件与父目录都不存在时
// 仍能解析出 root 内的绝对路径；多 root 场景首个 root 是相对路径基准、
// 其余 root 也放行。
#[test]
fn resolve_accepts_paths_inside_roots() {
    let (_dir, guard) = setup();
    let p = guard.resolve("src/main.rs").expect("应通过");
    assert!(p.starts_with(guard_roots_first(&guard)));

    let (dir, guard) = setup();
    std::fs::write(dir.path().join("a.txt"), "x").expect("写文件");
    let abs = dir.path().join("a.txt");
    let p = guard.resolve(abs.to_str().expect("utf8")).expect("应通过");
    assert_eq!(
        std::fs::canonicalize(&abs).expect("存在"),
        p,
        "已存在路径应 canonicalize"
    );
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

    // 多 root：相对路径相对首个 root（项目主目录），数据目录是第二 root。
    let proj = tempfile::tempdir().expect("项目目录");
    let data = tempfile::tempdir().expect("数据目录");
    let guard =
        PathGuard::new(vec![proj.path().to_path_buf(), data.path().to_path_buf()]).expect("guard");
    let p = guard.resolve("x.txt").expect("项目内");
    assert!(p.starts_with(std::fs::canonicalize(proj.path()).expect("c")));
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

// 断言：越界与非法路径拒绝——相对路径 `..` 逃出 root、root 之外的绝对
// 路径一律 E_PATH_OUTSIDE_ROOTS；经 root 内 symlink 指向外部目录的逃逸
// 同拦（canonicalize 后做前缀检查，仅 Unix 面）；空路径与含控制字符
// （NUL、换行）以 E_INVALID_ARG 拒绝；Windows 保留名（含带扩展名形态）
// 跨平台拒绝，仅前缀相似的合法名字（concourse）不受影响。
#[test]
fn resolve_rejects_escapes_and_invalid_args() {
    let (_dir, guard) = setup();
    let err = guard.resolve("a/../../etc/passwd").unwrap_err();
    assert_eq!(code_of(&err), "E_PATH_OUTSIDE_ROOTS");
    let err = guard.resolve("/etc/passwd").unwrap_err();
    assert_eq!(code_of(&err), "E_PATH_OUTSIDE_ROOTS");
    let err = guard.resolve("/usr/local/bin").unwrap_err();
    assert_eq!(code_of(&err), "E_PATH_OUTSIDE_ROOTS");

    // symlink 逃逸（符号链接创建用 std::os::unix API，仅 Unix 跑）。
    #[cfg(unix)]
    {
        let (dir, guard) = setup();
        // root/link → /tmp（root 外）。经 link 的相对逃逸必须被拦。
        let outside = PathBuf::from("/tmp");
        std::os::unix::fs::symlink(&outside, dir.path().join("link")).expect("建 symlink");
        let err = guard.resolve("link/../etc").unwrap_err();
        assert_eq!(code_of(&err), "E_PATH_OUTSIDE_ROOTS");
        // 直接指向外面的 symlink 文件本身也越界。
        let err = guard.resolve("link").unwrap_err();
        assert_eq!(code_of(&err), "E_PATH_OUTSIDE_ROOTS");
    }

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
    for name in ["CON", "nul.txt", "COM1", "aux", "LPT9.cfg"] {
        let err = guard.resolve(name).unwrap_err();
        assert_eq!(code_of(&err), "E_INVALID_ARG", "{name} 应被拒");
    }
    // 非 stem 命中不算：concourse 目录是合法名字。
    guard.resolve("concourse/data").expect("concourse 合法");
}
