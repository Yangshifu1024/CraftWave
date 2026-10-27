//! host 模块（§2.2）：IPC 层——命令实现（校验/转调）、事件汇、通知、托盘、菜单、退出拦截。
//!
//! 本切片（10 号票）落地：D14 错误形状（error.rs）、会话树两命令（commands/session_tree.rs）。
//! IPC manifest 单一事实源在 crate 根 lib.rs（`ipc_commands!` 宏）。

pub mod commands;
mod error;

pub use error::IpcError;
