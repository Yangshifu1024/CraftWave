//! list_files 工具测试：树形清单、500 上限、respectGitignore、depth、非目录拒绝。

use serde_json::json;

use super::{ListFilesArgs, MAX_ENTRIES, run};
use crate::util::test_support::TestWorkspace;

fn args(path: Option<&str>) -> ListFilesArgs {
    ListFilesArgs {
        path: path.map(str::to_string),
        depth: None,
        respect_gitignore: None,
    }
}

fn mk(ws: &TestWorkspace, rel: &str) {
    let p = ws.project_file(rel);
    if rel.ends_with('/') {
        std::fs::create_dir_all(p).expect("建目录");
    } else {
        std::fs::create_dir_all(p.parent().expect("parent")).expect("建父目录");
        std::fs::write(p, "x").expect("写文件");
    }
}

#[test]
fn lists_tree_with_gitignore_respected_by_default() {
    let ws = TestWorkspace::new();
    mk(&ws, "src/main.rs");
    mk(&ws, "src/deep/util.rs");
    mk(&ws, "target/generated.rs");
    mk(&ws, ".hidden");
    std::fs::write(ws.project_file(".gitignore"), "/target\n").expect("gitignore");

    let outcome = run(&ws.ctx(), &args(None));
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    let entries = data["entries"].as_array().expect("entries");
    let joined = serde_json::to_string(entries).expect("json");
    assert!(joined.contains("main.rs"));
    assert!(joined.contains("util.rs"));
    assert!(!joined.contains("generated.rs"), "gitignore 应被尊重");
    assert!(!joined.contains(".hidden"), "隐藏文件不列出");
    assert_eq!(data["truncated"], json!(false));
    // 目录带 "/" 后缀、缩进树。
    assert!(entries.iter().any(|e| e.as_str().expect("str") == "src/"));
    let util = entries
        .iter()
        .find(|e| e.as_str().expect("str").contains("util.rs"))
        .expect("util.rs");
    assert!(
        util.as_str().expect("str").starts_with("    "),
        "深层缩进两空格/层"
    );
}

#[test]
fn respect_gitignore_false_includes_ignored() {
    let ws = TestWorkspace::new();
    mk(&ws, "target/generated.rs");
    std::fs::write(ws.project_file(".gitignore"), "/target\n").expect("gitignore");
    let mut a = args(None);
    a.respect_gitignore = Some(false);
    let outcome = run(&ws.ctx(), &a);
    let data = outcome.data.expect("data");
    assert!(
        serde_json::to_string(&data["entries"])
            .expect("json")
            .contains("generated.rs")
    );
}

#[test]
fn entry_cap_at_500_with_truncated_flag() {
    let ws = TestWorkspace::new();
    for i in 0..(MAX_ENTRIES + 50) {
        mk(&ws, &format!("f{i:04}.txt"));
    }
    let outcome = run(&ws.ctx(), &args(None));
    let data = outcome.data.expect("data");
    assert_eq!(data["count"], json!(MAX_ENTRIES));
    assert_eq!(data["truncated"], json!(true));
    assert!(outcome.ui_summary.contains("截断"));
}

#[test]
fn depth_limits_levels() {
    let ws = TestWorkspace::new();
    mk(&ws, "a/b/c/deep.txt");
    mk(&ws, "a/top.txt");
    let mut a = args(None);
    a.depth = Some(2); // root 不计：a/ 与 a/top.txt 在深度 2 内，b/ 及以下不计
    let outcome = run(&ws.ctx(), &a);
    let data = outcome.data.expect("data");
    let joined = serde_json::to_string(&data["entries"]).expect("json");
    assert!(joined.contains("a/"));
    assert!(joined.contains("top.txt"));
    assert!(!joined.contains("deep.txt"));
}

#[test]
fn subdirectory_and_non_directory() {
    let ws = TestWorkspace::new();
    mk(&ws, "sub/inner.txt");
    let outcome = run(&ws.ctx(), &args(Some("sub")));
    let data = outcome.data.expect("data");
    assert_eq!(data["root"], json!("sub"));
    assert!(
        serde_json::to_string(&data["entries"])
            .expect("json")
            .contains("inner.txt")
    );

    let outcome = run(&ws.ctx(), &args(Some("sub/inner.txt")));
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_INVALID_ARG");
}
