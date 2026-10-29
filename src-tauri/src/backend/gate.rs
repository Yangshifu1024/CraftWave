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
//!    硬拦；NeedsApproval → 挂起；其余放行；
//! 5. **MCP 动态工具分支**（08 号票，§8.5 注 + §9.7）：MCP 工具未声明
//!    read_only（用户配置）且无 Q44 许可（三元组 = 服务器启动指纹 + 原始
//!    工具名 + 已确认输入 schema 全等；来源为审批「始终允许」或服务器级
//!    always_allow 的同步自动授权）时——**即便 full_access 也挂起确认**
//!    （FR-08-04 高危外部调用永远过人）。read_only 声明的 MCP 工具按
//!    ReadOnly 能力走矩阵放行；plan 档已由 filter_for_tier 全过滤。
//!
//! 挂起之后的载荷构造与 `resolve_ask` 恢复循环在宿主命令层（commands/chat.rs）。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::safety::fence::{Fence, FenceVerdict};
use crate::safety::tier::{GateDecision, PermissionTier, decide_gate};
use crate::tools::plan::PlanState;
use crate::tools::{ToolOrigin, lookup_spec};
use crate::util::errors::{E_FENCE_BLOCKED, E_PLAN_STALE, E_TOOL_BLOCKED};

use super::run_loop::{GateVerdict, ToolGate};

/// MCP 工具身份的判定视图（Q44 三元组 + 门控输入；08 号票）。
/// backend 层自定义的窄结构——审批门不依赖 mcp 模块（依赖方向 §2.2：
/// host 装配时注入实现，依赖倒置与「tools/agent 经接口交互」同精神）。
#[derive(Debug, Clone)]
pub struct McpToolView {
    /// 配置里的服务器名（审批标题展示）。
    pub server_name: String,
    /// 服务器启动指纹（Q44 许可三元组第一元）。
    pub server_fingerprint: String,
    /// 原始工具名（许可三元组第二元）。
    pub original_name: String,
    /// 输入 schema 哈希（许可三元组第三元）。
    pub schema_hash: String,
    /// 用户配置的只读声明（true → 能力按 ReadOnly 进矩阵）。
    pub read_only: bool,
}

/// MCP 判定视图（宿主装配；测试传 [`NoopMcpGate`] 或 None）。
pub trait McpGateView: Send + Sync {
    /// 归一名 → 工具身份视图（非 MCP 归一名返回 None）。
    fn view(&self, normalized_name: &str) -> Option<McpToolView>;
    /// Q44 许可三元组是否命中（serverFingerprint + originalName + schemaHash
    /// 全等 → 免审批）。
    fn license_hit(&self, view: &McpToolView) -> bool;
}

/// 空实现（无 MCP 装配时的默认视图：一切查询 None，走静态路径）。
pub struct NoopMcpGate;

impl McpGateView for NoopMcpGate {
    fn view(&self, _normalized_name: &str) -> Option<McpToolView> {
        None
    }

    fn license_hit(&self, _view: &McpToolView) -> bool {
        false
    }
}

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
    /// MCP 判定视图（None = 未装配 MCP，全部走静态路径）。
    mcp: Option<Arc<dyn McpGateView>>,
}

/// ApprovalGate 的登记与辅助判定。
impl ApprovalGate {
    /// 构造：给定权限档与共享计划状态（围栏与签名集内部自建）。MCP 视图
    /// 可后续经 [`Self::with_mcp`] 装配（M1 测试零改动）。
    pub fn new(tier: PermissionTier, plan: Arc<Mutex<Option<PlanState>>>) -> Self {
        Self {
            tier,
            fence: Fence::new(),
            approved: Mutex::new(HashSet::new()),
            plan,
            mcp: None,
        }
    }

    /// 装配 MCP 判定视图（08 号票：MCP 分支判定与 Q44 许可核验）。
    pub fn with_mcp(mut self, mcp: Arc<dyn McpGateView>) -> Self {
        self.mcp = Some(mcp);
        self
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

/// 审批门策略实现：按文件头列出的顺序判定（首个命中即出结论）。
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
        let Some(spec) = lookup_spec(name) else {
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
        // (4) MCP 动态工具分支（08 号票，§9.7 Q44）：未声明 read_only 且
        // 无许可 → 即便 full_access 也挂起。放行条件二选一：
        //   a. 用户配置 read_only=true（spec.capability 已是 ReadOnly，落
        //      到 (5) 矩阵按 ReadOnly 放行）；
        //   b. Q44 许可三元组命中（服务器启动指纹 + 原始工具名 + 输入
        //      schema 全等）——来源是用户审批「始终允许（当前版本）」或
        //      服务器级 always_allow=true 的清单同步自动授权（schema 有
        //      未确认变更时不自动放行，见 mcp/pool 的 auto-grant）。
        //      许可判定与服务器级声明解耦：用户点「始终允许」后立即生效
        //      （热更新，FR-08-04），不要求配置里另开开关。
        if spec.origin == ToolOrigin::Mcp
            && let Some(mcp) = &self.mcp
            && let Some(view) = mcp.view(name)
            && !view.read_only
            && !mcp.license_hit(&view)
        {
            return GateVerdict::Suspend { fence_reason: None };
        }
        // (5) 权限矩阵（§8.5；plan 档 MCP 已被 filter_for_tier 全过滤，
        // 兜底 E_TOOL_BLOCKED）
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
