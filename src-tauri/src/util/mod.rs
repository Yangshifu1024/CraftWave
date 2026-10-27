//! 通用工具（§2.2 九模块之一）：原子写、ids、time、错误码、编码解码器、test_support（按需增量补齐）。
//!
//! 依赖方向：util 被所有层依赖，自身不依赖其他业务模块（§2.2）。

pub mod atomic;
pub mod encoding;
pub mod errors;

#[cfg(test)]
pub mod test_support;
