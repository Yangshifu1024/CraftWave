//! 审批门（ApprovalGate）单元测试：权限矩阵各档判定、围栏黑名单与写信号、
//! 计划门拦截、已批准签名放行、未知工具透传。

use std::sync::{Arc, Mutex};

use super::gate::ApprovalGate;
use super::run_loop::{GateVerdict, ToolGate};
use crate::safety::tier::PermissionTier;
use crate::storage::entries::{PlanItem, PlanItemStatus};
use crate::tools::plan::PlanState;

/// 造一个计划条目（给定 id 与状态）。
fn item(id: &str, status: PlanItemStatus) -> PlanItem {
    PlanItem {
        id: id.to_owned(),
        text: format!("任务 {id}"),
        status,
    }
}

/// 造审批门与其共享计划状态（返回同一把锁的句柄，便于测试中修改计划）。
fn gate_with_plan(tier: PermissionTier) -> (ApprovalGate, Arc<Mutex<Option<PlanState>>>) {
    let plan: Arc<Mutex<Option<PlanState>>> = Arc::new(Mutex::new(None));
    (ApprovalGate::new(tier, plan.clone()), plan)
}

/// 造 command 工具的参数 JSON。
fn command_args(command: &str) -> String {
    serde_json::json!({ "command": command }).to_string()
}

/// 断言权限矩阵在各档位下给出正确判定（放行/挂起/硬拦逐例核对）。
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

/// 断言围栏黑名单命令在任何权限档位都被 E_FENCE_BLOCKED 硬拦。
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

/// 断言含写信号的命令被挂起，且携带围栏原因（界面卡片展示用）。
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

/// 断言 plan 档下白名单命令虽通过围栏，仍被权限矩阵以 E_TOOL_BLOCKED 拦下
/// （围栏放行不等于权限档放行）。
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

/// 断言计划门：已批准计划全部完成后再写文件被 E_PLAN_STALE 拦截且提示
/// 更新计划；只读工具不受计划门约束。
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

/// 断言已批准签名不再重复挂起；不同签名的调用不连带放行。
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

/// 断言未知工具放行给执行器（由执行器回喂 E_INVALID_ARG，不在门这里拦截）。
#[test]
fn unknown_tool_passes_to_executor() {
    let (gate, _) = gate_with_plan(PermissionTier::Plan);
    assert!(matches!(gate.judge("nope", "{}"), GateVerdict::Allow));
}
