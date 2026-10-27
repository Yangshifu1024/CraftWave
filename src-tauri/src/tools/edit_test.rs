//! edit 工具测试：三级匹配、唯一性拒绝、version 乐观锁、EOL 保持、
//! lineChanges 统计、GBK 文件转 UTF-8、越界拒绝。

use serde_json::json;

use super::{EditArgs, run};
use crate::util::test_support::TestWorkspace;

fn args(path: &str, old: &str, new: &str) -> EditArgs {
    EditArgs {
        path: path.to_string(),
        old_text: old.to_string(),
        new_text: new.to_string(),
        version: None,
    }
}

fn write_file(ws: &TestWorkspace, rel: &str, content: &str) {
    std::fs::write(ws.project_file(rel), content).expect("预置文件");
}

#[test]
fn l1_exact_unique_replacement() {
    let ws = TestWorkspace::new();
    write_file(&ws, "a.rs", "fn main() {\n    println!(\"hi\");\n}\n");
    let outcome = run(
        &ws.ctx(),
        &args("a.rs", "println!(\"hi\")", "println!(\"hello\")"),
    );
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["matchLevel"], "exact");
    assert_eq!(data["lineChanges"], json!({ "add": 1, "del": 1 }));
    assert_eq!(
        std::fs::read_to_string(ws.project_file("a.rs")).expect("读回"),
        "fn main() {\n    println!(\"hello\");\n}\n"
    );
}

#[test]
fn l2_indent_shift_recovers_nested_block() {
    let ws = TestWorkspace::new();
    // 模型给的 oldText 无缩进，文件里是深缩进块。
    write_file(
        &ws,
        "b.rs",
        "function f() {\n        doWork();\n        return true;\n    }\n",
    );
    let outcome = run(
        &ws.ctx(),
        &args(
            "b.rs",
            "doWork();\nreturn true;",
            "doWork(1);\nreturn false;",
        ),
    );
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["matchLevel"], "indent-shift");
    // §8.4 语义：缩进平移只用于「匹配」；替换区间含原首行缩进，
    // newText 原样落位（缩进由模型自带）。
    assert_eq!(
        std::fs::read_to_string(ws.project_file("b.rs")).expect("读回"),
        "function f() {\ndoWork(1);\nreturn false;\n    }\n",
    );
}

#[test]
fn l3_whitespace_normalized_matches() {
    let ws = TestWorkspace::new();
    // 两行缩进不一致 + 尾随空白：L1 不中、L2 统一缩进无法表达，落到 L3。
    write_file(&ws, "c.txt", "doA();   \n\t\tdoB();\n");
    let outcome = run(
        &ws.ctx(),
        &args("c.txt", "doA();\ndoB();", "doA(1);\ndoB(2);"),
    );
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["matchLevel"], "whitespace-normalized");
    assert_eq!(
        std::fs::read_to_string(ws.project_file("c.txt")).expect("读回"),
        "doA(1);\ndoB(2);\n"
    );
}

#[test]
fn no_match_and_ambiguous_are_rejected() {
    let ws = TestWorkspace::new();
    write_file(&ws, "d.txt", "one\ntwo\n");
    let outcome = run(&ws.ctx(), &args("d.txt", "three", "3"));
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_EDIT_NO_MATCH");

    write_file(&ws, "e.txt", "dup\ndup\n");
    let outcome = run(&ws.ctx(), &args("e.txt", "dup", "unique"));
    assert!(!outcome.ok);
    let err = outcome.error.expect("e");
    assert_eq!(err.code, "E_EDIT_AMBIGUOUS");
    assert!(err.message.contains("补充"));
}

#[test]
fn empty_old_text_is_invalid_arg() {
    let ws = TestWorkspace::new();
    write_file(&ws, "f.txt", "x\n");
    let outcome = run(&ws.ctx(), &args("f.txt", "", "y"));
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_INVALID_ARG");
}

#[test]
fn version_match_and_stale_downgrade() {
    let ws = TestWorkspace::new();
    write_file(&ws, "g.txt", "hello world\n");
    let bytes = std::fs::read(ws.project_file("g.txt")).expect("读");
    let good = super::version_hex(&bytes);

    // 版本匹配：正常执行。
    let mut a = args("g.txt", "hello", "hi");
    a.version = Some(good.clone());
    let outcome = run(&ws.ctx(), &a);
    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(
        std::fs::read_to_string(ws.project_file("g.txt")).expect("读回"),
        "hi world\n"
    );
    assert!(outcome.data.expect("data").get("warning").is_none());

    // 版本不匹配但仍可唯一匹配 → 降级警告执行。
    write_file(&ws, "h.txt", "hello world\n");
    let mut a = args("h.txt", "world", "rust");
    a.version = Some("deadbeefdeadbeef".to_string());
    let outcome = run(&ws.ctx(), &a);
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert!(data["warning"].as_str().expect("warn").contains("降级"));
    assert!(outcome.ui_summary.contains("降级"));
    assert_eq!(
        std::fs::read_to_string(ws.project_file("h.txt")).expect("读回"),
        "hello rust\n"
    );

    // 版本不匹配且不可唯一匹配 → E_EDIT_VERSION_STALE。
    write_file(&ws, "i.txt", "dup\ndup\n");
    let mut a = args("i.txt", "dup", "x");
    a.version = Some("deadbeefdeadbeef".to_string());
    let outcome = run(&ws.ctx(), &a);
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_EDIT_VERSION_STALE");
}

#[test]
fn crlf_style_is_preserved() {
    let ws = TestWorkspace::new();
    write_file(&ws, "win.txt", "line1\r\nline2\r\n");
    let outcome = run(&ws.ctx(), &args("win.txt", "line2", "line-two"));
    assert!(outcome.ok, "{outcome:?}");
    let bytes = std::fs::read(ws.project_file("win.txt")).expect("读回");
    assert_eq!(bytes, b"line1\r\nline-two\r\n", "CRLF 应保持");
}

#[test]
fn gbk_file_is_rewritten_as_utf8() {
    let ws = TestWorkspace::new();
    let (gbk, _, _) = encoding_rs::GBK.encode("旧值：alpha\n重复行\n");
    std::fs::write(ws.project_file("legacy.ini"), &gbk).expect("写 GBK");
    let outcome = run(&ws.ctx(), &args("legacy.ini", "alpha", "beta"));
    assert!(outcome.ok, "{outcome:?}");
    let bytes = std::fs::read(ws.project_file("legacy.ini")).expect("读回");
    let text = std::str::from_utf8(&bytes).expect("编辑后应为 UTF-8（C-10 副作用）");
    assert_eq!(text, "旧值：beta\n重复行\n");
}

#[test]
fn line_changes_counts_add_and_del() {
    let ws = TestWorkspace::new();
    // 单行扩为三行：+3/-1（口径：lines().count()）。
    write_file(&ws, "j.txt", "start\nend\n");
    let outcome = run(&ws.ctx(), &args("j.txt", "start", "a\nb\nc"));
    let data = outcome.data.expect("data");
    assert_eq!(data["lineChanges"], json!({ "add": 3, "del": 1 }));

    // 两行删为一行。
    write_file(&ws, "k.txt", "x\ny\n");
    let outcome = run(&ws.ctx(), &args("k.txt", "x\ny", "z"));
    let data = outcome.data.expect("data");
    assert_eq!(data["lineChanges"], json!({ "add": 1, "del": 2 }));

    // 整块删除：newText 空。
    write_file(&ws, "l.txt", "keep\ndrop\n");
    let outcome = run(&ws.ctx(), &args("l.txt", "keep\ndrop\n", ""));
    assert!(outcome.ok, "{outcome:?}");
    assert_eq!(
        std::fs::read_to_string(ws.project_file("l.txt")).expect("读回"),
        ""
    );
    let data = outcome.data.expect("data");
    assert_eq!(data["lineChanges"], json!({ "add": 0, "del": 2 }));
}

#[test]
fn outside_roots_rejected() {
    let ws = TestWorkspace::new();
    let outcome = run(&ws.ctx(), &args("/etc/hosts", "localhost", "evil"));
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_PATH_OUTSIDE_ROOTS");
}
