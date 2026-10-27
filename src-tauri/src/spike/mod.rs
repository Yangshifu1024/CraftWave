//! 【S2+S3+S5 Spike，用后即弃】rig 流事件保真、手动工具循环与取消传播的原型验证模块。
//!
//! Spike（原型验证）：为回答某个技术疑点而短期编写的验证代码——先做原型
//! 证明方案可行，把结论沉淀进票与设计文档，之后另写正式实现，原型本身
//! 用后即弃。本目录是 S2/S3/S5 三个已完结 Spike 的代码留存（早期原型验证
//! 代码的保留存档），不再演进，供回溯验证过程。
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
