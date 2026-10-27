//! 审批门（技术设计文档 §5.4 步骤①–④ × §8.5 权限矩阵 × §8.7 围栏 × §8.6
//! 计划门）：主循环 `ToolGate` 接口的生产策略实现。
//!
//! 名词解释：审批门 = 工具执行前的安全判定，结论有三种——放行（直接执行）、
//! 挂起（暂停整个 run 等用户批准）、硬拦（直接拒绝并把错误回喂给模型）。
//! 权限档 = 控制模型可用工具范围的档位（plan/confirm_each/auto_edit/
//! full_access，从只读到全开放）；围栏 = 命令工具的硬性安全底线（黑名单
//! 命令任何档位都拦）；计划门 = "已批准的计划全部完成后不允许继续写文件"
//! 的约束。
//!
//! 判定顺序（首个命中即出结论）：
//! 1. 已批准签名：本 run 内同"工具名 + 参数 JSON"的调用此前已批准 → 放行
//!    （挂起恢复后主循环会原样重放挂起批，按签名放行避免再次挂起死循环）；
//! 2. 围栏（command 专属）：Block → E_FENCE_BLOCKED 硬拦（任何档位不豁免）；
//!    Confirm → 挂起（围栏原因随载荷展示给用户）；
//! 3. 计划门（写文件/联网类）：已批准计划存在且条目全部完成仍要写 →
//!    E_PLAN_STALE 硬拦回喂（提示模型先更新计划）；
//! 4. 权限矩阵（§8.5 的单一实现 `decide_gate`）：Filtered → E_TOOL_BLOCKED
//!    硬拦；NeedsApproval → 挂起；其余放行。
//!
//! 挂起之后的载荷构造与 `resolve_ask` 恢复循环在宿主命令层（commands/chat.rs）。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::safety::fence::{Fence, FenceVerdict};
use crate::safety::tier::{GateDecision, PermissionTier, decide_gate};
use crate::tools::plan::PlanState;
use crate::util::errors::{E_FENCE_BLOCKED, E_PLAN_STALE, E_TOOL_BLOCKED};

use super::run_loop::{GateVerdict, ToolGate};

/// 生产环境使用的审批门。
pub struct ApprovalGate {
    /// 当前会话的权限档。
    tier: PermissionTier,
    /// 命令围栏判定器（黑名单/写信号）。
    fence: Fence,
    /// 本 run 内已批准的（工具名, 参数 JSON）签名集（resolve_ask 放行后登记）。
    approved: Mutex<HashSet<(String, String)>>,
    /// 计划状态（与 RunHandle 持有的是同一单元；plan 工具写、approve_plan 冻结）。
    plan: Arc<Mutex<Option<PlanState>>>,
}

/// ApprovalGate 的登记与辅助判定。
impl ApprovalGate {
    /// 构造：给定权限档与共享计划状态（围栏与签名集内部自建）。
    pub fn new(tier: PermissionTier, plan: Arc<Mutex<Option<PlanState>>>) -> Self {
        Self {
            tier,
            fence: Fence::new(),
            approved: Mutex::new(HashSet::new()),
            plan,
        }
    }

    /// 审批通过后登记签名（挂起恢复重放同一调用时按签名放行）。
    pub fn approve(&self, name: &str, args_json: &str) {
        self.approved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((name.to_owned(), args_json.to_owned()));
    }

    /// 从参数 JSON 中提取 command 字段（围栏判定的输入）。
    fn command_of(args_json: &str) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(args_json)
            .ok()?
            .get("command")?
            .as_str()
            .map(str::to_owned)
    }

    /// 计划是否"已过期"：存在已批准的计划且条目全部完成——此时继续写文件
    /// 应被计划门拦下（§8.6）。
    fn plan_stale(&self) -> bool {
        self.plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(PlanState::approved_and_all_completed)
    }
}

/// 审批门策略实现：按文件头列出的四步顺序判定（首个命中即出结论）。
impl ToolGate for ApprovalGate {
    /// 判定一次工具调用：返回放行/挂起/硬拦（各步骤见内部行注释）。
    fn judge(&self, name: &str, args_json: &str) -> GateVerdict {
        // (1) 已批准签名：恢复重放的挂起调用放行
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

/// 权限档的对外键名（错误提示文案里用）。
fn tier_key(tier: PermissionTier) -> &'static str {
    match tier {
        PermissionTier::Plan => "plan",
        PermissionTier::ConfirmEach => "confirm_each",
        PermissionTier::AutoEdit => "auto_edit",
        PermissionTier::FullAccess => "full_access",
    }
}
