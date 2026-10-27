//! 工具执行器适配的单元测试：五个文件工具的分发执行、参数错误以
//! E_INVALID_ARG 回喂、plan 权限档对写工具的排除。

use std::sync::Arc;

use super::exec::ToolsetExecutor;
use super::run_loop::{CancellableExecutor, ToolExecution};
use crate::safety::tier::PermissionTier;
use crate::tools::ToolsContext;
use tokio_util::sync::CancellationToken;

/// 构造以临时目录为项目根的测试上下文（临时目录故意不主动清理——
/// 单测进程生命周期内持有一个临时根即可，避免上下文被提前删掉）。
fn temp_ctx() -> ToolsContext {
    let dir = tempfile::tempdir().expect("tempdir");
    // leak：单测生命周期内持有一个临时根即可
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    ToolsContext::new(root.clone(), root)
}

/// 以全开放权限档执行一次工具，返回（执行产物, 测试上下文）。
async fn execute(name: &str, args_json: &str) -> (ToolExecution, ToolsContext) {
    let ctx = temp_ctx();
    let token = CancellationToken::new();
    let executor = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
    let out = executor.execute("call_1", name, args_json, &token).await;
    (out, ctx)
}

/// 断言 read 成功时双通道各自正确：界面摘要含文件名、模型通道文本携带
/// 文件内容。
#[tokio::test]
async fn read_success_model_channel_compact_text() {
    let (out, _ctx_guard) = {
        let ctx = temp_ctx();
        std::fs::write(ctx.project_root.join("a.txt"), "第一行\n第二行").unwrap();
        let token = CancellationToken::new();
        let executor = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
        let out = executor
            .execute("call_1", "read", r#"{"path": "a.txt"}"#, &token)
            .await;
        (out, ctx)
    };
    let ToolExecution::Completed {
        ok,
        ui_summary,
        error,
        model_text,
    } = out
    else {
        panic!("应为 Completed");
    };
    assert!(ok);
    assert!(error.is_none());
    assert!(
        ui_summary.contains("a.txt"),
        "ui_summary 一行人话：{ui_summary}"
    );
    assert!(
        model_text.contains("第一行"),
        "模型通道携带文件内容：{model_text}"
    );
}

/// 断言参数不是合法 JSON 时按工具级失败收口：仍是 Completed（失败也是
/// 收齐的结果），E_INVALID_ARG 回喂模型，run 不中断。
#[tokio::test]
async fn invalid_args_tool_level_failure_run_continues() {
    let (out, _ctx) = execute("read", "{oops 半截").await;
    let ToolExecution::Completed {
        ok,
        error,
        model_text,
        ..
    } = out
    else {
        panic!("应为 Completed（失败也是收齐的结果）");
    };
    assert!(!ok);
    let error = error.expect("带错误面");
    assert_eq!(error.code, "E_INVALID_ARG");
    assert!(
        model_text.contains("E_INVALID_ARG"),
        "模型通道含 code+message"
    );
}

/// 断言未知工具名同样以 E_INVALID_ARG 工具级失败回喂。
#[tokio::test]
async fn unknown_tool_feeds_back_invalid_arg() {
    let (out, _) = execute("rm_rf", "{}").await;
    let ToolExecution::Completed { ok, error, .. } = out else {
        panic!("应为 Completed");
    };
    assert!(!ok);
    assert_eq!(error.expect("错误面").code, "E_INVALID_ARG");
}

/// 断言 plan 权限档下写工具被 E_TOOL_BLOCKED 排除，且文件不落盘。
#[tokio::test]
async fn plan_tier_write_tools_excluded_tool_blocked() {
    let ctx = temp_ctx();
    let token = CancellationToken::new();
    let executor = Arc::new(ToolsetExecutor::new(ctx.clone(), PermissionTier::Plan));
    let out = executor
        .execute(
            "call_1",
            "write",
            r#"{"path": "x.txt", "content": "hi"}"#,
            &token,
        )
        .await;
    let ToolExecution::Completed {
        ok,
        error,
        ui_summary,
        ..
    } = out
    else {
        panic!("应为 Completed");
    };
    assert!(!ok, "plan 档 FileWrite 过滤（§8.5）");
    assert_eq!(error.expect("错误面").code, "E_TOOL_BLOCKED");
    assert!(ui_summary.contains("write"));
    assert!(!ctx.project_root.join("x.txt").exists(), "被排除工具不落盘");
}
