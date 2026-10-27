//! 安全层：权限档过滤、命令围栏（fence）、safe_join 路径边界、删除守卫、SSRF（P1）。
//!
//! 名词解释：权限档（tier）是把会话授权力度分为四档的开关（见 tier.rs）；
//! 命令围栏（fence）是对 shell 命令做危险度判定的规则集（见 fence.rs）；
//! safe_join 是把模型给出的路径约束在允许目录内的路径校验（见 safe_join.rs）；
//! 删除守卫、SSRF 防护为后续切片。
//!
//! 依赖方向：tools → safety（§2.2）。本层不做 IO 审批（审批门属 run loop 集成），
//! 只提供纯判定逻辑。

pub mod fence;
pub mod safe_join;
pub mod tier;
