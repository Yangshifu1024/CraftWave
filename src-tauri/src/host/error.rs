//! storage 错误 → IPC 错误的映射（host 层职责）。
//!
//! `IpcError` 形状本身定义在 `util::errors`（util 为最底层，providers 亦依赖）；
//! 本模块只承载 `From<StorageError>` 映射——它必须位于 storage 之上，不能下沉。
//!
//! code 取值须落在 §3.6 错误码注册表；`E_STORAGE_IO` 为注册表当前未覆盖的
//! 磁盘 IO 失败承载（10 号票 Answer 记录，待后续票补 §3.6）。

use crate::storage::StorageError;

use crate::util::errors::IpcError;

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
