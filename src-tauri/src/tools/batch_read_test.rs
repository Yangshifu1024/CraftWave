//! batch_read 工具测试：混合成功失败、文件数上限、共享 512KB 预算截断、
//! 续读 nextOffset、预算耗尽让路与空数组拒绝（矩阵化合并）。

use serde_json::json;

use super::{BatchReadArgs, run};
use crate::providers::types::{DEFAULT_BATCH_READ_LIMIT, MAX_BATCH_READ_LIMIT};
use crate::util::test_support::TestWorkspace;

/// 测试辅助：构造参数。
fn args(paths: &[&str]) -> BatchReadArgs {
    BatchReadArgs {
        paths: paths.iter().map(|p| (*p).to_string()).collect(),
    }
}

/// 测试辅助：在工作区写入文件（自动建父目录）。
fn write_project_file(ws: &TestWorkspace, rel: &str, content: &str) {
    if let Some(parent) = std::path::Path::new(rel).parent() {
        std::fs::create_dir_all(ws.project_file(parent.to_str().expect("utf8"))).expect("建父目录");
    }
    std::fs::write(ws.project_file(rel), content).expect("写文件");
}

// 断言：batch_read 全景矩阵——多文件按源序返回（内容与行数元数据齐全）、
// 单文件失败（缺失/二进制/越界）不失败整批、超文件数上限整批拒绝且上限
// 可配、空 paths 拒绝、512KB 预算跨全批共享（首个大文件按整行边界截断并带
// nextOffset、后续文件预算耗尽让路）、nextOffset 可直接续读到文件末行。
#[test]
fn batch_results_budget_and_resume_matrix() {
    // —— 多文件按源序成功 ——
    let ws = TestWorkspace::new();
    write_project_file(&ws, "a.txt", "a1\na2\n");
    write_project_file(&ws, "sub/b.md", "b1\n");
    let outcome = run(
        &ws.ctx(),
        &args(&["a.txt", "sub/b.md"]),
        DEFAULT_BATCH_READ_LIMIT,
    );
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    let files = data["files"].as_array().expect("files");
    assert_eq!(files.len(), 2);
    assert_eq!(files[0]["path"], json!("a.txt"));
    assert_eq!(files[0]["ok"], json!(true));
    assert_eq!(files[0]["content"], json!("a1\na2\n"));
    assert_eq!(files[0]["totalLines"], json!(2));
    assert_eq!(files[1]["path"], json!("sub/b.md"));
    assert_eq!(files[1]["content"], json!("b1\n"));
    assert_eq!(data["truncatedFiles"], json!([]));
    assert!(outcome.ui_summary.contains("2 成功、0 失败"));

    // —— 单文件失败不失败整批 ——
    write_project_file(&ws, "good.txt", "fine\n");
    std::fs::write(ws.project_file("bin.dat"), b"abc\x00def\x00").expect("写二进制");
    let outcome = run(
        &ws.ctx(),
        &args(&["good.txt", "missing.txt", "bin.dat", "../outside.txt"]),
        DEFAULT_BATCH_READ_LIMIT,
    );
    assert!(outcome.ok, "单文件失败不应失败整批：{outcome:?}");
    let files = outcome.data.expect("data")["files"]
        .as_array()
        .expect("files")
        .clone();
    assert_eq!(files.len(), 4);
    assert_eq!(files[0]["ok"], json!(true));
    assert_eq!(files[0]["content"], json!("fine\n"));
    assert_eq!(files[1]["ok"], json!(false));
    let missing = files[1]["error"].clone();
    assert_eq!(missing["code"], json!("E_NOT_FOUND"));
    assert!(
        missing["message"]
            .as_str()
            .expect("message")
            .contains("missing.txt"),
        "io 错误消息应带目标路径：{missing}"
    );
    // 二进制：E_INVALID_ARG（read 文本路径的判定复用）。
    assert_eq!(files[2]["error"]["code"], json!("E_INVALID_ARG"));
    // 越界：E_PATH_OUTSIDE_ROOTS（路径守卫拒绝）。
    assert_eq!(files[3]["error"]["code"], json!("E_PATH_OUTSIDE_ROOTS"));
    assert!(outcome.ui_summary.contains("1 成功、3 失败"));

    // —— 文件数上限：缺省 20 拒绝、limit=25 放行（上限常量与
    //    tools.batchReadLimit 同源）——
    assert_eq!(DEFAULT_BATCH_READ_LIMIT, 20);
    assert_eq!(MAX_BATCH_READ_LIMIT, 100);
    let paths: Vec<String> = (0..21).map(|i| format!("f{i}.txt")).collect();
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    let outcome = run(&ws.ctx(), &args(&refs), DEFAULT_BATCH_READ_LIMIT);
    assert!(!outcome.ok, "21 个文件应超缺省上限");
    assert_eq!(outcome.error.expect("error").code, "E_INVALID_ARG");
    assert!(outcome.ui_summary.contains("20"));
    let outcome = run(&ws.ctx(), &args(&refs), 25);
    assert!(outcome.ok, "{outcome:?}");

    // —— 空 paths 拒绝 ——
    let outcome = run(&ws.ctx(), &args(&[]), DEFAULT_BATCH_READ_LIMIT);
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("error").code, "E_INVALID_ARG");

    // —— 共享 512KB 预算：截断 + 让路 ——
    // 两个约 1.2MB 的文件（行 300 字节 × 4000 行）：2000 行默认窗口体积
    // （600KB）已超 512KB 预算——第一个在预算处按整行边界截断（byte 截断
    // 先于行窗口截断），第二个因预算耗尽不读取（让路）。
    let line = "x".repeat(299) + "\n"; // 300 字节/行
    let big = line.repeat(4000); // 1,200,000 字节
    write_project_file(&ws, "big1.txt", &big);
    write_project_file(&ws, "big2.txt", &big);
    let outcome = run(
        &ws.ctx(),
        &args(&["big1.txt", "big2.txt"]),
        DEFAULT_BATCH_READ_LIMIT,
    );
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    let files = data["files"].as_array().expect("files");
    assert_eq!(files.len(), 2);
    // 第一个文件：预算（512KB）内截断并带续读行号。
    assert_eq!(files[0]["ok"], json!(true));
    assert_eq!(files[0]["truncated"], json!(true));
    let next_offset = files[0]["nextOffset"].as_u64().expect("nextOffset");
    let returned = files[0]["returnedLines"].as_u64().expect("returnedLines");
    assert_eq!(next_offset, 1 + returned, "nextOffset = 起点 + 已返回行数");
    assert!(files[0]["content"].as_str().expect("content").len() <= 512 * 1024);
    // 第二个文件：预算耗尽未读取，指引分批。
    assert_eq!(files[1]["ok"], json!(false));
    assert!(
        files[1]["error"]["message"]
            .as_str()
            .expect("message")
            .contains("预算")
    );
    // truncatedFiles 只收录「已读取但截断」的文件。
    assert_eq!(data["truncatedFiles"], json!(["big1.txt"]));
    assert!(outcome.ui_summary.contains("截断"));

    // —— nextOffset 直接续读接上剩余内容 ——
    let line = "y".repeat(199) + "\n";
    let big = line.repeat(4000); // 800,000 字节 > 512KB
    write_project_file(&ws, "huge.txt", &big);
    let outcome = run(&ws.ctx(), &args(&["huge.txt"]), DEFAULT_BATCH_READ_LIMIT);
    assert!(outcome.ok, "{outcome:?}");
    let files = outcome.data.expect("data")["files"]
        .as_array()
        .expect("files")
        .clone();
    assert_eq!(files[0]["truncated"], json!(true));
    let next_offset = files[0]["nextOffset"].as_u64().expect("nextOffset");
    // 续读：从 nextOffset 起能读到文件末行（行号 4000）。
    let ctx = ws.ctx();
    let resolved = ctx
        .guard()
        .expect("guard")
        .resolve("huge.txt")
        .expect("resolve");
    let resumed = crate::tools::read::read_text_window(
        &resolved,
        "huge.txt",
        next_offset,
        crate::tools::read::DEFAULT_LINE_LIMIT,
        crate::util::encoding::MAX_RESPONSE_BYTES,
    )
    .expect("续读成功");
    assert_eq!(
        resumed.offset.saturating_add(resumed.returned_lines),
        4001,
        "续读应覆盖到文件末行（offset + returned = 末行 4000 的下一行号）"
    );
    assert!(!resumed.truncated, "续读窗口内不应再截断");
}
