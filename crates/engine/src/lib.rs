//! CraftWave 引擎（craftwave-engine）：代理运行、工具、存储、知识、MCP、git。
//!
//! 零框架依赖纯库——由 Tauri 壳（现）与 GPUI 壳（gpui-rewrite）共享。
//! 引擎的应用级共享状态见 [`app_state::AppState`]（壳负责构造并注入
//! [`util::events::EventSink`] 的生产实现）。
//!
//! 搬移自 src-tauri/src（gpui-rewrite 阶段 1，git mv 保留历史）：
//! 命令壳（#[tauri::command]）剥离留在壳侧，`*_core` 与类型随引擎走。

pub mod agent;
pub mod error;
pub mod app_state;
pub mod backend;
pub mod followup;
pub mod exit_confirm;
pub mod git;
pub mod knowledge;
pub mod mcp;
pub mod providers;
pub mod safety;
pub mod session_roots;
pub mod storage;
pub mod run_persistence;
pub mod session_log;
pub mod usage;
pub mod tools;
pub mod util;

