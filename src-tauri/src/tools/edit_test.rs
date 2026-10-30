//! edit 工具测试：三级匹配、唯一性拒绝、version 乐观锁、EOL 保持、
//! lineChanges 统计、GBK 文件转 UTF-8、越界拒绝。

use serde_json::json;

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

// 断言：L1 精确唯一命中完成替换，matchLevel=exact，lineChanges 统计正确。
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

// 断言：L2 缩进平移——oldText 无缩进也能命中深缩进块，
// 且替换区间含原首行缩进、newText 原样落位。
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

// 断言：L3 逐行空白归一——各行缩进/尾随空白不一致时仍能整块命中替换。
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

// 断言：0 命中报 E_EDIT_NO_MATCH，多处命中报 E_EDIT_AMBIGUOUS（绝不猜）。
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

// 断言：空 oldText 以 E_INVALID_ARG 拒绝。
#[test]
fn empty_old_text_is_invalid_arg() {
    let ws = TestWorkspace::new();
    write_file(&ws, "f.txt", "x\n");
    let outcome = run(&ws.ctx(), &args("f.txt", "", "y"));
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_INVALID_ARG");
}

// 断言：version 乐观锁三态——匹配正常执行、不匹配但可唯一命中降级警告执行、
// 不匹配且不可唯一命中报 E_EDIT_VERSION_STALE。
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

// 断言：CRLF 文件编辑后仍保持 CRLF 行尾风格。
#[test]
fn crlf_style_is_preserved() {
    let ws = TestWorkspace::new();
    write_file(&ws, "win.txt", "line1\r\nline2\r\n");
    let outcome = run(&ws.ctx(), &args("win.txt", "line2", "line-two"));
    assert!(outcome.ok, "{outcome:?}");
    let bytes = std::fs::read(ws.project_file("win.txt")).expect("读回");
    assert_eq!(bytes, b"line1\r\nline-two\r\n", "CRLF 应保持");
}

// 断言：GBK 编码文件编辑后按 UTF-8 落盘（C-10 接受的转换副作用）。
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

// 断言：lineChanges 的 add/del 分别按 newText/oldText 行数计
//（扩行、缩行、整块删除三种口径）。
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

// 断言：roots 外的绝对路径被拒绝（E_PATH_OUTSIDE_ROOTS）。
#[test]
fn outside_roots_rejected() {
    let ws = TestWorkspace::new();
    let outcome = run(&ws.ctx(), &args("/etc/hosts", "localhost", "evil"));
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_PATH_OUTSIDE_ROOTS");
}

// —— 07 号票：同批同文件多 edit 合并原子提交（apply_edit_group，§8.4 步骤 4/5）——

use super::apply_edit_group;

// 断言：非重叠多 edit 基于同一原内容预演，一次合并提交——两个 edit 都
// 成功、共享同一最终 version，文件内容为两处替换叠加。
#[test]
fn group_merges_non_overlapping_edits_one_commit() {
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
}

// 断言：区间重叠的后续成员被拒（E_INVALID_ARG），源序首个保留——
// 文件只体现首个 edit 的修改。
#[test]
fn group_rejects_overlapping_member_after_first() {
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
}

// 断言：批次事务回滚——落盘闭包失败 → 整组回滚（文件保持原内容），
// 全部接受成员以失败收口（§8.4 步骤 5"任一失败回滚整组"）。
#[test]
fn group_commit_failure_rolls_back_whole_group() {
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
    // 备份文件已清理（同目录无 .cw-bak-* 残留）。
    let leftovers: Vec<_> = std::fs::read_dir(ws.project_file("."))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".cw-bak-"))
        .collect();
    assert!(leftovers.is_empty(), "备份文件应清理：{leftovers:?}");
}

// 断言：任一成员匹配失败 → 整组降级为源序逐个独立执行（各出各的结果，
// 第二个成员基于第一个修改后的文件匹配）。
#[test]
fn group_demotes_to_independent_runs_when_member_fails_to_match() {
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
}

// 断言：合并组各成员的 lineChanges 各自统计（对位正确）。
#[test]
fn group_line_changes_are_per_member() {
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
}

// —— 22 号票：写后完整性自检（合并提交出口 + 单次 edit 出口）——

// 故障注入：落盘闭包写入「被篡改的内容」（与预期 final_bytes 不一致，
// 等价磁盘静默损坏场景），写后自检应在哈希阶段拦下，整组接受成员以
// 失败收口回喂 E_INTERNAL（错误回喂含路径与校验阶段）。
#[test]
fn group_postcheck_injection_fails_with_internal_error() {
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
    assert_eq!(err.code, "E_INTERNAL", "错误码用 E_INTERNAL（§3.6 兜底）");
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
}

// 正常路径：合并提交出口的自检对 CRLF 文件通过（EOL 保持实证不误伤）；
// 单次 edit（run）的 CRLF 写回同样过自检（行为面回归）。
#[test]
fn postcheck_passes_normal_and_crlf_edits() {
    let ws = TestWorkspace::new();
    // 合并提交 + LF 文件。
    write_file(&ws, "g.txt", "alpha\nbeta\n");
    let outcomes = apply_edit_group(
        &ws.ctx(),
        "g.txt",
        &[args("g.txt", "alpha", "ALPHA")],
        crate::tools::write::atomic_write_utf8,
    );
    assert!(outcomes[0].ok, "LF 正常合并应过自检：{:?}", outcomes[0]);
    // CRLF 文件的单次 edit：写回保持 CRLF，自检通过。
    write_file(&ws, "crlf.txt", "line one\r\nline two\r\n");
    let outcome = run(&ws.ctx(), &args("crlf.txt", "line one", "LINE ONE"));
    assert!(outcome.ok, "CRLF 编辑应过自检：{outcome:?}");
    assert_eq!(
        std::fs::read(ws.project_file("crlf.txt")).unwrap(),
        b"LINE ONE\r\nline two\r\n".to_vec(),
        "EOL 保持不被自检破坏"
    );
}
