//! 命令实现（校验/转调），按域分文件（§2.2 host/commands/）。
//!
//! 命令注册与 IPC manifest 由 crate 根 lib.rs 的 `ipc_commands!` 宏同源展开
//! （§12.3/NFR-12 防漂移，全命令统一收编）；本模块只承载 manifest 的事件帧键面。

pub mod chat;
pub mod config;
// 图片附件承载命令（17 号票，§3.3 #59h/#59i）
pub mod image;
pub mod project;
// 配额快照命令（21 号票，§3.3 #53）
pub mod quota;
// HTML 沙箱预览注册命令（14 号票，§3.3 #59g）
pub mod preview;
pub mod provider;
// 网络搜索连通测试命令（13 号票，§3.3 #59f）
pub mod search;
pub mod session;
pub mod session_tree;
// 用量统计命令（20 号票，§3.3 #52/#59k）
pub mod stats;
// 子代理命令与委派装配（10 号票，§3.3 #58/#59 + §5.9）
pub mod subagent;
pub mod system;
pub mod ui_state;
pub mod workspace;

/// macOS 交通灯精确摆位（依赖 AppKit，仅桌面 mac 编译）。
#[cfg(target_os = "macos")]
pub mod traffic_lights;

#[cfg(test)]
#[path = "chat_test.rs"]
mod chat_test;

#[cfg(test)]
#[path = "image_test.rs"]
mod image_test;

#[cfg(test)]
#[path = "system_test.rs"]
mod system_test;

#[cfg(test)]
#[path = "workspace_test.rs"]
mod workspace_test;

/// ChatFrame P0 全集 9 帧 + 【M2】压缩帧/摘要帧 + 【M2】子代理 4 帧
/// （§3.4；type 值与 TS 判别值一致）。
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
];

/// 事件键（§3.5 P0 子集 + 【M2】压缩三键 + 06 号票队列键 + 08 号票
/// mcp:status + 10 号票子代理六键 + 27 号票 session:title）：14 号票实装
/// 5 键（ask 协议 / plan / suggest / 会话用量）；17 号票补 run 终态三键与
/// notify/app 退出两键；04 号票补压缩生命周期三键；06 号票补 run:start /
/// run:inject / run:queue 三键；08 号票补 mcp:status；10 号票补 sub:*
/// 六键；27 号票补 session:title；其余键（run:retry / tool:* / menu:action）
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
