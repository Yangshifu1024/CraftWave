//! 工具执行器适配：tools 五文件类工具 → RunLoop 的
//! [`CancellableExecutor`](super::run_loop::CancellableExecutor) 契约。
//!
//! - 模型通道回喂文本 = [`crate::tools::compact_text`]（§8.2 双通道）；
//! - 帧面 ok/uiSummary/error 直接取 `ToolOutcome`；
//! - argsJson 解析失败 / 未知工具 → `E_INVALID_ARG` 工具级失败回喂
//!   （Malformed 调用的宿主决议走「回喂错误促模型纠正」，§5.4 记录见票 Answer）；
//! - M1 五工具均为亚秒级文件操作，执行中取消感知由未来 command 工具在其
//!   实现内部以 `token` select! 进程组收割承担（原语 `tools::process`）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::frames::FrameError;
use super::run_loop::{CancellableExecutor, ToolExecution};
use crate::safety::tier::PermissionTier;
use crate::tools::{ToolOutcome, ToolsContext, compact_text, edit, grep, list_files, read, write};
use crate::util::errors::{E_INVALID_ARG, ToolError};

/// 五文件类工具执行器（§8.3 P0 子集）。
pub struct ToolsetExecutor {
    ctx: ToolsContext,
    /// 权限档（§8.5）：plan 档过滤后的工具集之外调用一律 E_TOOL_BLOCKED。
    tier: PermissionTier,
}

impl ToolsetExecutor {
    pub fn new(ctx: ToolsContext, tier: PermissionTier) -> Self {
        Self { ctx, tier }
    }

    /// 分发一个工具调用（同步文件操作，async 包装）。
    fn dispatch(&self, name: &str, args_json: &str) -> ToolOutcome {
        // §8.1 三处同源过滤的 RunLoop 侧：plan 档 FileWrite 已从工具表过滤，
        // 模型仍点名调用时按权限档排除（E_TOOL_BLOCKED，不中断 run）。
        if !crate::tools::TOOL_NAMES.contains(&name) {
            return ToolOutcome::failure(
                ToolError::new(E_INVALID_ARG, format!("未知工具：{name}")),
                format!("未知工具：{name}"),
            );
        }
        let spec = crate::tools::build_registry()
            .into_iter()
            .find(|s| s.name == name);
        let Some(spec) = spec else {
            return ToolOutcome::failure(
                ToolError::new(E_INVALID_ARG, format!("未知工具：{name}")),
                format!("未知工具：{name}"),
            );
        };
        if crate::safety::tier::decide_gate(spec.capability, self.tier)
            == crate::safety::tier::GateDecision::Filtered
        {
            return ToolOutcome::failure(
                ToolError::tool_blocked(format!(
                    "权限档排除：{name}（当前档 {tier}）",
                    tier = tier_label(self.tier)
                )),
                format!("权限档排除：{name}"),
            );
        }
        let args: serde_json::Value = match serde_json::from_str(args_json) {
            Ok(v) => v,
            Err(e) => {
                return ToolOutcome::failure(
                    ToolError::invalid_arg(format!("工具 {name} 参数不是合法 JSON：{e}")),
                    format!("工具 {name} 参数解析失败"),
                );
            }
        };
        match name {
            "read" => run_typed(&self.ctx, &args, read::run),
            "write" => run_typed(&self.ctx, &args, write::run),
            "edit" => run_typed(&self.ctx, &args, edit::run),
            "list_files" => run_typed(&self.ctx, &args, list_files::run),
            "grep" => run_typed(&self.ctx, &args, grep::run),
            other => ToolOutcome::failure(
                ToolError::new(E_INVALID_ARG, format!("未知工具：{other}")),
                format!("未知工具：{other}"),
            ),
        }
    }
}

/// 参数解析 + 调用 + 失败兜底的模板（解析失败 → E_INVALID_ARG 结果回喂）。
fn run_typed<A: serde::de::DeserializeOwned>(
    ctx: &ToolsContext,
    args: &serde_json::Value,
    f: fn(&ToolsContext, &A) -> ToolOutcome,
) -> ToolOutcome {
    match serde_json::from_value::<A>(args.clone()) {
        Ok(parsed) => f(ctx, &parsed),
        Err(e) => ToolOutcome::failure(
            ToolError::invalid_arg(format!("工具参数校验失败：{e}")),
            "工具参数校验失败",
        ),
    }
}

fn tier_label(tier: PermissionTier) -> &'static str {
    match tier {
        PermissionTier::Plan => "plan",
        PermissionTier::ConfirmEach => "confirm_each",
        PermissionTier::AutoEdit => "auto_edit",
        PermissionTier::FullAccess => "full_access",
    }
}

impl CancellableExecutor for ToolsetExecutor {
    fn execute<'a>(
        &'a self,
        _call_id: &'a str,
        name: &'a str,
        args_json: &'a str,
        _token: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async move {
            let outcome = self.dispatch(name, args_json);
            let model_text = compact_text(&outcome);
            ToolExecution::Completed {
                ok: outcome.ok,
                ui_summary: outcome.ui_summary,
                error: outcome.error.map(|e| FrameError::new(e.code, e.message)),
                model_text,
            }
        })
    }
}

/// 供测试/未来 command 工具复用的取消感知骨架：在闭包外包一层 select!，
/// token 触发即放弃闭包（由实现自行收割资源后返回 Cancelled）。
pub async fn with_cancel<F: Future<Output = ToolExecution>>(
    token: &CancellationToken,
    fut: F,
    executed_message: impl FnOnce() -> String,
) -> ToolExecution {
    tokio::select! {
        biased;
        _ = token.cancelled() => ToolExecution::Cancelled {
            executed: true,
            message: executed_message(),
        },
        result = fut => result,
    }
}

/// Arc 别名（RunLoop 参数签名用）。
pub type SharedExecutor = Arc<dyn CancellableExecutor>;
