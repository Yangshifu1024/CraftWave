//! write 工具测试：新建成功 UTF-8 落盘、已存在拒绝（越界拒绝见 mod.rs
//! 的共享守卫测试 path_guard_test）。

use serde_json::json;

use super::{WriteArgs, run};
use crate::util::test_support::TestWorkspace;

/// 测试辅助：构造 write 参数。
fn args(path: &str, content: &str) -> WriteArgs {
    WriteArgs {
        path: path.to_string(),
        content: content.to_string(),
    }
}

// 断言：新建成功（嵌套父目录自动创建、UTF-8 落盘、字节数正确）；
// 目标已存在时拒绝并引导用 edit，原文件内容不被破坏。
#[test]
fn create_new_and_reject_existing_matrix() {
    let ws = TestWorkspace::new();

    // 正常路径：新建 + 嵌套目录 + UTF-8 落盘。
    let outcome = run(&ws.ctx(), &args("notes/新文件.md", "# 标题\n中文内容\n"));
    assert!(outcome.ok, "{outcome:?}");
    let bytes = std::fs::read(ws.project_file("notes/新文件.md")).expect("读回");
    // UTF-8 严格校验通过即证明落盘为 UTF-8。
    std::str::from_utf8(&bytes).expect("应为 UTF-8");
    assert_eq!(bytes, "# 标题\n中文内容\n".as_bytes());
    let data = outcome.data.expect("data");
    assert_eq!(data["bytesWritten"], json!(bytes.len()));
    assert!(outcome.ui_summary.contains("新建文件"));

    // 已存在：拒绝并引导用 edit，原文件不被破坏。
    std::fs::write(ws.project_file("a.txt"), "old").expect("预置");
    let outcome = run(&ws.ctx(), &args("a.txt", "new"));
    assert!(!outcome.ok);
    let err = outcome.error.expect("error");
    assert_eq!(err.code, "E_INVALID_ARG");
    assert!(err.message.contains("edit"), "应引导用 edit");
    assert_eq!(
        std::fs::read(ws.project_file("a.txt")).expect("读回"),
        b"old"
    );
}
