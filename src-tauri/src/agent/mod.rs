//! agent（§2.2/§5.1）：RunManager、run 状态机与 RunHandle。
//!
//! 本切片承载 M1 核心链路的命令面依赖：同会话互斥（E_SESSION_RUNNING）、
//! cancel_run 置位（§5.3）、run 终态摘除。steer/followUp 队列、审批挂起恢复
//! 的宿主面、对账快照命令随对应票接入。

pub mod run_manager;

pub use run_manager::{RunHandle, RunManager, RunState, RunStatus};
