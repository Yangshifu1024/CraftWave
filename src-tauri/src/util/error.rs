//! IPC 错误统一负载（§3.1/§3.6）：`{ code, message, detail? }`，camelCase。
//!
//! 命令层一律返回 `Result<T, IpcError>`；Tauri 把 Serialize 错误直接序列化给
//! 前端 reject。code 全集见技术设计文档 §3.6 错误码注册表。

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const E_INVALID_ARG: &str = "E_INVALID_ARG";
pub const E_NOT_FOUND: &str = "E_NOT_FOUND";
pub const E_KEYRING: &str = "E_KEYRING";
pub const E_INTERNAL: &str = "E_INTERNAL";

/// 统一错误负载（D14：字段 camelCase）。
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    pub fn with_detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn invalid_arg(message: impl Into<String>) -> Self {
        Self::new(E_INVALID_ARG, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(E_NOT_FOUND, message)
    }

    pub fn keyring(message: impl Into<String>) -> Self {
        Self::new(E_KEYRING, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(E_INTERNAL, message)
    }
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for IpcError {}
