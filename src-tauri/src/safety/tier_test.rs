//! 权限四档矩阵与 plan 档过滤测试（§8.5）。

use super::{Capability, GateDecision, PermissionTier, decide_gate};

// 断言：四档 × 能力判定矩阵（§8.5 全表），档位/能力枚举经 JSON
// （camelCase）往返不丢语义（跨 IPC 传输用）。
#[test]
fn permission_matrix_and_serde_roundtrip() {
    let tiers = [
        PermissionTier::Plan,
        PermissionTier::ConfirmEach,
        PermissionTier::AutoEdit,
        PermissionTier::FullAccess,
    ];
    // ReadOnly/Interactive/Meta 三类能力在全部四档都放行。
    for cap in [
        Capability::ReadOnly,
        Capability::Interactive,
        Capability::Meta,
    ] {
        for tier in tiers {
            assert_eq!(
                decide_gate(cap, tier),
                GateDecision::Allow,
                "{cap:?} @ {tier:?} 应放行"
            );
        }
    }
    // FileWrite 行——plan 过滤、confirm_each 审批、auto_edit/full_access 放行。
    assert_eq!(
        decide_gate(Capability::FileWrite, PermissionTier::Plan),
        GateDecision::Filtered
    );
    assert_eq!(
        decide_gate(Capability::FileWrite, PermissionTier::ConfirmEach),
        GateDecision::NeedsApproval
    );
    assert_eq!(
        decide_gate(Capability::FileWrite, PermissionTier::AutoEdit),
        GateDecision::Allow
    );
    assert_eq!(
        decide_gate(Capability::FileWrite, PermissionTier::FullAccess),
        GateDecision::Allow
    );
    // Network 行——plan 过滤、confirm_each/auto_edit 审批、full_access 放行。
    assert_eq!(
        decide_gate(Capability::Network, PermissionTier::Plan),
        GateDecision::Filtered
    );
    assert_eq!(
        decide_gate(Capability::Network, PermissionTier::ConfirmEach),
        GateDecision::NeedsApproval
    );
    assert_eq!(
        decide_gate(Capability::Network, PermissionTier::AutoEdit),
        GateDecision::NeedsApproval
    );
    assert_eq!(
        decide_gate(Capability::Network, PermissionTier::FullAccess),
        GateDecision::Allow
    );
    // 跨 IPC（runConfig.permissionTier）用，camelCase。
    let tier: PermissionTier = serde_json::from_str("\"confirmEach\"").expect("反序列化");
    assert_eq!(tier, PermissionTier::ConfirmEach);
    assert_eq!(
        serde_json::to_string(&Capability::FileWrite)
            .expect("序列化")
            .as_str(),
        "\"fileWrite\""
    );
}
