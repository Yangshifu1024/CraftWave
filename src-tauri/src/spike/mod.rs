//! 【S2+S3+S5 Spike，用后即弃】rig 流事件保真、手动工具循环与取消传播的原型验证模块。
//!
//! 问题与结论见 docs/specs/m1-mvp/tasks/05（S2+S3）与 08（S5 取消传播）号票；
//! 正式实现应落在 backend/rig_impl.rs（防腐层 §4）与 host（帧转发）、
//! tools（命令执行器），本模块只回答「rig 0.45 能不能按设计文档 §3.4/§4.3/§5.2/§5.3 跑」。
//! 不挂进 tauri builder，不参与应用启动路径。

pub mod cancel;
pub mod frames;
pub mod rig_bridge;
pub mod turn;

#[cfg(test)]
#[path = "frames_test.rs"]
mod frames_test;

#[cfg(test)]
#[path = "cancel_test.rs"]
mod cancel_test;
