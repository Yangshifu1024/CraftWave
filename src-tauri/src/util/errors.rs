//! 工具错误与错误码（§3.6 错误码注册表）。
//!
//! 放 util 而非 tools：safety（safe_join/围栏）与 tools 都要返回它，
//! util 是被所有层依赖的最底层，避免 safety 反向依赖 tools 造成环。

use std::fmt;

/// 工具/安全层错误的统一载体：`code` 必须取自 §3.6 错误码注册表。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolError {
    pub code: &'static str,
    pub message: String,
}

impl ToolError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// E_INVALID_ARG：参数校验失败（通用）。
    pub fn invalid_arg(message: impl Into<String>) -> Self {
        Self::new("E_INVALID_ARG", message)
    }

    /// E_NOT_FOUND：目标不存在（通用）。
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("E_NOT_FOUND", message)
    }

    /// E_PATH_OUTSIDE_ROOTS：路径越界（FR-05-06）。
    pub fn path_outside_roots(message: impl Into<String>) -> Self {
        Self::new("E_PATH_OUTSIDE_ROOTS", message)
    }

    /// E_FENCE_BLOCKED：命令围栏 Block（FR-05-05）。
    pub fn fence_blocked(message: impl Into<String>) -> Self {
        Self::new("E_FENCE_BLOCKED", message)
    }

    /// E_TOOL_BLOCKED：权限档排除（FR-05-01）。
    pub fn tool_blocked(message: impl Into<String>) -> Self {
        Self::new("E_TOOL_BLOCKED", message)
    }

    /// E_EDIT_NO_MATCH / E_EDIT_AMBIGUOUS / E_EDIT_VERSION_STALE（§8.4）。
    pub fn edit_no_match(message: impl Into<String>) -> Self {
        Self::new("E_EDIT_NO_MATCH", message)
    }

    pub fn edit_ambiguous(message: impl Into<String>) -> Self {
        Self::new("E_EDIT_AMBIGUOUS", message)
    }

    pub fn edit_version_stale(message: impl Into<String>) -> Self {
        Self::new("E_EDIT_VERSION_STALE", message)
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ToolError {}

impl From<std::io::Error> for ToolError {
    /// io 错误映射：NotFound → E_NOT_FOUND；其余归 E_INTERNAL_STATE 兜底。
    /// 注册表（§3.6）无通用 IO 码，映射决策见 11 号票 Answer。
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => Self::not_found(format!("文件或目录不存在：{e}")),
            _ => Self::new("E_INTERNAL_STATE", format!("文件系统操作失败：{e}")),
        }
    }
}
