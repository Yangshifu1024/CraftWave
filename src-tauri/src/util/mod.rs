//! 通用小件：原子写（storage 侧）、ids、time、错误码、编码解码器。
//!
//! 依赖方向：util 被所有层依赖，自身不依赖其他业务模块（§2.2）。

pub mod encoding;
pub mod errors;

#[cfg(test)]
pub mod test_support;
