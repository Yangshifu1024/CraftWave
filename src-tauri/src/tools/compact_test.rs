//! compact 文本与 ToolOutcome 测试（§8.2 双通道）。

use super::{ToolOutcome, compact_text};
use crate::util::errors::ToolError;

// 断言：成功结果的 compact 文本是 data 的 JSON 序列化。
#[test]
fn success_compact_serializes_data() {
    let outcome = ToolOutcome::success(serde_json::json!({ "content": "hello" }), "读取 ok");
    assert_eq!(compact_text(&outcome), r#"{"content":"hello"}"#);
}

// 断言：失败结果的 compact 文本是 "错误码 + 消息"。
#[test]
fn failure_compact_is_code_plus_message() {
    let outcome = ToolOutcome::failure(ToolError::edit_no_match("oldText 未命中"), "编辑失败");
    assert_eq!(compact_text(&outcome), "E_EDIT_NO_MATCH oldText 未命中");
}

// 断言：超限 data 截断为头 4KB + 尾 8KB，带截断标记且保留 JSON 收尾。
#[test]
fn oversize_data_is_truncated_head_and_tail() {
    // 头 4KB + 尾 8KB：构造 >12KB 的 data。
    let big = "A".repeat(4 * 1024) + &"B".repeat(5 * 1024) + &"C".repeat(8 * 1024);
    let outcome = ToolOutcome::success(serde_json::json!({ "content": big }), "读取大文件");
    let compact = compact_text(&outcome);
    assert!(compact.len() < big.len(), "应发生截断");
    assert!(compact.contains("…[截断"), "应含截断标记");
    assert!(compact.starts_with(r#"{"content":"AAAA"#));
    assert!(compact.ends_with("CCCC\"}"), "尾部应保留（含 JSON 收尾）");
}

// 断言：多字节字符（中文 3 字节）边界落在字符中间时回退到字符边界，不 panic。
#[test]
fn multibyte_truncation_never_panics() {
    // 中文字符 3 字节，边界落在字符中间时须回退到字符边界。
    let big = "中".repeat(10_000);
    let outcome = ToolOutcome::success(serde_json::json!({ "content": big }), "中文");
    let compact = compact_text(&outcome);
    assert!(compact.contains("…[截断"));
}

// 断言：ui_summary 与 detail（含 warnings）只进 UI 边车，不进模型文本。
#[test]
fn ui_summary_and_detail_stay_out_of_model_text() {
    // detail（含 warnings）只进边车，不进 compact。
    let outcome = ToolOutcome::success(serde_json::json!({ "content": "x" }), "ok")
        .with_detail(serde_json::json!({ "warnings": ["版本过期"] }));
    assert!(!compact_text(&outcome).contains("版本过期"));
    assert!(outcome.detail.is_some());
}
