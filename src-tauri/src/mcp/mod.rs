//! MCP 客户端接入。
//!
//! 模块结构（与 host 同层的独立域模块）：
//! - [`config`]：双作用域 mcp.json（全局 `~/.craftwave/mcp.json` + 项目
//!   `<项目主目录>/.craftwave/mcp.json`，同名覆盖）与未知字段 round-trip；
//! - [`trust`]：TOFU 启动信任（指纹 = `(scope, projectId?, name,
//!   canonicalConfigPath, sha256(config))`，决策落 `~/.craftwave/trust/mcp.json`）
//!   与工具级动态许可（`trust/mcp-tools.json`，绑定服务器指纹 + 原始
//!   工具名 + 已确认输入 schema 哈希）；
//! - [`tools`]：工具清单动态同步（tools/list → 归一化 ToolSpec + 碰撞
//!   后缀 + 反向映射还原）与进程级 [`tools::McpToolRegistry`] 单例；
//! - [`pool`]：连接池（LRU ≤16、Starting→Ready→Stopped/Error 状态机、
//!   generation 防旧握手复活、断线指数退避重连、结果未知不重放）；
//! - [`commands`]： –47 的 8 个 `mcp_*` 命令与 TOFU 询问槽。
//!
//! 断连语义（2026-10-29 用户确认）：连接可自动恢复，但**已发出
//! 而结果未知的调用不自动重放**——`pool` 返回可辨识的
//! `E_MCP_RESULT_UNKNOWN` 结果（"可能已执行"），由主代理先核查实际结果再
//! 决定是否重试；read_only 声明不豁免。

pub mod commands;
pub mod config;
pub mod pool;
pub mod tools;
pub mod trust;

#[cfg(test)]
mod config_test;

#[cfg(test)]
mod tools_test;

#[cfg(test)]
mod trust_test;

#[cfg(test)]
mod pool_test;

#[cfg(test)]
#[path = "commands_test.rs"]
mod commands_test;

/// 测试基建（本地 fixture echo server）。仅测试构建编译——
/// 生产二进制（cargo build 不激活 dev-dependencies 的 rmcp server feature）
/// 不含 server 面。
#[cfg(test)]
pub mod fixture;
