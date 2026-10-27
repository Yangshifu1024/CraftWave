//! 【S2+S3 Spike，用后即弃】rig 流事件保真与手动工具循环的原型验证模块。
//!
//! 问题与结论见 docs/specs/m1-mvp/tasks/05-spike-s2s3-rig-stream-loop.md；
//! 正式实现应落在 backend/rig_impl.rs（防腐层 §4）与 host（帧转发），
//! 本模块只回答「rig 0.45 / 0.46-rc.1 能不能按设计文档 §3.4/§4.3/§5.2 跑」。
//! 不挂进 tauri builder，不参与应用启动路径。

pub mod frames;
pub mod rig_bridge;
pub mod turn;

#[cfg(test)]
#[path = "frames_test.rs"]
mod frames_test;
