//! ApprovalGate 单测（§5.4 × §8.5 × §8.7 × §8.6）：判定顺序与签名放行。

use std::sync::{Arc, Mutex};

use super::gate::ApprovalGate;
use super::run_loop::{GateVerdict, ToolGate};
use crate::safety::tier::PermissionTier;
use crate::storage::entries::{PlanItem, PlanItemStatus};
use crate::tools::plan::PlanState;

fn item(id: &str, status: PlanItemStatus) -> PlanItem {
    PlanItem {
        id: id.to_owned(),
        text: format!("任务 {id}"),
        status,
    }
}

fn gate_with_plan(tier: PermissionTier) -> (ApprovalGate, Arc<Mutex<Option<PlanState>>>) {
    let plan: Arc<Mutex<Option<PlanState>>> = Arc::new(Mutex::new(None));
    (ApprovalGate::new(tier, plan.clone()), plan)
}

fn command_args(command: &str) -> String {
    serde_json::json!({ "command": command }).to_string()
}

#[test]
fn matrix_verdicts_by_tier() {
    // (name, args, tier) → 期望判定形态
    let cases: Vec<(&str, String, PermissionTier, &str)> = vec![
        // confirm_each：FileWrite 挂起
        (
            "write",
            r#"{"path":"a.txt","content":"x"}"#.into(),
            PermissionTier::ConfirmEach,
            "suspend",
        ),
        (
            "edit",
            r#"{"path":"a.txt","oldText":"x","newText":"y"}"#.into(),
            PermissionTier::ConfirmEach,
            "suspend",
        ),
        // auto_edit：FileWrite 直写、Network（无写信号命令）挂起
        (
            "write",
            r#"{"path":"a.txt","content":"x"}"#.into(),
            PermissionTier::AutoEdit,
            "allow",
        ),
        (
            "command",
            command_args("ls"),
            PermissionTier::AutoEdit,
            "suspend",
        ),
        // full_access：FileWrite/无信号 Network 放行
        (
            "write",
            r#"{"path":"a.txt","content":"x"}"#.into(),
            PermissionTier::FullAccess,
            "allow",
        ),
        (
            "command",
            command_args("ls"),
            PermissionTier::FullAccess,
            "allow",
        ),
        // plan：FileWrite/Network 过滤硬拦；plan/ask 放行
        (
            "write",
            r#"{"path":"a.txt","content":"x"}"#.into(),
            PermissionTier::Plan,
            "blocked:E_TOOL_BLOCKED",
        ),
        (
            "command",
            command_args("ls"),
            PermissionTier::Plan,
            "blocked:E_TOOL_BLOCKED",
        ),
        (
            "plan",
            r#"{"action":"set"}"#.into(),
            PermissionTier::Plan,
            "allow",
        ),
        (
            "ask",
            r#"{"title":"t"}"#.into(),
            PermissionTier::Plan,
            "allow",
        ),
    ];
    for (name, args, tier, expect) in cases {
        let (gate, _) = gate_with_plan(tier);
        let verdict = gate.judge(name, &args);
        let actual = match &verdict {
            GateVerdict::Allow => "allow".to_owned(),
            GateVerdict::Suspend { fence_reason } => {
                if fence_reason.is_some() {
                    "suspend+fence".to_owned()
                } else {
                    "suspend".to_owned()
                }
            }
            GateVerdict::Blocked { code, .. } => format!("blocked:{code}"),
        };
        assert_eq!(actual, expect, "{name} @ {tier:?}");
    }
}

#[test]
fn fence_blacklist_blocks_all_tiers() {
    for tier in [
        PermissionTier::Plan,
        PermissionTier::ConfirmEach,
        PermissionTier::AutoEdit,
        PermissionTier::FullAccess,
    ] {
        let (gate, _) = gate_with_plan(tier);
        let verdict = gate.judge("command", &command_args("rm -rf /tmp/whatever"));
        let GateVerdict::Blocked { code, .. } = &verdict else {
            panic!("黑名单应硬拦（{tier:?}）：{verdict:?}")
        };
        assert_eq!(*code, "E_FENCE_BLOCKED");
    }
}

#[test]
fn fence_write_signal_suspend_with_reason() {
    let (gate, _) = gate_with_plan(PermissionTier::ConfirmEach);
    let verdict = gate.judge("command", &command_args("echo hi > out.txt"));
    let GateVerdict::Suspend { fence_reason } = &verdict else {
        panic!("写信号应挂起：{verdict:?}")
    };
    assert!(
        fence_reason.is_some(),
        "挂起带 fenceReason（AskPayload 展示）"
    );
}

#[test]
fn fence_plan_whitelist_still_filtered_by_matrix() {
    // plan 档：ls 命中白名单（fence Allow），但 Network 能力被矩阵 Filtered 硬拦
    let (gate, _) = gate_with_plan(PermissionTier::Plan);
    let verdict = gate.judge("command", &command_args("ls"));
    let GateVerdict::Blocked { code, .. } = &verdict else {
        panic!("plan 档 command 应被矩阵拦截：{verdict:?}")
    };
    assert_eq!(*code, "E_TOOL_BLOCKED");
}

#[test]
fn plan_gate_approved_all_completed_blocks_writes() {
    let (gate, plan) = gate_with_plan(PermissionTier::AutoEdit);
    {
        let mut state = plan.lock().unwrap();
        *state = Some(PlanState::default());
        let s = state.as_mut().unwrap();
        s.items = vec![item("a", PlanItemStatus::Completed)];
        s.approve(1);
    }
    let verdict = gate.judge("write", r#"{"path":"a.txt","content":"x"}"#);
    let GateVerdict::Blocked { code, message } = &verdict else {
        panic!("计划门应拦写：{verdict:?}")
    };
    assert_eq!(*code, "E_PLAN_STALE");
    assert!(message.contains("plan"), "提示模型先更新计划：{message}");
    // 只读不受计划门约束
    assert!(matches!(
        gate.judge("read", r#"{"path":"a"}"#),
        GateVerdict::Allow
    ));
}

#[test]
fn approved_signature_no_resuspend() {
    let (gate, _) = gate_with_plan(PermissionTier::ConfirmEach);
    let args = r#"{"path":"a.txt","content":"x"}"#;
    assert!(matches!(
        gate.judge("write", args),
        GateVerdict::Suspend { .. }
    ));
    gate.approve("write", args);
    assert!(matches!(gate.judge("write", args), GateVerdict::Allow));
    // 不同签名不连带放行
    let other = r#"{"path":"b.txt","content":"x"}"#;
    assert!(matches!(
        gate.judge("write", other),
        GateVerdict::Suspend { .. }
    ));
}

#[test]
fn unknown_tool_passes_to_executor() {
    let (gate, _) = gate_with_plan(PermissionTier::Plan);
    assert!(matches!(gate.judge("nope", "{}"), GateVerdict::Allow));
}
