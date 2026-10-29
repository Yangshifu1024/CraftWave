//! mcp/config.rs 的单测（08 号票）：未知字段 round-trip、双作用域同名
//! 覆盖、条目解析容错、保存原子性。

use super::config::{McpConfigFile, McpConfigStore, McpScope, McpServerConfig};

/// 服务器条目：未知字段 round-trip（flatten + 保序 map——保存后原样读回，
/// 顺序与内容都不丢）。
#[test]
fn unknown_fields_round_trip_in_order() {
    let raw = serde_json::json!({
        "command": "uvx",
        "args": ["mcp-server-fetch"],
        "env": { "HOME": "/tmp" },
        "enabled": true,
        // 未知字段（用户/前瞻性字段，保存不得丢）
        "futureHint": "保持原样",
        "nested": { "a": 1, "b": [true, null] }
    });
    let config: McpServerConfig = serde_json::from_value(raw).expect("解析失败");
    // 未知字段确实被收留
    assert!(config.extra.contains_key("futureHint"));
    assert!(config.extra.contains_key("nested"));
    // round-trip：序列化后未知字段原样在场，且出现在已知字段之后（保序）
    let out = serde_json::to_value(&config).expect("序列化失败");
    assert_eq!(out["futureHint"], "保持原样");
    assert_eq!(out["nested"]["b"][0], serde_json::json!(true));
    let keys: Vec<&String> = out.as_object().expect("对象").keys().collect();
    let future_pos = keys
        .iter()
        .position(|k| *k == "futureHint")
        .expect("futureHint 在场");
    let command_pos = keys
        .iter()
        .position(|k| *k == "command")
        .expect("command 在场");
    assert!(
        future_pos > command_pos,
        "flatten 未知字段保序在已知字段之后"
    );
}

/// 顶层文件：mcpServers 条目 + 未知顶层字段 round-trip。
#[test]
fn file_round_trip_with_unknown_top_level_fields() {
    let raw = serde_json::json!({
        "mcpServers": {
            "fetch": { "command": "uvx", "args": ["mcp-server-fetch"] }
        },
        "comment": "手写注释字段保留"
    });
    let file: McpConfigFile = serde_json::from_value(raw).expect("解析失败");
    assert_eq!(file.mcp_servers.len(), 1);
    assert_eq!(
        file.extra.get("comment"),
        Some(&serde_json::json!("手写注释字段保留"))
    );
    let out = serde_json::to_value(&file).expect("序列化失败");
    assert_eq!(out["comment"], "手写注释字段保留");
    let (servers, bad) = file.parse_servers();
    assert!(bad.is_empty(), "条目解析失败：{bad:?}");
    assert_eq!(servers[0].0, "fetch");
    assert_eq!(servers[0].1.command.as_deref(), Some("uvx"));
}

/// 双作用域：同名覆盖（project 整体替换 global 同名条目）+ 其余并存。
#[test]
fn project_scope_overrides_same_name() {
    let dir = tempfile::tempdir().expect("临时目录");
    let project = tempfile::tempdir().expect("临时目录");
    let store = McpConfigStore::new(dir.path(), Some(&project.path().join(".craftwave")));
    // 全局：alpha(stdio) + beta
    let global: McpConfigFile = serde_json::from_value(serde_json::json!({
        "mcpServers": {
            "alpha": { "command": "global-alpha", "timeout": 10 },
            "beta": { "url": "http://127.0.0.1:9000" }
        }
    }))
    .expect("构造");
    store.save(McpScope::Global, &global).expect("保存全局");
    // 项目：alpha 覆盖（url 形态）
    let project_cfg: McpConfigFile = serde_json::from_value(serde_json::json!({
        "mcpServers": {
            "alpha": { "url": "http://127.0.0.1:8000", "read_only": true }
        }
    }))
    .expect("构造");
    store
        .save(McpScope::Project, &project_cfg)
        .expect("保存项目");
    // 合并视图：alpha 取项目版，beta 留全局版
    let merged = store.merged().expect("合并");
    let alpha = merged
        .iter()
        .find(|(n, _, _)| n == "alpha")
        .expect("alpha 在场");
    assert_eq!(alpha.1, McpScope::Project);
    assert_eq!(alpha.2.url.as_deref(), Some("http://127.0.0.1:8000"));
    assert!(alpha.2.read_only);
    let beta = merged
        .iter()
        .find(|(n, _, _)| n == "beta")
        .expect("beta 在场");
    assert_eq!(beta.1, McpScope::Global);
}

/// 配置校验：stdio 与 HTTP 二义（同时给 command 与 url）在保存命令层报
/// 参数错误（校验在 commands 层；此处钉 parse 层两者都收留）。
#[test]
fn both_transports_parse_but_validation_is_command_layer() {
    let raw = serde_json::json!({ "command": "x", "url": "http://x" });
    let config: McpServerConfig = serde_json::from_value(raw).expect("两者都收留");
    assert!(config.command.is_some() && config.url.is_some());
}

/// 缺失文件 = 空配置（首次使用形态）。
#[test]
fn missing_file_is_empty_config() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = McpConfigStore::new(dir.path(), None);
    let file = store.load(McpScope::Global).expect("缺失即空");
    assert!(file.mcp_servers.is_empty());
    // project 不可用（无项目上下文）
    assert!(store.path_of(McpScope::Project).is_none());
    assert!(store.load(McpScope::Project).is_err());
}
