//! Provider 适配层（§2.2/§11）：CW schema v2 类型、内置预设、服务编排。
//!
//! 供应商（provider）指提供大模型对话 API 的服务方（OpenAI、DeepSeek、
//! 智谱等）。本模块把"用户配置了哪些供应商与模型"落成类型定义与读写服务。
//!
//! 模块职责（§2.2）：供应商解析（CW schema v2 → rig Model，后续票）、
//! Key 池（M2 实装，M1 单 Key 直用）、错误分类（FR-03-04，run 时接入）。
//! 本切片落地：schema v2 配置类型 + 归一化、内置供应商预设、
//! 自定义 OpenAI 兼容端点（baseUrl 覆写）。
//!
//! 2026-10-28 用户决策废除 keyring（keyring = 操作系统级密钥库，如 macOS
//! 钥匙串；§9.4 改写）：API Key 明文存 config.json（0600 权限收紧在
//! ConfigStore，即仅文件属主可读写）。

pub mod http;
pub mod presets;
pub mod probe;
pub mod reasoning;
pub mod service;
pub mod types;

#[cfg(test)]
#[path = "types_test.rs"]
mod types_test;

#[cfg(test)]
#[path = "presets_test.rs"]
mod presets_test;

#[cfg(test)]
#[path = "service_test.rs"]
mod service_test;
