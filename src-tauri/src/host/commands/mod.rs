//! 命令实现（校验/转调），按域分文件（§2.2 host/commands/）。
//!
//! 命令注册与 IPC manifest 由 crate 根 lib.rs 的 `ipc_commands!` 宏同源展开
//! （§12.3/NFR-12 防漂移，全命令统一收编）；本模块只承载 manifest 的事件帧键面。

pub mod config;
pub mod provider;
pub mod session_tree;

/// ChatFrame P0 全集 9 帧（§3.4；type 值与 TS 判别值一致）。
pub const FRAMES: &[&str] = &[
    "runStarted",
    "stepStarted",
    "textDelta",
    "reasoningDelta",
    "toolStarted",
    "toolArgs",
    "toolResult",
    "usage",
    "runEnded",
];

/// 事件键（§3.5 P0 18 键）。事件汇属后续票；本切片不发射事件，先占位空表。
pub const EVENTS: &[&str] = &[];
