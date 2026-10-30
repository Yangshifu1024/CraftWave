//! 安全层：权限档过滤、命令围栏（fence）、safe_join 路径边界、删除守卫、SSRF 防护。
//!
//! 名词解释：权限档（tier）是把会话授权力度分为四档的开关（见 tier.rs）；
//! 命令围栏（fence）是对 shell 命令做危险度判定的规则集（见 fence.rs）；
//! safe_join 是把模型给出的路径约束在允许目录内的路径校验（见 safe_join.rs）；
//! SSRF 防护是 web 抓取类工具（web_fetch/http_request，12 号票）对目标地址
//! 的内网/回环访问拦截（见 ssrf.rs，§9.2）。
//!
//! 依赖方向：tools → safety（§2.2）。fence/safe_join/tier 只提供纯判定逻辑，
//! 不做 IO 审批（审批门属 run loop 集成）；ssrf 承载 DNS 与 HTTP 客户端
//! 原语（§9.2 管道本质是网络面校验，与 reqwest 绑定，13 号票 web_search
//! 复用）。

pub mod fence;
pub mod safe_join;
pub mod ssrf;
pub mod tier;
