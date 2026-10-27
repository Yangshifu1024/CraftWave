//! 工具系统：注册表 + 各工具实现 + 批次调度（§8.1）。
//!
//! M1 第二切片（14 号票）：在五文件类工具（11 号票）之上接线
//! command/plan/ask 三工具与 suggest 后台小调用（非模型工具，不进注册表，
//! 由 host 在 run 收尾发起）。审批门与工具卡片 UI 接线同票落地。
//!
//! 双通道结果（§8.2）：工具返回 [`ToolOutcome`]，模型通道 compact 文本经
//! [`compact_text`] 生成（成功头 4KB + 尾 8KB 截断；失败 error code+message）。

pub mod ask;
pub mod command;
pub mod edit;
pub mod grep;
pub mod list_files;
pub mod plan;
pub mod process;
pub mod read;
pub mod write;

use std::path::PathBuf;

use serde::Serialize;

use crate::safety::safe_join::PathGuard;
use crate::safety::tier::{Capability, GateDecision, PermissionTier, decide_gate};
use crate::util::errors::ToolError;

/// 工具执行上下文：roots（项目主目录 + 数据目录）由调用方构造。
/// M1 无 extra_roots（§9.1）。
#[derive(Debug, Clone)]
pub struct ToolsContext {
    /// 项目主目录（canonical），相对路径的解析基准与首个 root。
    pub project_root: PathBuf,
    /// 数据目录（canonical）。
    pub data_root: PathBuf,
}

impl ToolsContext {
    pub fn new(project_root: PathBuf, data_root: PathBuf) -> Self {
        Self {
            project_root,
            data_root,
        }
    }

    /// 构造路径守卫（roots canonicalize 失败时返回参数错误）。
    pub fn guard(&self) -> Result<PathGuard, ToolError> {
        PathGuard::new(vec![self.project_root.clone(), self.data_root.clone()])
    }
}

/// 工具注解（镜像 MCP annotations 的 P0 子集）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    pub read_only_hint: bool,
    pub destructive_hint: bool,
    pub idempotent_hint: bool,
    pub open_world_hint: bool,
}

/// 工具规格（§8.1）。`params_schema` 为 JSON Schema 对象，
/// 约束 `additionalProperties: false`。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    /// 合法字符 [a-z_][a-z0-9_]{0,63}。
    pub name: &'static str,
    /// 面向模型，≤800 字符。
    pub description: String,
    pub capability: Capability,
    pub params_schema: serde_json::Value,
    pub annotations: ToolAnnotations,
}

/// 工具结果（§3.2 ToolOutcomeJson 镜像）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutcome {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ToolError>,
    /// 一行人话（UI 通道，不进模型文本）。
    pub ui_summary: String,
    /// 全量细节（toolres 边车；warnings 仅 UI 显示不进模型文本，§8.2）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

impl ToolOutcome {
    pub fn success(data: serde_json::Value, ui_summary: impl Into<String>) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
            ui_summary: ui_summary.into(),
            detail: None,
        }
    }

    pub fn failure(err: ToolError, ui_summary: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(err),
            ui_summary: ui_summary.into(),
            detail: None,
        }
    }

    /// 附带 detail（全量信息进边车）。
    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

impl From<ToolError> for ToolOutcome {
    /// 无更具体 UI 话术时的兜底转换（错误码即摘要）。
    fn from(err: ToolError) -> Self {
        let summary = if err.message.is_empty() {
            err.code.to_string()
        } else {
            format!("{}：{}", err.code, err.message)
        };
        Self::failure(err, summary)
    }
}

/// 模型通道 compact 文本（§8.2）：成功序列化 data（头 4KB + 尾 8KB 截断）；
/// 失败输出 error code+message。warnings/detail 不进模型文本。
pub fn compact_text(outcome: &ToolOutcome) -> String {
    if !outcome.ok {
        let Some(err) = &outcome.error else {
            return "unknown error".to_string();
        };
        return format!("{} {}", err.code, err.message);
    }
    let body = outcome
        .data
        .as_ref()
        .map(|d| serde_json::to_string(d).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_default();
    truncate_middle(&body, 4 * 1024, 8 * 1024)
}

/// 头 `head` + 尾 `tail` 字节截断，中间以省略标记连接。按 UTF-8 字符边界切。
fn truncate_middle(s: &str, head: usize, tail: usize) -> String {
    if s.len() <= head + tail {
        return s.to_string();
    }
    let head_cut = floor_char_boundary(s, head);
    let tail_cut_start = floor_char_boundary(s, s.len() - tail);
    format!(
        "{}…[截断 {} 字节]…{}",
        &s[..head_cut],
        s.len() - head_cut - (s.len() - tail_cut_start),
        &s[tail_cut_start..]
    )
}

/// 向下取最近 UTF-8 字符边界。
fn floor_char_boundary(s: &str, mut index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    while index > 0 && !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// 工具名称与顺序钉死（§8.1：集成测试防漂移）。M1 注册表 8 个
/// （§8.3 P0 9 个中的模型可见子集；suggest 仅 run 尾调用不开放模型）。
pub const TOOL_NAMES: [&str; 8] = [
    "read",
    "write",
    "edit",
    "list_files",
    "grep",
    "command",
    "plan",
    "ask",
];

/// 静态注册表（§8.1：启动时构建）。顺序与 [`TOOL_NAMES`] 一致。
pub fn build_registry() -> Vec<ToolSpec> {
    vec![
        read::spec(),
        write::spec(),
        edit::spec(),
        list_files::spec(),
        grep::spec(),
        command::spec(),
        plan::spec(),
        ask::spec(),
    ]
}

/// 三处同源过滤的单一实现（§8.1，FR-05-01）：按档位过滤工具表。
/// plan 档 FileWrite/Network 不出现；其余档位全保留——
/// NeedsApproval 的审批在门控执行，不影响工具是否暴露给模型。
pub fn filter_for_tier(specs: Vec<ToolSpec>, tier: PermissionTier) -> Vec<ToolSpec> {
    specs
        .into_iter()
        .filter(|spec| decide_gate(spec.capability, tier) != GateDecision::Filtered)
        .collect()
}

#[cfg(test)]
#[path = "ask_test.rs"]
mod ask_test;

#[cfg(test)]
#[path = "command_test.rs"]
mod command_test;

#[cfg(test)]
#[path = "compact_test.rs"]
mod compact_test;

#[cfg(test)]
#[path = "plan_test.rs"]
mod plan_test;
