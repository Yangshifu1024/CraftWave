//! 权限四档与能力分级判定（权限 × 能力门控矩阵）。
//!
//! 矩阵只回答「该能力在该档位下能否执行/是否要审批/是否被过滤」；
//! 灾难级（fence 黑名单、删除守卫命中）任何档位均拦截——由
//! [`crate::safety::fence`] 叠加判定，本模块不感知命令内容。

use serde::{Deserialize, Serialize};

/// 权限四档。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum PermissionTier {
    /// 计划档：只读白名单，FileWrite/Network 工具从工具表过滤。
    Plan,
    /// 逐次审批：写/网络需用户确认（diff 预览）。
    ConfirmEach,
    /// 自动写：roots 内直写，网络仍审批。
    AutoEdit,
    /// 全权：网络放行（fence=Block 仍拦），路径边界不解除。
    FullAccess,
}

/// 工具能力五级：权限矩阵按能力分级，不按具体工具逐个配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Capability {
    /// 只读（read/grep/list_files 等），全档放行。
    ReadOnly,
    /// 写文件（write/edit），plan 档过滤、confirm_each 档审批。
    FileWrite,
    /// 交互（ask 向用户提问），全档放行。
    Interactive,
    /// 网络/命令（command），plan 档过滤、非 full_access 档审批。
    Network,
    /// 元能力（plan 计划维护），全档放行。
    Meta,
}

/// 门控判定结果（门控矩阵 + plan 档过滤语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GateDecision {
    /// 放行（或 roots 内直写——路径边界由 safe_join 独立保证）。
    Allow,
    /// 需用户审批（审批门属 run loop 集成）。
    NeedsApproval,
    /// 从工具表过滤（plan 档 FileWrite/Network）。
    Filtered,
    /// 硬拦（矩阵层面不产生，仅 fence 黑名单叠加；保留枚举完整性）。
    Blocked,
}

/// 权限 × 工具门控矩阵的单一实现。
pub fn decide_gate(cap: Capability, tier: PermissionTier) -> GateDecision {
    use Capability as C;
    use GateDecision as G;
    use PermissionTier as T;
    match (cap, tier) {
        // ReadOnly / Interactive(ask) / Meta：全档放行。
        (C::ReadOnly, _) | (C::Interactive, _) | (C::Meta, _) => G::Allow,
        // FileWrite：plan 过滤；confirm_each 审批；auto_edit+ roots 内直写。
        (C::FileWrite, T::Plan) => G::Filtered,
        (C::FileWrite, T::ConfirmEach) => G::NeedsApproval,
        (C::FileWrite, T::AutoEdit | T::FullAccess) => G::Allow,
        // Network：plan 过滤；confirm_each/auto_edit 审批；full_access 放行
        // （fence=Block 仍拦）。
        (C::Network, T::Plan) => G::Filtered,
        (C::Network, T::ConfirmEach | T::AutoEdit) => G::NeedsApproval,
        (C::Network, T::FullAccess) => G::Allow,
    }
}

#[cfg(test)]
#[path = "tier_test.rs"]
mod tier_test;
