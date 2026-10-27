//! 命令实现（校验/转调），按域分文件（§2.2 host/commands/）。
//!
//! 命令注册与 IPC manifest 由 crate 根 lib.rs 的 `ipc_commands!` 宏同源展开
//! （§12.3/NFR-12 防漂移，全命令统一收编）；本模块只承载 manifest 的事件帧键面。

pub mod chat;
pub mod config;
pub mod project;
pub mod provider;
pub mod session;
pub mod session_tree;
pub mod system;
pub mod ui_state;
pub mod workspace;

#[cfg(test)]
#[path = "chat_test.rs"]
mod chat_test;

#[cfg(test)]
#[path = "workspace_test.rs"]
mod workspace_test;

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

/// 事件键（§3.5 P0 子集）：14 号票实装 5 键（ask 协议 / plan / suggest /
/// 会话用量）；17 号票补 run 终态三键与 notify/app 退出两键；其余键
/// （run:start / run:retry / run:inject / tool:* / session:title / menu:action）
/// 随对应票填充。
pub const EVENTS: &[&str] = &[
    "ask:opened",
    "ask:closed",
    "plan:update",
    "run:suggestions",
    "tokens:update",
    "run:done",
    "run:error",
    "run:cancelled",
    "notify:activate",
    "app:exit_requested",
];
