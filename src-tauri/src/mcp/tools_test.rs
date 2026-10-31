//! mcp/tools.rs 的单测（08 号票）：工具名归一（FR-08-06 合法集）、碰撞
//! 后缀、反向映射还原、glob 过滤、Q44 身份三元组。

use super::config::{McpScope, McpServerConfig};
use super::tools::{ConnKey, McpToolRegistry, normalize_tool_name};
use crate::tools::ToolOrigin;

// 断言：归一化合法集 [a-z_][a-z0-9_]{0,63}（§8.1）——大写小写化、非法
// 字符替换 _、首字符数字加前缀、超长截断、空名兜底；同 server 内注册名
// 与静态段/其他 server 的碰撞：`__2` 序号后缀 + 反向映射还原（归一名 →
// (连接键, 原名)，跨 server 不串）；重复同步（tools/list_changed 热更新）
// 保留既有归一名不抖动；deny glob 过滤优先于 allow；能力标注与 Q44 身份
// 三元组（read_only 配置 → ReadOnly，未声明 → Network；fingerprint/原名/
// schema 哈希/timeout 随身份带出）；断开移除后动态段即时收窄。
#[test]
fn normalize_collision_identity_and_lifecycle() {
    // 归一化规则。
    assert_eq!(normalize_tool_name("read_file"), "read_file");
    assert_eq!(normalize_tool_name("ReadFile"), "readfile");
    assert_eq!(normalize_tool_name("fs:read!path"), "fs_read_path");
    assert_eq!(normalize_tool_name("9lives"), "_9lives");
    assert_eq!(normalize_tool_name("日本語"), "___", "逐字符替换非 ASCII");
    let long = "a".repeat(100);
    assert_eq!(normalize_tool_name(&long).len(), 64);
    assert_eq!(normalize_tool_name(""), "_");

    // 碰撞后缀 + 反向映射还原。
    let registry = McpToolRegistry::default();
    let cfg = McpServerConfig::default();
    let key_a = ConnKey {
        scope: McpScope::Global,
        project_id: None,
        name: "server_a".into(),
        canonical_path: "/tmp/a.json".into(),
    };
    let key_b = ConnKey {
        scope: McpScope::Global,
        project_id: None,
        name: "server_b".into(),
        canonical_path: "/tmp/b.json".into(),
    };
    let mk = |name: &str, schema: serde_json::Value| {
        (
            name.to_owned(),
            format!("描述 {name}"),
            schema,
            crate::tools::ToolAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
                open_world_hint: false,
            },
        )
    };
    // server_a 注册 "read"（撞静态段）与同名 "echo"
    let a1 = registry.sync(
        key_a.clone(),
        "fp_a".into(),
        &cfg,
        vec![
            mk("read", serde_json::json!({})),
            mk("echo", serde_json::json!({"type":"object"})),
        ],
    );
    assert_eq!(a1, vec!["read".to_owned(), "echo".to_owned()]);
    // 静态段撞名：归一名为 read__2（"read" 本名被静态段占用，动态段不注册）
    let read2 = registry.identity("read__2").expect("read__2 在场");
    assert_eq!(read2.original_name, "read");
    assert_eq!(read2.conn_key.name, "server_a");
    assert!(registry.identity("read").is_none(), "本名归静态段");
    // server_b 注册 "echo"（跨 server 撞 a 的 echo）→ echo__2，且反向映射
    // 不串（echo → a，echo__2 → b）
    registry.sync(
        key_b.clone(),
        "fp_b".into(),
        &cfg,
        vec![mk("echo", serde_json::json!({"type":"object"}))],
    );
    assert_eq!(
        registry.identity("echo").expect("echo 归 a").conn_key.name,
        "server_a"
    );
    let b_echo = registry.identity("echo__2").expect("echo__2 归 b");
    assert_eq!(b_echo.conn_key.name, "server_b");
    assert_eq!(b_echo.original_name, "echo");

    // 重复同步（tools/list_changed 热更新）：同 server 保留既有归一名不抖动。
    registry.sync(
        key_a.clone(),
        "fp_a".into(),
        &cfg,
        vec![mk("echo", serde_json::json!({"type":"object"}))],
    );
    assert!(registry.identity("echo").is_some(), "重同步保留自己占用名");

    // 能力标注与 Q44 身份 + glob 过滤。
    let key = ConnKey {
        scope: McpScope::Global,
        project_id: None,
        name: "srv".into(),
        canonical_path: "/tmp/s.json".into(),
    };
    let mut cfg = McpServerConfig {
        read_only: true,
        timeout: Some(7),
        tools: Some(super::config::McpToolFilter {
            allow: vec!["fs__*".to_owned()],
            deny: vec!["fs__danger*".to_owned()],
        }),
        ..McpServerConfig::default()
    };
    let schema = serde_json::json!({"type": "object", "properties": {}});
    let registered = registry.sync(
        key.clone(),
        "fp1".into(),
        &cfg,
        vec![
            mk("fs__read", schema.clone()),
            mk("fs__danger_zone", schema.clone()),
            mk("other", schema.clone()),
        ],
    );
    // deny 命中的不注册；allow 未命中的不注册
    assert_eq!(registered, vec!["fs__read".to_owned()]);
    let spec = registry
        .snapshot()
        .into_iter()
        .find(|s| s.name == "fs__read")
        .expect("fs__read 在场");
    assert_eq!(spec.origin, ToolOrigin::Mcp);
    assert_eq!(
        spec.capability,
        crate::safety::tier::Capability::ReadOnly,
        "用户 read_only 配置 → ReadOnly"
    );
    let identity = registry.identity("fs__read").expect("身份在场");
    assert_eq!(identity.server_fingerprint, "fp1");
    assert_eq!(identity.original_name, "fs__read");
    assert_eq!(
        identity.schema_hash,
        crate::mcp::trust::schema_fingerprint(&schema)
    );
    assert_eq!(identity.call_timeout, std::time::Duration::from_secs(7));
    cfg.read_only = false;
    registry.sync(
        key.clone(),
        "fp2".into(),
        &cfg,
        vec![mk("fs__read", schema)],
    );
    let spec = registry
        .snapshot()
        .into_iter()
        .find(|s| s.name == "fs__read")
        .expect("fs__read 在场");
    assert_eq!(
        spec.capability,
        crate::safety::tier::Capability::Network,
        "未声明 read_only → Network"
    );

    // 断开移除：动态段即时收窄（工具表下一轮请求生效）。
    registry.remove(&key);
    assert!(registry.identity("fs__read").is_none());
}
