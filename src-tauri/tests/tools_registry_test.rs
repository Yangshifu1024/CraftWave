//! 工具注册表契约测试（§8.1）：名称清单与顺序钉死（防漂移）、
//! spec 完整性（名称合法字符、描述长度、schema 形态）、三处同源过滤。

use craftwave_lib::safety::tier::{Capability, GateDecision, PermissionTier};
use craftwave_lib::tools::{STATIC_TOOL_NAMES, build_registry, filter_for_tier};

#[test]
fn registry_names_and_order_are_pinned() {
    let registry = build_registry();
    let names: Vec<String> = registry.iter().map(|s| s.name.clone()).collect();
    assert_eq!(names, STATIC_TOOL_NAMES.to_vec());
    // 顺序钉死（追加新工具在尾部，不重排既有顺序；suggest 仅 run 尾调用
    // 不开放模型，不在注册表——14 号票；skill 随 09 号票 §8.3 M2 表进
    // 静态段尾部，description 动态列出启用技能名；subagent 随 10 号票
    // §8.3 M2 表 Meta 行进静态段尾部，description 内嵌 7 角色清单）。
    assert_eq!(
        names,
        [
            "read",
            "write",
            "edit",
            "list_files",
            "grep",
            "command",
            "plan",
            "ask",
            "skill",
            "subagent"
        ]
    );
}

#[test]
fn every_spec_is_wellformed() {
    for spec in build_registry() {
        // 名称合法字符 [a-z_][a-z0-9_]{0,63}。
        let mut chars = spec.name.chars();
        assert!(
            chars
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c == '_'),
            "名称首字符非法：{}",
            spec.name
        );
        assert!(
            chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                && spec.name.len() <= 64,
            "名称含非法字符或超长：{}",
            spec.name
        );
        // 描述 ≤800 字符。
        assert!(
            spec.description.chars().count() <= 800,
            "描述超长：{}",
            spec.name
        );
        // schema：object + additionalProperties false + 必填字段声明形态正确。
        assert_eq!(spec.params_schema["type"], "object", "{}", spec.name);
        assert_eq!(
            spec.params_schema["additionalProperties"], false,
            "{}",
            spec.name
        );
        assert!(
            spec.params_schema["properties"].is_object(),
            "{} 缺 properties",
            spec.name
        );
    }
}

#[test]
fn capabilities_match_design() {
    let registry = build_registry();
    let cap_of = |name: &str| {
        registry
            .iter()
            .find(|s| s.name == name)
            .expect("存在")
            .capability
    };
    assert_eq!(cap_of("read"), Capability::ReadOnly);
    assert_eq!(cap_of("write"), Capability::FileWrite);
    assert_eq!(cap_of("edit"), Capability::FileWrite);
    assert_eq!(cap_of("list_files"), Capability::ReadOnly);
    assert_eq!(cap_of("grep"), Capability::ReadOnly);
    assert_eq!(cap_of("command"), Capability::Network);
    assert_eq!(cap_of("plan"), Capability::Meta);
    assert_eq!(cap_of("ask"), Capability::Interactive);
    // skill（09 号票，§8.3 M2 表）：Meta 能力——全档放行（加载技能正文
    // 是无副作用操作，信任控制在技能面自身，§9.8）。
    assert_eq!(cap_of("skill"), Capability::Meta);
    // subagent（10 号票，§8.3 M2 表）：Meta 能力——全档放行（委派面的
    // 权限控制在子代理工具表/白名单与并发闸，§5.9）。
    assert_eq!(cap_of("subagent"), Capability::Meta);
}

#[test]
fn filter_for_tier_matches_matrix() {
    // plan 档：FileWrite/Network 过滤，只读/交互/Meta 保留（§8.5）。
    let plan: Vec<String> = filter_for_tier(build_registry(), PermissionTier::Plan)
        .iter()
        .map(|s| s.name.to_string())
        .collect();
    assert_eq!(
        plan,
        [
            "read",
            "list_files",
            "grep",
            "plan",
            "ask",
            "skill",
            "subagent"
        ]
    );

    // confirm_each / auto_edit / full_access：全保留（审批在门控做）。
    for tier in [
        PermissionTier::ConfirmEach,
        PermissionTier::AutoEdit,
        PermissionTier::FullAccess,
    ] {
        assert_eq!(
            filter_for_tier(build_registry(), tier).len(),
            10,
            "{tier:?} 应全保留"
        );
    }
}

#[test]
fn filter_never_changes_order() {
    let filtered = filter_for_tier(build_registry(), PermissionTier::ConfirmEach);
    let names: Vec<String> = filtered.iter().map(|s| s.name.clone()).collect();
    assert_eq!(names, STATIC_TOOL_NAMES.to_vec());
    let _ = GateDecision::Allow; // 引用完整性
}
