//! IPC 统一错误形状（D14）：`{ code, message, detail? }`。
//!
//! code 取值须落在 §3.6 错误码注册表；`E_STORAGE_IO` 为注册表当前未覆盖的
//! 磁盘 IO 失败承载（10 号票 Answer 记录，待后续票补 §3.6）。

use serde::Serialize;
use serde_json::Value;

use crate::storage::StorageError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

impl IpcError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            detail: None,
        }
    }

    /// E_INVALID_ARG：参数校验失败（§3.6）。
    pub fn invalid_arg(message: impl Into<String>) -> Self {
        Self::new("E_INVALID_ARG", message)
    }

    /// E_NOT_FOUND：目标不存在（§3.6）。
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("E_NOT_FOUND", message)
    }

    /// E_SESSION_REV：树同步基线过期（§3.6/§6.4）。
    pub fn session_rev(message: impl Into<String>) -> Self {
        Self::new("E_SESSION_REV", message)
    }

    /// E_STORAGE_IO：磁盘 IO 失败（§3.6 暂缺，见模块注释）。
    pub fn storage_io(message: impl Into<String>) -> Self {
        Self::new("E_STORAGE_IO", message)
    }
}

impl From<StorageError> for IpcError {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::RevisionConflict { expected, actual } => Self::session_rev(format!(
                "树同步基线过期：baseRevision={expected}，当前 revision={actual}"
            )),
            StorageError::NotFound(id) => Self::not_found(format!("会话不存在：{id}")),
            StorageError::InvalidArg(msg) => Self::invalid_arg(msg),
            StorageError::Io(msg) => Self::storage_io(msg),
        }
    }
}

#[cfg(test)]
#[path = "error_test.rs"]
mod error_test;
