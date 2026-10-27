//! Provider 适配层（§2.2/§11）：CW schema v2 类型、内置预设、keyring 提交协议。
//!
//! 模块职责（§2.2）：供应商解析（CW schema v2 → rig Model，后续票）、
//! Key 池（M2 实装，M1 单 Key 直用）、错误分类（FR-03-04，run 时接入）。
//! 本切片落地：schema v2 配置类型 + 归一化、内置供应商预设、
//! keyring 提交协议（§9.4）、自定义 OpenAI 兼容端点（baseUrl 覆盖）。

pub mod keyring;
pub mod presets;
pub mod service;
pub mod types;

#[cfg(test)]
#[path = "types_test.rs"]
mod types_test;

#[cfg(test)]
#[path = "presets_test.rs"]
mod presets_test;

#[cfg(test)]
#[path = "keyring_test.rs"]
mod keyring_test;

#[cfg(test)]
#[path = "service_test.rs"]
mod service_test;
