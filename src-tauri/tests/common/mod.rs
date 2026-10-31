//! 集成测试公共模块：本地内存 mock SSE 服务（OpenAI Chat / Anthropic / OpenAI Responses 三方言）。
//!
//! backend_e2e.rs 内部的 fixture 自包含，不与本模块共享类型；
//! 本模块供 protocol_dialects_e2e.rs 与 session_logs_e2e.rs 使用。

// 各集成测试二进制独立链接本模块，未用到的导出属预期。
#![allow(dead_code)]

pub mod mock_anthropic;
pub mod mock_openai;
pub mod mock_responses;
