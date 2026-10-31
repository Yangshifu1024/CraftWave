//! grep 工具测试：命中行、glob 过滤、结果上限截断、正则无效拒绝、
//! gitignore 与二进制跳过（越界拒绝见 mod.rs 共享守卫测试）。

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

// 断言：grep 检索矩阵——命中带 "文件:行内容" 前缀且 count/truncated 正确、
// glob 过滤只搜匹配文件、显式 maxResults=10 与缺省 100 两档截断生效、
// 非法正则拒绝、.gitignore 与二进制文件不出现在命中里。
#[test]
fn match_filter_cap_and_ignore_matrix() {
    // —— 命中 + 前缀 + count ——
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

    // —— glob 过滤 ——
    let mut a = args("TODO");
    a.glob = Some("*.rs".to_string());
    let outcome = run(&ws.ctx(), &a);
    let data = outcome.data.expect("data");
    let joined = serde_json::to_string(&data["matches"]).expect("json");
    assert!(joined.contains("b.rs"));
    assert!(!joined.contains("readme.md"), "glob 应过滤 md");

    // —— 显式 maxResults=10 截断 ——
    let body: String = (0..50).map(|i| format!("hit {i}\n")).collect();
    write(&ws, "many.txt", &body);
    let mut a = args("hit");
    a.max_results = Some(10);
    let outcome = run(&ws.ctx(), &a);
    let data = outcome.data.expect("data");
    assert_eq!(data["count"], json!(10));
    assert_eq!(data["truncated"], json!(true));
    assert!(outcome.ui_summary.contains("截断"));

    // —— 缺省上限 100 ——
    let body: String = (0..150).map(|i| format!("line {i}\n")).collect();
    write(&ws, "many2.txt", &body);
    let outcome = run(&ws.ctx(), &args("line"));
    let data = outcome.data.expect("data");
    assert_eq!(data["count"], json!(100), "默认上限 100");
    assert_eq!(data["truncated"], json!(true));

    // —— 非法正则拒绝 ——
    let outcome = run(&ws.ctx(), &args("(unclosed"));
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_INVALID_ARG");

    // —— gitignore 与二进制跳过 ——
    write(&ws, "kept.txt", "needle visible\n");
    write(&ws, "ignored.log", "needle hidden\n");
    std::fs::write(ws.project_file(".gitignore"), "*.log\n").expect("gitignore");
    write(&ws, "bin.dat", "needle\x00hidden\n");
    let outcome = run(&ws.ctx(), &args("needle"));
    let data = outcome.data.expect("data");
    let joined = serde_json::to_string(&data["matches"]).expect("json");
    assert!(joined.contains("kept.txt"));
    assert!(!joined.contains("ignored.log"));
    assert!(!joined.contains("bin.dat"));
}
