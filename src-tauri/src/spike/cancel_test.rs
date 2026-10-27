//! 【S5 Spike】取消标记帧协议的单元测试（独立测试文件，源码零测试）。

use super::cancel::{E_CANCELLED, ToolOutcome};
use super::frames::{ChatFrame, EndReason, FrameError};

/// 取消终态帧的序列化形态：reason=cancelled（§3.4 枚举），取消不是 error。
#[test]
fn run_ended_取消终态序列化形态() {
    let v = serde_json::to_value(ChatFrame::RunEnded {
        reason: EndReason::Cancelled,
        duration_ms: 87,
        tool_count: 2,
        turn: 1,
    })
    .unwrap();
    assert_eq!(v["type"], "runEnded");
    assert_eq!(v["reason"], "cancelled");
    assert_eq!(v["durationMs"], 87);
    assert_eq!(v["toolCount"], 2);
    assert!(v.get("error").is_none(), "取消终态不挂 error 面");
}

/// 被取消工具的 toolResult：ok=false + error.code=E_CANCELLED（camelCase）。
#[test]
fn tool_result_取消标记带_e_cancelled_错误码() {
    let v = serde_json::to_value(ChatFrame::ToolResult {
        call_id: "call_2".into(),
        ok: false,
        ui_summary: "E_CANCELLED: 进程组已收割".into(),
        error: Some(FrameError {
            code: E_CANCELLED.into(),
            message: "进程组已收割".into(),
        }),
    })
    .unwrap();
    assert_eq!(v["type"], "toolResult");
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "E_CANCELLED");
    assert_eq!(v["error"]["message"], "进程组已收割");
}

/// 已完成工具的 toolResult 不带 error 字段（§3.4 skip 序列化，前端兼容）。
#[test]
fn tool_result_已完成结果不带错误面() {
    let v = serde_json::to_value(ChatFrame::ToolResult {
        call_id: "call_1".into(),
        ok: true,
        ui_summary: "[read] 12 行".into(),
        error: None,
    })
    .unwrap();
    assert_eq!(v["ok"], true);
    assert!(v.get("error").is_none(), "无错误时 error 字段省略");
}

/// 工具结果语义：Completed 保留 / Cancelled 补标记（收齐后由 RunLoop 出帧）。
#[test]
fn tool_outcome_语义区分() {
    let done = ToolOutcome::Completed("[read] 12 行".into());
    let cancelled = ToolOutcome::Cancelled("进程组已收割".into());
    assert!(matches!(done, ToolOutcome::Completed(_)));
    assert!(matches!(cancelled, ToolOutcome::Cancelled(_)));
    assert_ne!(done, cancelled);
}
