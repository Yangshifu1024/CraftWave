//! read 工具测试：行窗口 offset/limit、512KB 截断、GBK 解码、二进制拒绝、
//! image base64 与上限（越界拒绝见 mod.rs 共享守卫测试 path_guard_test）。

use serde_json::json;

use crate::tools::read::{ReadArgs, ReadKind, run};
use crate::util::test_support::TestWorkspace;

/// 测试辅助：构造默认参数（无窗口、text 类型）的读取参数。
fn text_args(path: &str) -> ReadArgs {
    ReadArgs {
        path: path.to_string(),
        offset: None,
        limit: None,
        kind: None,
        page: None,
        pages: None,
        sheet: None,
    }
}

/// 测试辅助：在工作区写入一个文件（自动建父目录）。
fn write_project_file(ws: &TestWorkspace, rel: &str, content: &str) {
    if let Some(parent) = std::path::Path::new(rel).parent() {
        std::fs::create_dir_all(ws.project_file(parent.to_str().expect("utf8"))).expect("建父目录");
    }
    std::fs::write(ws.project_file(rel), content).expect("写文件");
}

// 断言：文本读取矩阵——默认参数读全文（encoding/totalLines/content 齐全）、
// offset/limit 切窗 + truncated 标记、offset 超总行数返回空不算截断、超
// 512KB 在整行边界截断、GBK 经共用解码器正确解码、二进制（含 NUL）拒绝。
#[test]
fn text_window_encoding_and_truncation_matrix() {
    // 默认参数读全文。
    let ws = TestWorkspace::new();
    write_project_file(&ws, "a.txt", "l1\nl2\nl3\n");
    let outcome = run(&ws.ctx(), &text_args("a.txt"));
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["kind"], "text");
    assert_eq!(data["encoding"], "UTF-8");
    assert_eq!(data["totalLines"], json!(3));
    assert_eq!(data["content"], json!("l1\nl2\nl3\n"));
    assert_eq!(data["truncated"], json!(false));
    assert!(outcome.ui_summary.contains("第 1–3 行"));

    // offset/limit 切窗，窗口后仍有行时 truncated=true。
    let body: String = (1..=10).map(|i| format!("line{i}\n")).collect();
    write_project_file(&ws, "win.txt", &body);
    let mut args = text_args("win.txt");
    args.offset = Some(4);
    args.limit = Some(3);
    let outcome = run(&ws.ctx(), &args);
    let data = outcome.data.expect("data");
    assert_eq!(data["content"], json!("line4\nline5\nline6\n"));
    assert_eq!(data["returnedLines"], json!(3));
    assert_eq!(data["truncated"], json!(true), "后面还有行应标 truncated");
    assert!(outcome.ui_summary.contains("第 4–6 行"));

    // offset 超过总行数返回空内容且不算截断。
    write_project_file(&ws, "short.txt", "one\n");
    let mut args = text_args("short.txt");
    args.offset = Some(100);
    let outcome = run(&ws.ctx(), &args);
    let data = outcome.data.expect("data");
    assert_eq!(data["content"], json!(""));
    assert_eq!(data["truncated"], json!(false));

    // 超 512KB 单响应上限：整行边界截断。
    let line = "x".repeat(200 * 1024);
    let body = format!("{line}\n{line}\n{line}\n{line}\n{line}\n");
    write_project_file(&ws, "big.txt", &body);
    let outcome = run(&ws.ctx(), &text_args("big.txt"));
    let data = outcome.data.expect("data");
    let content = data["content"].as_str().expect("str");
    assert!(content.len() <= 512 * 1024, "应 ≤512KB");
    assert_eq!(data["truncated"], json!(true));
    assert!(content.ends_with('\n'), "整行边界截断");

    // GBK 文件经共用解码器正确解码（encoding 报 GBK）。
    let (encoded, _, _) = encoding_rs::GBK.encode("第一行：中文\n第二行：GBK\n");
    std::fs::write(ws.project_file("gbk.txt"), &encoded).expect("写 GBK");
    let outcome = run(&ws.ctx(), &text_args("gbk.txt"));
    let data = outcome.data.expect("data");
    assert_eq!(data["encoding"], "GBK");
    assert_eq!(data["content"], json!("第一行：中文\n第二行：GBK\n"));

    // 二进制文件（前 8KB 含 NUL）不支持文本读取。
    std::fs::write(ws.project_file("bin.dat"), b"abc\x00def\x00").expect("写二进制");
    let outcome = run(&ws.ctx(), &text_args("bin.dat"));
    assert!(!outcome.ok);
    let err = outcome.error.expect("error");
    assert_eq!(err.code, "E_INVALID_ARG");
    assert!(err.message.contains("二进制"));
}

// 断言：kind=image 通道矩阵——返回可解码的 base64 与正确 mediaType（含 NUL
// 的 PNG 不受二进制判定影响）、超 3MiB 拒绝、不支持的格式（bmp）拒绝。
#[test]
fn image_kind_matrix() {
    let ws = TestWorkspace::new();

    // 正常路径：base64 可原样解码还原。
    let png: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00";
    std::fs::write(ws.project_file("icon.png"), png).expect("写 png");
    let mut args = text_args("icon.png");
    args.kind = Some(ReadKind::Image);
    let outcome = run(&ws.ctx(), &args);
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["kind"], "image");
    assert_eq!(data["mediaType"], "image/png");
    use base64::Engine;
    let b64 = data["base64"].as_str().expect("base64");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .expect("合法 base64");
    assert_eq!(decoded, png);

    // 超 3MiB 拒绝。
    let big = vec![0xFFu8; 3 * 1024 * 1024 + 1];
    std::fs::write(ws.project_file("big.jpg"), &big).expect("写大图");
    let mut args = text_args("big.jpg");
    args.kind = Some(ReadKind::Image);
    let outcome = run(&ws.ctx(), &args);
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_INVALID_ARG");

    // 不支持的图片格式拒绝。
    std::fs::write(ws.project_file("pic.bmp"), b"BMxx").expect("写 bmp");
    let mut args = text_args("pic.bmp");
    args.kind = Some(ReadKind::Image);
    let outcome = run(&ws.ctx(), &args);
    assert!(!outcome.ok);
}
