//! 安全层：权限档过滤、命令围栏（fence）、safe_join 路径边界、删除守卫、SSRF（P1）。
//!
//! 依赖方向：tools → safety（§2.2）。本层不做 IO 审批（审批门属 run loop 集成），
//! 只提供纯判定逻辑。

pub mod fence;
pub mod safe_join;
pub mod tier;
