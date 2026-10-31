//! 角色表测试（10 号票，FR-07-01）：7 角色清单、budgetSteps 钳制、
//! 白名单裁剪（三处同源纪律——工具可见面 = 执行许可）与 subagent 工具规格。

use super::*;
use crate::safety::tier::PermissionTier;

// 断言：7 个内置角色、名字唯一、清单顺序与票面默认预算（§5.9）；
// 未知角色查询返回 None（执行入口回喂 E_INVALID_ARG 的依据）且大小写
// 敏感；budgetSteps 钳制（1–1000，None → 角色默认值）；只读角色的工具表
// 不含写/命令工具（可见面 = 执行许可，注册表 ⊇ 白名单）；写入角色白名单
// 与 plan 档过滤（filter_for_tier 同一实现裁剪）；任何角色的白名单都不含
// subagent（Q13 仅主代理委派的第一道落实）；subagent 工具规格 description
// 内嵌 7 角色清单与职责一句话、schema 必填 role/task、role 枚举全表。
#[test]
fn roles_roster_budget_and_tool_whitelists() {
    // 7 个内置角色、名字唯一、清单顺序（§5.9）。
    let names: Vec<&str> = ROLES.iter().map(|r| r.name).collect();
    assert_eq!(
        names,
        [
            "explore",
            "planner",
            "implementer",
            "reviewer",
            "tester",
            "frontend-dev",
            "backend-dev"
        ]
    );
    let mut sorted = names.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "角色名必须唯一");
    // 票面默认预算：explore 40 / dev 90 / reviewer 45 / tester 60。
    let budget_of = |name: &str| find_role(name).unwrap().default_budget_steps;
    assert_eq!(budget_of("explore"), 40);
    assert_eq!(budget_of("implementer"), 90);
    assert_eq!(budget_of("frontend-dev"), 90);
    assert_eq!(budget_of("backend-dev"), 90);
    assert_eq!(budget_of("reviewer"), 45);
    assert_eq!(budget_of("tester"), 60);

    // 未知角色查询返回 None（执行入口回喂 E_INVALID_ARG 的依据）。
    assert!(find_role("ceo").is_none());
    assert!(find_role("").is_none());
    assert!(find_role("Explore").is_none(), "角色名大小写敏感");

    // budgetSteps 钳制（票面：1–1000；None → 角色默认值）。
    let explore = find_role("explore").unwrap();
    assert_eq!(clamp_budget_steps(explore, None), 40);
    assert_eq!(clamp_budget_steps(explore, Some(500)), 500);
    assert_eq!(clamp_budget_steps(explore, Some(0)), 1, "0 钳到下限 1");
    assert_eq!(
        clamp_budget_steps(explore, Some(1001)),
        1000,
        "超限钳到 1000"
    );
    let planner = find_role("planner").unwrap();
    assert_eq!(
        clamp_budget_steps(planner, None),
        45,
        "planner 默认与 reviewer 同级"
    );

    // 白名单裁剪：只读角色的工具表不含写/命令工具（可见面 = 执行许可）。
    for name in ["explore", "planner", "reviewer"] {
        let role = find_role(name).unwrap();
        let specs = roles::role_tool_specs(role, PermissionTier::FullAccess);
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(
            !names
                .iter()
                .any(|n| matches!(*n, "write" | "edit" | "command")),
            "只读角色 {name} 的工具表不得含写入面：{names:?}"
        );
        // 白名单成员全部命中（注册表 ⊇ 白名单）。
        assert_eq!(names.len(), role.tools.len());
    }

    // 写入角色的白名单与 plan 档过滤（filter_for_tier 同一实现裁剪）。
    let role = find_role("implementer").unwrap();
    let full = roles::role_tool_specs(role, PermissionTier::AutoEdit);
    let full_names: Vec<&str> = full.iter().map(|s| s.name.as_str()).collect();
    assert!(
        full_names.contains(&"write")
            && full_names.contains(&"edit")
            && full_names.contains(&"command")
    );
    // plan 档：写/联网工具被档位过滤（可见面同步收窄）。
    let plan = roles::role_tool_specs(role, PermissionTier::Plan);
    let plan_names: Vec<&str> = plan.iter().map(|s| s.name.as_str()).collect();
    assert!(
        !plan_names.contains(&"write"),
        "plan 档 implementer 不得见 write"
    );
    assert!(
        !plan_names.contains(&"command"),
        "plan 档 implementer 不得见 command"
    );
    assert!(plan_names.contains(&"read"), "只读工具保留");

    // 子代理的角色边界：任何角色的白名单都不含 subagent（Q13 仅主代理
    // 委派在工具表层面的第一道落实；执行入口拒绝由 executor 的
    // allowed_tools + subagents=None 承担）。
    for role in ROLES {
        assert!(
            !role.tools.contains(&"subagent"),
            "角色 {} 白名单不得含 subagent",
            role.name
        );
    }

    // subagent 工具规格：description 内嵌 7 角色清单（模型点名依据）、
    // schema 必填 role/task。
    let spec = roles::subagent_spec();
    assert_eq!(spec.name, "subagent");
    for role in ROLES {
        assert!(
            spec.description.contains(role.name),
            "description 缺角色 {}",
            role.name
        );
        assert!(
            spec.description.contains(role.summary),
            "description 缺角色 {} 的职责一句话",
            role.name
        );
    }
    assert_eq!(
        spec.params_schema["required"],
        serde_json::json!(["role", "task"])
    );
    let enum_values: Vec<&str> = spec.params_schema["properties"]["role"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        enum_values,
        ROLES.iter().map(|r| r.name).collect::<Vec<_>>()
    );
}
