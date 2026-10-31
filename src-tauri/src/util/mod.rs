//! 通用工具（§2.2 九模块之一）：原子写、ids、time、错误码、编码解码器、test_support（按需增量补齐）。
//!
//! 依赖方向：util 被所有层依赖，自身不依赖其他业务模块（§2.2）。
//! errors 模块承载两套 §3.6 错误载体（语义不同，并存于同一错误模块）：
//! `ToolError`（工具/安全域错误，ToolOutcome 承载）与 `IpcError`
//! （IPC 命令统一 reject 负载，host 与 providers 共用，故下沉到 util）。

pub mod atomic;
pub mod encoding;
pub mod errors;
pub mod events;
pub mod ids;
// 日志脱敏（32 号票，D12 复刻点 / NFR-09）：密钥名单替换 + 字符串兜底 +
// 图片本体摘要化——日志落盘前的最后一道闸。
pub mod redact;

#[cfg(test)]
pub mod test_support;
