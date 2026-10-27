//! 审批门（§5.4 步骤①–④ × §8.5 矩阵 × §8.7 围栏 × §8.6 计划门）：
//! RunLoop `ToolGate` 的生产策略装配。
//!
//! 判定顺序（首个命中即出结论）：
//! 1. 已批准签名（本 run 内同 name+argsJson 放行——挂起恢复后 `next_step`
//!    原样重放挂起批，按签名放行避免死循环）；
//! 2. 围栏（command 专属）：Block → E_FENCE_BLOCKED 硬拦（任何档位不豁免）；
//!    Confirm → 挂起（fenceReason 随 AskPayload 展示）；
//! 3. 计划门（FileWrite/Network）：approvedSnapshot 存在且当前条目全部
//!    completed 仍要写 → E_PLAN_STALE 硬拦回喂（提示模型先更新计划）；
//! 4. 权限矩阵（§8.5 单一实现 `decide_gate`）：Filtered → E_TOOL_BLOCKED
//!    硬拦；NeedsApproval → 挂起；其余放行。
//!
//! 挂起后的 AskPayload 构造与 `resolve_ask` 恢复循环在 host（commands/chat.rs）。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::safety::fence::{Fence, FenceVerdict};
use crate::safety::tier::{GateDecision, PermissionTier, decide_gate};
use crate::tools::plan::PlanState;
use crate::util::errors::{E_FENCE_BLOCKED, E_PLAN_STALE, E_TOOL_BLOCKED};

use super::run_loop::{GateVerdict, ToolGate};

/// 生产审批门。
pub struct ApprovalGate {
    tier: PermissionTier,
    fence: Fence,
    /// 本 run 内已批准的 (name, argsJson) 签名集（resolve_ask(allow) 后登记）。
    approved: Mutex<HashSet<(String, String)>>,
    /// 计划状态（RunHandle 持有的同一单元；plan 工具写、approve_plan 冻结）。
    plan: Arc<Mutex<Option<PlanState>>>,
}

impl ApprovalGate {
    pub fn new(tier: PermissionTier, plan: Arc<Mutex<Option<PlanState>>>) -> Self {
        Self {
            tier,
            fence: Fence::new(),
            approved: Mutex::new(HashSet::new()),
            plan,
        }
    }

    /// 审批通过后登记签名（挂起恢复重放时放行）。
    pub fn approve(&self, name: &str, args_json: &str) {
        self.approved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((name.to_owned(), args_json.to_owned()));
    }

    /// 从 argsJson 提取 command 字段（围栏判定输入）。
    fn command_of(args_json: &str) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(args_json)
            .ok()?
            .get("command")?
            .as_str()
            .map(str::to_owned)
    }

    fn plan_stale(&self) -> bool {
        self.plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(PlanState::approved_and_all_completed)
    }
}

impl ToolGate for ApprovalGate {
    fn judge(&self, name: &str, args_json: &str) -> GateVerdict {
        // (0) 已批准签名：恢复重放的挂起调用放行
        if self
            .approved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&(name.to_owned(), args_json.to_owned()))
        {
            return GateVerdict::Allow;
        }
        let Some(spec) = crate::tools::build_registry()
            .into_iter()
            .find(|s| s.name == name)
        else {
            // 未知工具：放行给 executor 回喂 E_INVALID_ARG（不在这里拦截）
            return GateVerdict::Allow;
        };
        // (2) 围栏（command 专属，§5.4 ③）
        if name == "command"
            && let Some(command) = Self::command_of(args_json)
        {
            match self.fence.judge(&command, self.tier) {
                FenceVerdict::Block => {
                    return GateVerdict::Blocked {
                        code: E_FENCE_BLOCKED,
                        message: format!("围栏拦截（灾难级命令，任何档位不豁免）：{command}"),
                    };
                }
                FenceVerdict::Confirm => {
                    return GateVerdict::Suspend {
                        fence_reason: Some("命令含写信号，需确认".to_owned()),
                    };
                }
                FenceVerdict::Allow => {}
            }
        }
        // (3) 计划门（§8.6：FileWrite/Network 执行前校验）
        if matches!(
            spec.capability,
            crate::safety::tier::Capability::FileWrite | crate::safety::tier::Capability::Network
        ) && self.plan_stale()
        {
            return GateVerdict::Blocked {
                code: E_PLAN_STALE,
                message: "已批准的计划条目全部完成，继续写入前请先用 plan 工具更新计划".to_owned(),
            };
        }
        // (4) 权限矩阵（§8.5）
        match decide_gate(spec.capability, self.tier) {
            GateDecision::Filtered => GateVerdict::Blocked {
                code: E_TOOL_BLOCKED,
                message: format!("权限档排除：{name}（当前档 {}）", tier_key(self.tier)),
            },
            GateDecision::NeedsApproval => GateVerdict::Suspend { fence_reason: None },
            _ => GateVerdict::Allow,
        }
    }
}

fn tier_key(tier: PermissionTier) -> &'static str {
    match tier {
        PermissionTier::Plan => "plan",
        PermissionTier::ConfirmEach => "confirm_each",
        PermissionTier::AutoEdit => "auto_edit",
        PermissionTier::FullAccess => "full_access",
    }
}
