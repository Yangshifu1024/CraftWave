//! edit 工具测试：三级匹配、唯一性拒绝、version 乐观锁、EOL 保持、
//! lineChanges 统计、GBK 文件转 UTF-8、同批合并原子提交与
//! 写后自检（越界拒绝见 mod.rs 共享守卫测试 path_guard_test）。

use serde_json::json;

use super::apply_edit_group;
use super::{EditArgs, run};
use crate::util::test_support::TestWorkspace;

/// 测试辅助：构造 edit 参数（不带 version 乐观锁）。
fn args(path: &str, old: &str, new: &str) -> EditArgs {
    EditArgs {
        path: path.to_string(),
        old_text: old.to_string(),
        new_text: new.to_string(),
        version: None,
    }
}

/// 测试辅助：在测试工作区预置一个文件。
fn write_file(ws: &TestWorkspace, rel: &str, content: &str) {
    std::fs::write(ws.project_file(rel), content).expect("预置文件");
}

// 断言：单次 edit 语义矩阵——L1 精确唯一命中（matchLevel/lineChanges/内容
// 替换正确）、L2 缩进平移（替换区间含原首行缩进、newText 原样落位）、L3 逐行
// 空白归一；0 命中报 E_EDIT_NO_MATCH、多处命中报 E_EDIT_AMBIGUOUS、空
// oldText 报 E_INVALID_ARG；version 乐观锁三态（匹配执行/不匹配可唯一命中
// 降级警告执行/不匹配且不可唯一命中报 E_EDIT_VERSION_STALE）；CRLF 保持、
// GBK 转 UTF-8 落盘、lineChanges 三种口径。
#[test]
fn single_edit_semantics_matrix() {
    // —— L1 精确唯一命中 ——
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

    // —— L2 缩进平移：oldText 无缩进也能命中深缩进块 ——
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
    // 缩进平移只用于「匹配」；替换区间含原首行缩进，
    // newText 原样落位（缩进由模型自带）。
    assert_eq!(
        std::fs::read_to_string(ws.project_file("b.rs")).expect("读回"),
        "function f() {\ndoWork(1);\nreturn false;\n    }\n"
    );

    // —— L3 逐行空白归一 ——
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

    // —— 0 命中 / 多处命中 / 空 oldText ——
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

    write_file(&ws, "f.txt", "x\n");
    let outcome = run(&ws.ctx(), &args("f.txt", "", "y"));
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_INVALID_ARG");

    // —— version 乐观锁三态 ——
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

    // —— CRLF 保持 ——
    write_file(&ws, "win.txt", "line1\r\nline2\r\n");
    let outcome = run(&ws.ctx(), &args("win.txt", "line2", "line-two"));
    assert!(outcome.ok, "{outcome:?}");
    let bytes = std::fs::read(ws.project_file("win.txt")).expect("读回");
    assert_eq!(bytes, b"line1\r\nline-two\r\n", "CRLF 应保持");

    // —— GBK 转 UTF-8 落盘（接受的转换副作用）——
    let (gbk, _, _) = encoding_rs::GBK.encode("旧值：alpha\n重复行\n");
    std::fs::write(ws.project_file("legacy.ini"), &gbk).expect("写 GBK");
    let outcome = run(&ws.ctx(), &args("legacy.ini", "alpha", "beta"));
    assert!(outcome.ok, "{outcome:?}");
    let bytes = std::fs::read(ws.project_file("legacy.ini")).expect("读回");
    let text = std::str::from_utf8(&bytes).expect("编辑后应为 UTF-8（落盘编码副作用）");
    assert_eq!(text, "旧值：beta\n重复行\n");

    // —— lineChanges 三种口径：扩行、缩行、整块删除 ——
    write_file(&ws, "j.txt", "start\nend\n");
    let outcome = run(&ws.ctx(), &args("j.txt", "start", "a\nb\nc"));
    let data = outcome.data.expect("data");
    assert_eq!(data["lineChanges"], json!({ "add": 3, "del": 1 }));

    write_file(&ws, "k.txt", "x\ny\n");
    let outcome = run(&ws.ctx(), &args("k.txt", "x\ny", "z"));
    let data = outcome.data.expect("data");
    assert_eq!(data["lineChanges"], json!({ "add": 1, "del": 2 }));

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

// 断言：同批同文件多 edit 合并语义矩阵（apply_edit_group 预演/应用 +
// 写后自检）——非重叠多 edit 一次合并提交（共享同一 version、各成员
// lineChanges 对位）；区间重叠的后续成员被拒；任一成员匹配失败整组降级为
// 源序逐个独立执行；落盘闭包失败整组回滚且备份清理；写后自检拦下篡改写入
// （E_INTERNAL + 路径 + 校验阶段）；正常与 CRLF 编辑通过自检。
#[test]
fn edit_group_commit_and_postcheck_matrix() {
    // —— 非重叠多 edit 合并提交 ——
    let ws = TestWorkspace::new();
    write_file(&ws, "g.txt", "alpha\nbeta\ngamma\n");
    let outcomes = apply_edit_group(
        &ws.ctx(),
        "g.txt",
        &[
            args("g.txt", "alpha", "ALPHA"),
            args("g.txt", "gamma", "GAMMA"),
        ],
        crate::tools::write::atomic_write_utf8,
    );
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes[0].ok, "{:?}", outcomes[0]);
    assert!(outcomes[1].ok, "{:?}", outcomes[1]);
    // 两个成员共享同一最终 version（一次落盘）。
    assert_eq!(
        outcomes[0].data.as_ref().unwrap()["version"],
        outcomes[1].data.as_ref().unwrap()["version"]
    );
    assert_eq!(
        std::fs::read_to_string(ws.project_file("g.txt")).unwrap(),
        "ALPHA\nbeta\nGAMMA\n"
    );

    // —— lineChanges 按成员对位统计 ——
    let ws = TestWorkspace::new();
    write_file(&ws, "g.txt", "a\nb\nc\n");
    let outcomes = apply_edit_group(
        &ws.ctx(),
        "g.txt",
        &[args("g.txt", "a\nb", "x\ny\nz"), args("g.txt", "c", "d")],
        crate::tools::write::atomic_write_utf8,
    );
    assert!(outcomes[0].ok && outcomes[1].ok, "{outcomes:?}");
    assert_eq!(
        outcomes[0].data.as_ref().unwrap()["lineChanges"],
        json!({ "add": 3, "del": 2 })
    );
    assert_eq!(
        outcomes[1].data.as_ref().unwrap()["lineChanges"],
        json!({ "add": 1, "del": 1 })
    );
    assert_eq!(
        std::fs::read_to_string(ws.project_file("g.txt")).unwrap(),
        "x\ny\nz\nd\n"
    );

    // —— 区间重叠的后续成员被拒（E_INVALID_ARG），首个保留 ——
    let ws = TestWorkspace::new();
    write_file(&ws, "g.txt", "one\ntwo\nthree\n");
    let outcomes = apply_edit_group(
        &ws.ctx(),
        "g.txt",
        &[
            args("g.txt", "one\ntwo", "X"),
            args("g.txt", "two\nthree", "Y"),
        ],
        crate::tools::write::atomic_write_utf8,
    );
    assert!(outcomes[0].ok, "首个成员保留：{:?}", outcomes[0]);
    assert!(!outcomes[1].ok, "重叠成员被拒");
    assert_eq!(outcomes[1].error.as_ref().unwrap().code, "E_INVALID_ARG");
    assert_eq!(
        std::fs::read_to_string(ws.project_file("g.txt")).unwrap(),
        "X\nthree\n"
    );

    // —— 成员匹配失败 → 整组降级为源序独立执行 ——
    let ws = TestWorkspace::new();
    write_file(&ws, "g.txt", "head\nmid\ntail\n");
    let outcomes = apply_edit_group(
        &ws.ctx(),
        "g.txt",
        &[
            args("g.txt", "mid", "MID"),
            args("g.txt", "不存在的原文", "whatever"),
        ],
        crate::tools::write::atomic_write_utf8,
    );
    assert_eq!(outcomes.len(), 2);
    assert!(outcomes[0].ok, "第一个成员独立执行成功：{:?}", outcomes[0]);
    assert!(!outcomes[1].ok, "匹配失败成员回喂错误");
    assert_eq!(outcomes[1].error.as_ref().unwrap().code, "E_EDIT_NO_MATCH");
    assert_eq!(
        std::fs::read_to_string(ws.project_file("g.txt")).unwrap(),
        "head\nMID\ntail\n"
    );

    // —— 落盘失败 → 整组回滚（文件保持原内容），备份清理 ——
    let ws = TestWorkspace::new();
    write_file(&ws, "g.txt", "keep\n");
    let outcomes = apply_edit_group(
        &ws.ctx(),
        "g.txt",
        &[
            args("g.txt", "keep", "changed1"),
            args("g.txt", "keep", "changed2"),
        ],
        |_target, _bytes| {
            Err(crate::util::errors::ToolError::new(
                "E_IO",
                "注入的落盘失败",
            ))
        },
    );
    assert_eq!(outcomes.len(), 2);
    for outcome in &outcomes {
        assert!(!outcome.ok, "落盘失败全组失败：{outcome:?}");
    }
    assert_eq!(
        std::fs::read_to_string(ws.project_file("g.txt")).unwrap(),
        "keep\n",
        "回滚后文件保持原样"
    );
    let leftovers: Vec<_> = std::fs::read_dir(ws.project_file("."))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".cw-bak-"))
        .collect();
    assert!(leftovers.is_empty(), "备份文件应清理：{leftovers:?}");

    // —— 写后自检：篡改写入在哈希阶段拦下 ——
    let ws = TestWorkspace::new();
    write_file(&ws, "g.txt", "alpha\nbeta\ngamma\n");
    let outcomes = apply_edit_group(
        &ws.ctx(),
        "g.txt",
        &[args("g.txt", "beta", "BETA")],
        |target, _expected| crate::tools::write::atomic_write_utf8(target, b"tampered"),
    );
    assert_eq!(outcomes.len(), 1);
    assert!(!outcomes[0].ok, "篡改写入应被自检拦下：{:?}", outcomes[0]);
    let err = outcomes[0].error.as_ref().unwrap();
    assert_eq!(err.code, "E_INTERNAL", "错误码用 E_INTERNAL（通用兜底）");
    assert!(
        err.message.contains("自检失败"),
        "message 指明自检失败：{}",
        err.message
    );
    assert!(
        err.message.contains("g.txt"),
        "message 含路径：{}",
        err.message
    );
    assert!(
        err.message.contains("哈希"),
        "message 含校验阶段：{}",
        err.message
    );

    // —— 自检不误伤：LF 合并提交与 CRLF 单次编辑均通过且 EOL 保持 ——
    let ws = TestWorkspace::new();
    write_file(&ws, "g.txt", "alpha\nbeta\n");
    let outcomes = apply_edit_group(
        &ws.ctx(),
        "g.txt",
        &[args("g.txt", "alpha", "ALPHA")],
        crate::tools::write::atomic_write_utf8,
    );
    assert!(outcomes[0].ok, "LF 正常合并应过自检：{:?}", outcomes[0]);
    write_file(&ws, "crlf.txt", "line one\r\nline two\r\n");
    let outcome = run(&ws.ctx(), &args("crlf.txt", "line one", "LINE ONE"));
    assert!(outcome.ok, "CRLF 编辑应过自检：{outcome:?}");
    assert_eq!(
        std::fs::read(ws.project_file("crlf.txt")).unwrap(),
        b"LINE ONE\r\nline two\r\n".to_vec(),
        "EOL 保持不被自检破坏"
    );
}
