//! 权限四档矩阵与 plan 档过滤测试（§8.5）。

use super::{Capability, GateDecision, PermissionTier, decide_gate};

#[test]
fn readonly_interactive_meta_pass_all_tiers() {
    for cap in [
        Capability::ReadOnly,
        Capability::Interactive,
        Capability::Meta,
    ] {
        for tier in [
            PermissionTier::Plan,
            PermissionTier::ConfirmEach,
            PermissionTier::AutoEdit,
            PermissionTier::FullAccess,
        ] {
            assert_eq!(
                decide_gate(cap, tier),
                GateDecision::Allow,
                "{cap:?} @ {tier:?} 应放行"
            );
        }
    }
}

#[test]
fn file_write_row_matches_matrix() {
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
}

#[test]
fn network_row_matches_matrix() {
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
}

#[test]
fn tier_and_capability_survive_serde_roundtrip() {
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
