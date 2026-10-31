//! 命令实现（校验/转调），按域分文件（host/commands/）。
//!
//! 命令注册与 IPC manifest 由 crate 根 lib.rs 的 `ipc_commands!` 宏同源展开
//! （防漂移，全命令统一收编）；本模块只承载 manifest 的事件帧键面。

// 产物面板与编辑器打开
pub mod artifacts;
pub mod chat;
pub mod config;
pub mod git_cmds;
pub mod knowledge_cmds;
// 图片附件承载命令
pub mod image;
// 日志命令（读取/清单/打开目录）
pub mod logs;
pub mod mcp_cmds;
pub mod project;
// 配额快照命令
pub mod quota;
// HTML 沙箱预览注册命令
pub mod preview;
pub mod provider;
// 网络搜索连通测试命令
pub mod search;
pub mod session;
pub mod session_tree;
// 用量统计命令
pub mod stats;
// 子代理命令与委派装配
pub mod subagent;
pub mod system;
pub mod ui_state;
// 更新器支撑命令（状态门/安装门/主动暂停）
pub mod updater;
pub mod workspace;

/// macOS 交通灯精确摆位（依赖 AppKit，仅桌面 mac 编译）。
#[cfg(target_os = "macos")]
pub mod traffic_lights;

#[cfg(test)]
#[path = "chat_test.rs"]
mod chat_test;

#[cfg(test)]
#[path = "artifacts_test.rs"]
mod artifacts_test;

#[cfg(test)]
#[path = "image_test.rs"]
mod image_test;

#[cfg(test)]
#[path = "system_test.rs"]
mod system_test;

#[cfg(test)]
#[path = "updater_test.rs"]
mod updater_test;

#[cfg(test)]
#[path = "workspace_test.rs"]
mod workspace_test;

/// ChatFrame P0 全集 9 帧 + 压缩帧/摘要帧 + 子代理 4 帧 +
/// toolAudit 审计帧（type 值与 TS 判别值一致）。
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
    "compactDone",
    "runSnapshot",
    "subStarted",
    "subFrame",
    "subReport",
    "subDone",
    "toolAudit",
];

/// 事件键：ask 协议 / plan / suggest / 会话用量 / run 终态三键 /
/// notify 与 app 退出两键 / 压缩生命周期三键 / run:start /
/// run:inject / run:queue / mcp:status / sub:*
/// 六键 / session:title；其余键（run:retry / tool:* / menu:action）
/// 暂未实装。
pub const EVENTS: &[&str] = &[
    "ask:opened",
    "ask:closed",
    "plan:update",
    "run:suggestions",
    "tokens:update",
    "run:done",
    "run:error",
    "run:cancelled",
    "run:start",
    "run:inject",
    "run:queue",
    "notify:activate",
    "app:exit_requested",
    "run:compacting",
    "run:compacted",
    "run:compact_failed",
    "mcp:status",
    "sub:spawn",
    "sub:step",
    "sub:report",
    "sub:usage",
    "sub:done",
    "sub:error",
    "session:title",
];
