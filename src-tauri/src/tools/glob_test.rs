//! glob 工具测试：递归模式、gitignore 生效、500 截断、起点越界拒、
//! 非法 pattern 拒绝、排序与 kind 标注（票面测试计划逐项）。

use serde_json::json;

use super::{GlobArgs, MAX_ENTRIES, run};
use crate::util::test_support::TestWorkspace;

/// 测试辅助：构造参数（起点缺省项目根）。
fn args(pattern: &str) -> GlobArgs {
    args_at(pattern, None)
}

/// 测试辅助：构造带起点的参数。
fn args_at(pattern: &str, path: Option<&str>) -> GlobArgs {
    GlobArgs {
        pattern: pattern.to_string(),
        path: path.map(str::to_string),
    }
}

/// 测试辅助：在工作区建目录（路径以 / 结尾）或文件。
fn mk(ws: &TestWorkspace, rel: &str) {
    let p = ws.project_file(rel);
    if rel.ends_with('/') {
        std::fs::create_dir_all(p).expect("建目录");
    } else {
        std::fs::create_dir_all(p.parent().expect("parent")).expect("建父目录");
        std::fs::write(p, "x").expect("写文件");
    }
}

/// 从结果里取 path 数组。
fn paths_of(outcome: &crate::tools::ToolOutcome) -> Vec<String> {
    outcome.data.as_ref().expect("data")["paths"]
        .as_array()
        .expect("paths")
        .iter()
        .map(|p| p["path"].as_str().expect("str").to_string())
        .collect()
}

// 断言：`**/*.rs` 递归命中任意深度；`*.rs`（literal_separator=false 的
// globset 默认语义）同样跨段命中；结果按路径排序，kind 区分文件/目录。
#[test]
fn recursive_patterns_match_across_depth_and_sort_results() {
    let ws = TestWorkspace::new();
    mk(&ws, "root.rs");
    mk(&ws, "src/main.rs");
    mk(&ws, "src/deep/util.rs");
    mk(&ws, "src/");

    let outcome = run(&ws.ctx(), &args("**/*.rs"));
    assert!(outcome.ok, "{outcome:?}");
    let paths = paths_of(&outcome);
    assert_eq!(paths, vec!["root.rs", "src/deep/util.rs", "src/main.rs"]);

    // 短模式跨段命中同一批文件。
    let outcome = run(&ws.ctx(), &args("*.rs"));
    assert_eq!(paths_of(&outcome), paths);

    // kind：目录条目标 dir（`src/**` 匹配 src 下内容含目录本身）。
    let outcome = run(&ws.ctx(), &args("src/**"));
    assert!(outcome.ok, "{outcome:?}");
    let kinds = outcome.data.expect("data")["paths"]
        .as_array()
        .expect("paths")
        .iter()
        .map(|p| p["kind"].as_str().expect("str").to_string())
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"dir".to_string()));
    assert!(kinds.contains(&"file".to_string()));
    assert!(outcome.ui_summary.contains("命中"));
}

// 断言：.gitignore 生效（被忽略的文件不出现在结果里），隐藏文件不列出；
// 非 git 项目同样应用 .gitignore（与 list_files 同源 ignore 栈）。
#[test]
fn respects_gitignore_and_hidden_files() {
    let ws = TestWorkspace::new();
    mk(&ws, "src/keep.rs");
    mk(&ws, "src/skip.rs");
    mk(&ws, ".hidden.rs");
    mk(&ws, ".git/inner.rs");
    std::fs::write(ws.project_file(".gitignore"), "skip.rs\n").expect("gitignore");

    let outcome = run(&ws.ctx(), &args("**/*.rs"));
    assert!(outcome.ok, "{outcome:?}");
    let paths = paths_of(&outcome);
    assert!(paths.contains(&"src/keep.rs".to_string()));
    assert!(
        !paths.contains(&"src/skip.rs".to_string()),
        "gitignore 应生效"
    );
    assert!(!paths.contains(&".hidden.rs".to_string()), "隐藏文件不列出");
    assert!(!paths.iter().any(|p| p.contains(".git/")), ".git 目录跳过");
}

// 断言：命中数达 500 上限截断并标注（构造 501 个命中文件）。
#[test]
fn truncates_at_500_entries() {
    let ws = TestWorkspace::new();
    for i in 0..MAX_ENTRIES + 1 {
        mk(&ws, &format!("gen/f{i}.log"));
    }
    let outcome = run(&ws.ctx(), &args("gen/*.log"));
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["count"], json!(MAX_ENTRIES));
    assert_eq!(data["truncated"], json!(true));
    assert_eq!(data["paths"].as_array().expect("paths").len(), MAX_ENTRIES);
    assert!(outcome.ui_summary.contains("截断"));
}

// 断言：起点越界（roots 之外）拒绝；起点不是目录拒绝；空/非法 pattern 拒绝。
#[test]
fn rejects_out_of_roots_and_bad_inputs() {
    let ws = TestWorkspace::new();

    let outcome = run(&ws.ctx(), &args_at("**/*", Some("../outside")));
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("error").code, "E_PATH_OUTSIDE_ROOTS");

    // 起点是文件不是目录。
    mk(&ws, "a.txt");
    let outcome = run(&ws.ctx(), &args_at("*.rs", Some("a.txt")));
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("error").code, "E_INVALID_ARG");

    // 空 pattern 与非法 pattern（未闭合的字符组）。
    for bad in ["", "  ", "["] {
        let outcome = run(&ws.ctx(), &args(bad));
        assert!(!outcome.ok, "pattern {bad:?} 应拒绝");
        assert_eq!(outcome.error.expect("error").code, "E_INVALID_ARG");
    }
}

// 断言：无命中时成功返回空列表（count=0、truncated=false），不算失败。
#[test]
fn empty_hits_are_ok() {
    let ws = TestWorkspace::new();
    mk(&ws, "readme.md");
    let outcome = run(&ws.ctx(), &args("**/*.rs"));
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["count"], json!(0));
    assert_eq!(data["truncated"], json!(false));
    assert_eq!(data["paths"], json!([]));
}
