//! compact 文本与 ToolOutcome 测试（§8.2 双通道）。

use super::{ToolOutcome, compact_text};
use crate::util::errors::ToolError;

// 断言：compact 通道矩阵——成功序列化 data、失败为「错误码 + 消息」、
// 超限 data 头 4KB + 尾 8KB 截断（含 JSON 收尾）、多字节字符边界安全、
// ui_summary 与 detail（含 warnings）只进 UI 边车不进模型文本。
#[test]
fn compact_text_channel_matrix() {
    // 成功：data 的 JSON 序列化。
    let outcome = ToolOutcome::success(serde_json::json!({ "content": "hello" }), "读取 ok");
    assert_eq!(compact_text(&outcome), r#"{"content":"hello"}"#);

    // 失败："错误码 + 消息"。
    let outcome = ToolOutcome::failure(ToolError::edit_no_match("oldText 未命中"), "编辑失败");
    assert_eq!(compact_text(&outcome), "E_EDIT_NO_MATCH oldText 未命中");

    // 超限：头 4KB + 尾 8KB，带截断标记且保留 JSON 收尾。
    let big = "A".repeat(4 * 1024) + &"B".repeat(5 * 1024) + &"C".repeat(8 * 1024);
    let outcome = ToolOutcome::success(serde_json::json!({ "content": big }), "读取大文件");
    let compact = compact_text(&outcome);
    assert!(compact.len() < 17 * 1024, "应发生截断：{}", compact.len());
    assert!(compact.contains("…[截断"), "应含截断标记");
    assert!(compact.starts_with(r#"{"content":"AAAA"#));
    assert!(compact.ends_with("CCCC\"}"), "尾部应保留（含 JSON 收尾）");

    // 多字节字符（中文 3 字节）边界落在字符中间时回退到字符边界，不 panic。
    let big = "中".repeat(10_000);
    let outcome = ToolOutcome::success(serde_json::json!({ "content": big }), "中文");
    assert!(compact_text(&outcome).contains("…[截断"));

    // detail（含 warnings）只进边车，不进 compact。
    let outcome = ToolOutcome::success(serde_json::json!({ "content": "x" }), "ok")
        .with_detail(serde_json::json!({ "warnings": ["版本过期"] }));
    assert!(!compact_text(&outcome).contains("版本过期"));
    assert!(outcome.detail.is_some());
}
