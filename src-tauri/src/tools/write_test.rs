//! write 工具测试：新建成功 UTF-8 落盘、已存在拒绝、嵌套目录创建、越界拒绝。

use serde_json::json;

use super::{WriteArgs, run};
use crate::util::test_support::TestWorkspace;

fn args(path: &str, content: &str) -> WriteArgs {
    WriteArgs {
        path: path.to_string(),
        content: content.to_string(),
    }
}

#[test]
fn creates_new_file_with_utf8() {
    let ws = TestWorkspace::new();
    let outcome = run(&ws.ctx(), &args("notes/新文件.md", "# 标题\n中文内容\n"));
    assert!(outcome.ok, "{outcome:?}");
    let bytes = std::fs::read(ws.project_file("notes/新文件.md")).expect("读回");
    // UTF-8 严格校验通过即证明落盘为 UTF-8。
    std::str::from_utf8(&bytes).expect("应为 UTF-8");
    assert_eq!(bytes, "# 标题\n中文内容\n".as_bytes());
    let data = outcome.data.expect("data");
    assert_eq!(data["bytesWritten"], json!(bytes.len()));
    assert!(outcome.ui_summary.contains("新建文件"));
}

#[test]
fn existing_file_is_rejected() {
    let ws = TestWorkspace::new();
    std::fs::write(ws.project_file("a.txt"), "old").expect("预置");
    let outcome = run(&ws.ctx(), &args("a.txt", "new"));
    assert!(!outcome.ok);
    let err = outcome.error.expect("error");
    assert_eq!(err.code, "E_INVALID_ARG");
    assert!(err.message.contains("edit"), "应引导用 edit");
    // 原文件未被破坏。
    assert_eq!(
        std::fs::read(ws.project_file("a.txt")).expect("读回"),
        b"old"
    );
}

#[test]
fn outside_roots_is_rejected() {
    let ws = TestWorkspace::new();
    let outcome = run(&ws.ctx(), &args("/tmp/cw-evil.txt", "x"));
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_PATH_OUTSIDE_ROOTS");
}
