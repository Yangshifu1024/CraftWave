//! 子代理内置角色表。
//!
//! 名词解释：角色（role）是主代理委派任务时可点名的"专职分身"——每个角色
//! 是「系统提示文本 + 工具白名单 + 默认步数预算 + 工作区形态」的组合。
//! 白名单是该角色可见、可执行的工具子集（三处同源纪律：工具可见面 =
//! 执行许可 = 子代理边界；白名单裁剪复用 [`crate::tools::filter_for_tier`]
//! 同一实现，先白名单过滤再档位过滤，plan 档写/联网工具照旧被过滤）。
//!
//! 预算（budgetSteps）：子代理最多进行的模型请求轮次；耗尽即以
//! E_SUB_BUDGET 回喂父 run、终态 outcome=partial。
//! 入参 `budgetSteps` 可覆盖默认值，钳制 1–1000。
//!
//! 工作区形态：只读角色直接读取当前工作区；写入角色在 Git
//! 项目中使用独立 worktree（ADR-0001），非 Git 项目禁止修改（由主代理
//! 负责，工具表裁掉写工具后只读执行）。

use crate::safety::tier::Capability;
use crate::tools::{ToolAnnotations, ToolOrigin, ToolSpec};

/// budgetSteps 入参的合法区间（钳制 1–1000）。
pub const MIN_BUDGET_STEPS: u32 = 1;
pub const MAX_BUDGET_STEPS: u32 = 1000;

/// 子代理的工作区形态（ADR-0001）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceKind {
    /// 只读：直接读取当前工作区（探索/评审/规划）。
    ReadOnly,
    /// 写入：Git 项目使用独立 worktree；非 Git 项目降级为只读（禁止修改）。
    WriteWorkspace,
}

/// 内置角色：系统提示 + 工具白名单 + 默认预算 + 工作区形态。
#[derive(Debug, Clone, Copy)]
pub struct SubagentRole {
    /// 角色名（模型点名依据；与前端 `$角色` 菜单同名单）。
    pub name: &'static str,
    /// 一句话职责（工具 description 与前端角色菜单共用口径）。
    pub summary: &'static str,
    /// 子代理的系统提示（注入子 run 请求面 history 首位，不进父会话史册）。
    pub system_prompt: &'static str,
    /// 工具白名单（⊆ 静态注册表；执行许可与可见面同源）。
    pub tools: &'static [&'static str],
    /// 默认步数预算（explore 40 / dev 90 / reviewer 45 / tester 60）。
    pub default_budget_steps: u32,
    /// 工作区形态。
    pub workspace: WorkspaceKind,
}

/// 7 个内置角色（explore/planner/implementer/reviewer/tester/
/// frontend-dev/backend-dev）。数组顺序即工具 description 与前端菜单顺序。
pub const ROLES: &[SubagentRole] = &[
    SubagentRole {
        name: "explore",
        summary: "只读探索：大范围检索代码与文档，返回调研结论而不污染主对话",
        system_prompt: "你是探索专员（explore）。你的任务是只读地调查代码库/文档并汇报结论：\
            优先使用 list_files/grep/read 定位证据，不修改任何文件。\
            最终把结论整理为结构化汇报（发现了什么、关键路径与行号、建议下一步），\
            汇报正文就是给主代理的 <report>。",
        tools: &["read", "list_files", "grep"],
        default_budget_steps: 40,
        workspace: WorkspaceKind::ReadOnly,
    },
    SubagentRole {
        name: "planner",
        summary: "只读规划：分析现状并产出可执行的分步计划文本，由主代理决策执行",
        system_prompt: "你是规划专员（planner）。你的任务是只读地理解任务背景并产出分步计划：\
            先探索关键文件确认现状，再把计划写成编号步骤（每步给出目标、涉及文件、验证方式）。\
            你不修改文件、不执行命令；计划正文就是给主代理的 <report>。",
        tools: &["read", "list_files", "grep"],
        default_budget_steps: 45,
        workspace: WorkspaceKind::ReadOnly,
    },
    SubagentRole {
        name: "implementer",
        summary: "实现者：在独立工作区修改代码完成指定任务，跑局部测试并汇报改动",
        system_prompt: "你是实现专员（implementer）。你在自己的独立工作区内修改代码完成任务：\
            改动保持最小聚焦；完成或受阻时用局部命令验证（注意：数据库、外部服务等共享资源\
            属于公共操作，你不要直接操作，在汇报中交回主代理协调）。\
            汇报（<report>）说明改了哪些文件、为什么、验证结果与未尽事项。",
        tools: &["read", "write", "edit", "list_files", "grep", "command"],
        default_budget_steps: 90,
        workspace: WorkspaceKind::WriteWorkspace,
    },
    SubagentRole {
        name: "reviewer",
        summary: "只读评审：审查代码/改动质量并给出结论与风险清单，不直接修改",
        system_prompt: "你是评审专员（reviewer）。你的任务是只读地审查指定范围的代码或改动：\
            给出正确性/安全性/可维护性结论与具体风险清单（引用文件与行号）。\
            你不修改文件；评审结论就是给主代理的 <report>。",
        tools: &["read", "list_files", "grep"],
        default_budget_steps: 45,
        workspace: WorkspaceKind::ReadOnly,
    },
    SubagentRole {
        name: "tester",
        summary: "测试者：在独立工作区运行与编写局部测试，回报通过/失败证据",
        system_prompt: "你是测试专员（tester）。你在自己的独立工作区内验证指定目标：\
            运行局部测试与检查命令收集证据；新增测试文件须放在独立 _test.rs 或 .test.ts(x)。\
            数据库、外部服务等共享资源操作属公共操作，交回主代理协调。\
            汇报（<report>）给出通过/失败清单与复现命令。",
        tools: &["read", "list_files", "grep", "command"],
        default_budget_steps: 60,
        workspace: WorkspaceKind::WriteWorkspace,
    },
    SubagentRole {
        name: "frontend-dev",
        summary: "前端实现者：在独立工作区修改界面层代码并自验渲染逻辑",
        system_prompt: "你是前端实现专员（frontend-dev）。你在自己的独立工作区内修改界面层\
            （组件/状态/样式）完成任务：保持既有组件范式；能本地验证的先验证。\
            汇报（<report>）说明改动文件、行为变化与自验结果。",
        tools: &["read", "write", "edit", "list_files", "grep", "command"],
        default_budget_steps: 90,
        workspace: WorkspaceKind::WriteWorkspace,
    },
    SubagentRole {
        name: "backend-dev",
        summary: "后端实现者：在独立工作区修改服务端代码并跑局部检查",
        system_prompt: "你是后端实现专员（backend-dev）。你在自己的独立工作区内修改服务端\
            代码完成任务：改动保持最小聚焦；跑局部构建/测试验证。\
            数据库、外部服务等共享资源操作属公共操作，交回主代理协调。\
            汇报（<report>）说明改动文件、验证结果与未尽事项。",
        tools: &["read", "write", "edit", "list_files", "grep", "command"],
        default_budget_steps: 90,
        workspace: WorkspaceKind::WriteWorkspace,
    },
];

/// 按名查角色（未知角色返回 None，由调用方回喂 E_INVALID_ARG）。
pub fn find_role(name: &str) -> Option<&'static SubagentRole> {
    ROLES.iter().find(|r| r.name == name)
}

/// budgetSteps 入参钳制（1–1000；None → 角色默认值）。
pub fn clamp_budget_steps(role: &SubagentRole, budget_steps: Option<u32>) -> u32 {
    budget_steps
        .unwrap_or(role.default_budget_steps)
        .clamp(MIN_BUDGET_STEPS, MAX_BUDGET_STEPS)
}

/// subagent 工具的注册表规格：
/// 参数 `{ role, task, budgetSteps? }`；description 内嵌 7 角色清单与各自
/// 职责一句话（模型点名的依据）。Meta 能力全档放行。
pub fn subagent_spec() -> ToolSpec {
    let roster: String = ROLES
        .iter()
        .map(|r| format!("- {}: {}", r.name, r.summary))
        .collect::<Vec<_>>()
        .join("\n");
    let description = format!(
        "把独立任务委派给专职子代理执行（子代理有自己的上下文与预算，并行干活互不踩文件；\
         仅主代理可委派）。用 role 点名角色，task 给出完整自足的任务描述（子代理看不到\
         本对话历史，任务描述必须包含它需要的全部背景与目标）。budgetSteps 可覆盖步数\
         预算。子代理完成后以 <report> 汇报作为本工具结果返回。可选角色：\n{roster}"
    );
    ToolSpec::build(
        "subagent",
        description,
        Capability::Meta,
        serde_json::json!({
            "type": "object",
            "properties": {
                "role": {
                    "type": "string",
                    "enum": ROLES.iter().map(|r| r.name).collect::<Vec<_>>(),
                    "description": "角色名（见 description 清单）"
                },
                "task": {
                    "type": "string",
                    "description": "完整自足的任务描述（子代理无本对话历史）"
                },
                "budgetSteps": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1000,
                    "description": "步数预算（模型请求轮次上限），缺省按角色默认"
                }
            },
            "required": ["role", "task"],
            "additionalProperties": false
        }),
        ToolAnnotations {
            read_only_hint: false,
            destructive_hint: false,
            idempotent_hint: false,
            open_world_hint: false,
        },
        ToolOrigin::Static,
    )
}

/// 角色白名单的工具规格裁剪（三处同源：白名单过滤 + 档位过滤
/// 复用 [`crate::tools::filter_for_tier`] 同一实现）。
/// 非工作区形态约束（非 Git 项目禁写）由装配层再裁——本函数只做
/// 「白名单 ∩ 注册表 → 档位过滤」的静态面。
pub fn role_tool_specs(
    role: &SubagentRole,
    tier: crate::safety::tier::PermissionTier,
) -> Vec<crate::tools::ToolSpec> {
    let whitelisted: Vec<crate::tools::ToolSpec> = crate::tools::build_registry()
        .into_iter()
        .filter(|spec| role.tools.contains(&spec.name.as_str()))
        .collect();
    crate::tools::filter_for_tier(whitelisted, tier)
}
