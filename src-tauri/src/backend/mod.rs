//! backend 模块：面向第三方模型库 rig 的"防腐层"（技术设计文档 §2.2/§4）。
//!
//! 名词解释：rig 是本项目使用的 Rust 大模型编排库（负责模型调用与工具
//! 调用循环）；防腐层指隔离第三方库的适配代码——rig 升级或更换时，只需要
//! 改本模块与 `providers/`，其余代码不受影响（§4.1）。
//!
//! 本模块由早期验证原型（`spike/` 目录下的 S2/S3/S5 试验代码，验证可行性
//! 用，原样保留供对照）产品化而来，包含以下子模块：
//! - [`frames`]：帧协议全集（ChatFrame 九种帧，§3.4）。帧（frame）是后端
//!   推送给前端的一条事件消息，界面的打字机效果、工具卡片都由帧驱动；
//!   同时提供 64 毫秒节流合并器（把密集的小增量合并出帧，避免刷屏）；
//! - [`rig_bridge`]：把 rig 的流式事件转换成帧，并维护"模型调用 id ↔ 帧面
//!   id"的同一映射（保证工具的开始/参数/结果三类帧能互相配对）；
//! - [`turn`]：单轮流式调用的驱动（§4.3。turn = 一次"请求模型并收到完整
//!   回复"的过程）；
//! - [`run_loop`]：run 主循环（§5.2）——步进 rig 的 AgentRun 状态机、生产
//!   帧、处理工具批的审批挂起与取消收齐（§5.3）；
//! - [`exec`]：工具执行器适配——把工具注册表接到主循环的执行契约上；
//! - [`gate`]：审批门——工具执行前的安全判定策略（放行/挂起审批/硬拦）；
//! - [`ask`]：询问与审批的载荷协议（模型向用户提问、请求批准时的数据结构）；
//! - [`wire`]：把存储层的 wire 中立消息重建为 rig 的消息对象（§6.2 当前
//!   阶段子集；wire 中立消息 = 存储层定义的、不依赖具体模型 SDK 的通用
//!   消息格式）；
//! - [`estimator`]：请求 token 估算器（§6.2 每请求预算检查的估算面，
//!   Q18：宁高勿低启发式，不依赖 usage 帧）；
//! - [`compaction`]：上下文压缩编排（FR-03-05，04 号票）——阈值判定、
//!   压缩范围计算、增量/分块摘要（Q31/Q38 共用总超时）与原子提交；
//! - [`watchdog`]：监督看门狗（FR-03-07，37 号票，§5.7）——工具结果滑动
//!   窗口与四类信号的 turn 边界评估（重复失败/空转的纠偏与终止）、模型流
//!   停滞计时（仅请求阶段计时、有效增量刷新，Q33 边界）。

pub mod ask;
pub mod compaction;
pub mod estimator;
pub mod exec;
pub mod frames;
pub mod gate;
pub mod rig_bridge;
pub mod run_loop;
pub mod turn;
pub mod watchdog;
pub mod wire;

// 以下为各子模块的测试文件挂载：测试代码在独立文件中，仅 cargo test 时编译。
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

#[cfg(test)]
#[path = "web_gate_e2e_test.rs"]
mod web_gate_e2e_test;

#[cfg(test)]
#[path = "plan_reminder_test.rs"]
mod plan_reminder_test;
