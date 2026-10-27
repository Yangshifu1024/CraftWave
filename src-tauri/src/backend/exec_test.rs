//! 工具执行器适配单测：五工具分发、参数错误回喂、权限档排除。

use std::sync::Arc;

use super::exec::ToolsetExecutor;
use super::run_loop::{CancellableExecutor, ToolExecution};
use crate::safety::tier::PermissionTier;
use crate::tools::ToolsContext;
use tokio_util::sync::CancellationToken;

fn temp_ctx() -> ToolsContext {
    let dir = tempfile::tempdir().expect("tempdir");
    // leak：单测生命周期内持有一个临时根即可
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    ToolsContext::new(root.clone(), root)
}

async fn execute(name: &str, args_json: &str) -> (ToolExecution, ToolsContext) {
    let ctx = temp_ctx();
    let token = CancellationToken::new();
    let executor = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
    let out = executor.execute("call_1", name, args_json, &token).await;
    (out, ctx)
}

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

#[tokio::test]
async fn unknown_tool_feeds_back_invalid_arg() {
    let (out, _) = execute("rm_rf", "{}").await;
    let ToolExecution::Completed { ok, error, .. } = out else {
        panic!("应为 Completed");
    };
    assert!(!ok);
    assert_eq!(error.expect("错误面").code, "E_INVALID_ARG");
}

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
