//! grep 工具测试：命中行、glob 过滤、maxResults 截断、正则无效拒绝、gitignore。

use serde_json::json;

use super::{GrepArgs, run};
use crate::util::test_support::TestWorkspace;

/// 测试辅助：构造默认参数（全项目搜索、无过滤）的 grep 参数。
fn args(pattern: &str) -> GrepArgs {
    GrepArgs {
        pattern: pattern.to_string(),
        path: None,
        glob: None,
        max_results: None,
    }
}

/// 测试辅助：在工作区写入一个文件（自动建父目录）。
fn write(ws: &TestWorkspace, rel: &str, content: &str) {
    let p = ws.project_file(rel);
    std::fs::create_dir_all(p.parent().expect("parent")).expect("父目录");
    std::fs::write(p, content).expect("写文件");
}

// 断言：命中结果带 "文件:行内容" 前缀，count/truncated 字段正确。
#[test]
fn finds_matches_with_file_prefix() {
    let ws = TestWorkspace::new();
    write(&ws, "src/a.rs", "fn main() {}\nlet todo_count = 1;\n");
    write(&ws, "src/b.rs", "// TODO: fix\nfn other() {}\n");
    write(&ws, "docs/readme.md", "TODO in docs\n");

    let outcome = run(&ws.ctx(), &args("TODO"));
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    let matches = data["matches"].as_array().expect("matches");
    assert_eq!(matches.len(), 2, "应命中 b.rs 与 readme.md");
    let joined = serde_json::to_string(matches).expect("json");
    assert!(joined.contains("src/b.rs:// TODO: fix"));
    assert!(joined.contains("docs/readme.md:TODO in docs"));
    assert_eq!(data["count"], json!(2));
    assert_eq!(data["truncated"], json!(false));
}

// 断言：glob 过滤只搜索匹配文件名的文件。
#[test]
fn glob_filter_limits_files() {
    let ws = TestWorkspace::new();
    write(&ws, "src/a.rs", "TODO here\n");
    write(&ws, "docs/b.md", "TODO there\n");
    let mut a = args("TODO");
    a.glob = Some("*.rs".to_string());
    let outcome = run(&ws.ctx(), &a);
    let data = outcome.data.expect("data");
    let joined = serde_json::to_string(&data["matches"]).expect("json");
    assert!(joined.contains("a.rs"));
    assert!(!joined.contains("b.md"), "glob 应过滤 md");
}

// 断言：显式 maxResults=10 时命中到 10 条即截断并标记 truncated。
#[test]
fn max_results_truncates() {
    let ws = TestWorkspace::new();
    let body: String = (0..50).map(|i| format!("hit {i}\n")).collect();
    write(&ws, "many.txt", &body);
    let mut a = args("hit");
    a.max_results = Some(10);
    let outcome = run(&ws.ctx(), &a);
    let data = outcome.data.expect("data");
    assert_eq!(data["count"], json!(10));
    assert_eq!(data["truncated"], json!(true));
    assert!(outcome.ui_summary.contains("截断"));
}

// 断言：不传 maxResults 时缺省上限 100 生效。
#[test]
fn default_cap_applies_without_max_results() {
    let ws = TestWorkspace::new();
    let body: String = (0..150).map(|i| format!("line {i}\n")).collect();
    write(&ws, "many.txt", &body);
    let outcome = run(&ws.ctx(), &args("line"));
    let data = outcome.data.expect("data");
    assert_eq!(data["count"], json!(100), "默认上限 100");
    assert_eq!(data["truncated"], json!(true));
}

// 断言：非法正则以 E_INVALID_ARG 拒绝。
#[test]
fn invalid_regex_is_invalid_arg() {
    let ws = TestWorkspace::new();
    let outcome = run(&ws.ctx(), &args("(unclosed"));
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_INVALID_ARG");
}

// 断言：.gitignore 忽略的文件与二进制文件（含 NUL）不出现在命中里。
#[test]
fn gitignore_and_binary_respected() {
    let ws = TestWorkspace::new();
    write(&ws, "kept.txt", "needle visible\n");
    write(&ws, "ignored.log", "needle hidden\n");
    std::fs::write(ws.project_file(".gitignore"), "*.log\n").expect("gitignore");
    // 二进制文件（含 NUL）跳过。
    write(&ws, "bin.dat", "needle\x00hidden\n");

    let outcome = run(&ws.ctx(), &args("needle"));
    let data = outcome.data.expect("data");
    let joined = serde_json::to_string(&data["matches"]).expect("json");
    assert!(joined.contains("kept.txt"));
    assert!(!joined.contains("ignored.log"));
    assert!(!joined.contains("bin.dat"));
}

// 断言：搜索起点指向 roots 外（/etc）被拒绝。
#[test]
fn outside_roots_rejected() {
    let ws = TestWorkspace::new();
    let mut a = args("x");
    a.path = Some("/etc".to_string());
    let outcome = run(&ws.ctx(), &a);
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_PATH_OUTSIDE_ROOTS");
}
