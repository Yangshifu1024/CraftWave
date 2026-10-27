//! backend（§2.2/§4）：rig 防腐层——Channel 帧生产、AgentRun 步进 RunLoop、
//! 审批挂起机制层、取消传播、wire 重建。
//!
//! 本模块是 S2/S3/S5 spike（`spike/`，验证原型，原样保留）的产品化落地：
//! - [`frames`]：ChatFrame P0 全集 9 帧（§3.4）+ 64ms 节流合并器；
//! - [`rig_bridge`]：rig 流事件 → 帧（callId 同源映射）；
//! - [`turn`]：单 turn 流式调用驱动（§4.3）；
//! - [`run_loop`]：AgentRun 步进 RunLoop（§5.2）+ CallTools 审批挂起机制层 + 取消收齐（§5.3）；
//! - [`exec`]：五文件类工具的执行器适配（tools → RunLoop 契约）；
//! - [`wire`]：wire 中立消息 → rig Message 重建（§6.2 M1 子集）。
//!
//! rig 升级只应改到本模块与 `providers/`（§4.1）。

pub mod ask;
pub mod exec;
pub mod frames;
pub mod gate;
pub mod rig_bridge;
pub mod run_loop;
pub mod turn;
pub mod wire;

#[cfg(test)]
#[path = "frames_test.rs"]
mod frames_test;

#[cfg(test)]
#[path = "gate_test.rs"]
mod gate_test;

#[cfg(test)]
#[path = "rig_bridge_test.rs"]
mod rig_bridge_test;

#[cfg(test)]
#[path = "exec_test.rs"]
mod exec_test;

#[cfg(test)]
#[path = "wire_test.rs"]
mod wire_test;
