//! glob 工具测试：递归模式、gitignore 生效、500 截断、非法 pattern 拒绝、
//! 排序与 kind 标注（越界拒绝见 mod.rs 共享守卫测试）。

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

// 断言：glob 检索矩阵——`**/*.rs` 递归跨段命中且按路径排序、`*.rs` 短模式
// 同语义、kind 区分文件/目录；.gitignore 与隐藏文件/.git 目录生效；无命中
// 成功返回空列表；500 上限截断；空/非法 pattern 与文件起点拒绝。
#[test]
fn pattern_match_filter_cap_and_rejections_matrix() {
    // —— 递归匹配 + 排序 + 短模式 + kind ——
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

    // —— gitignore / 隐藏文件 / .git 目录 ——
    mk(&ws, "src/skip.rs");
    mk(&ws, ".hidden.rs");
    mk(&ws, ".git/inner.rs");
    std::fs::write(ws.project_file(".gitignore"), "skip.rs\n").expect("gitignore");
    let outcome = run(&ws.ctx(), &args("**/*.rs"));
    assert!(outcome.ok, "{outcome:?}");
    let paths = paths_of(&outcome);
    assert!(paths.contains(&"src/main.rs".to_string()));
    assert!(
        !paths.contains(&"src/skip.rs".to_string()),
        "gitignore 应生效"
    );
    assert!(!paths.contains(&".hidden.rs".to_string()), "隐藏文件不列出");
    assert!(!paths.iter().any(|p| p.contains(".git/")), ".git 目录跳过");

    // —— 无命中：成功返回空列表，不算失败 ——
    let outcome = run(&ws.ctx(), &args("**/*.zzz"));
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["count"], json!(0));
    assert_eq!(data["truncated"], json!(false));
    assert_eq!(data["paths"], json!([]));

    // —— 500 上限截断 ——
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

    // —— 非法输入拒绝：空/空白/未闭合字符组 pattern、文件起点 ——
    for bad in ["", "  ", "["] {
        let outcome = run(&ws.ctx(), &args(bad));
        assert!(!outcome.ok, "pattern {bad:?} 应拒绝");
        assert_eq!(outcome.error.expect("error").code, "E_INVALID_ARG");
    }
    mk(&ws, "a.txt");
    let outcome = run(&ws.ctx(), &args_at("*.rs", Some("a.txt")));
    assert!(!outcome.ok, "文件起点应拒绝");
    assert_eq!(outcome.error.expect("error").code, "E_INVALID_ARG");
}
