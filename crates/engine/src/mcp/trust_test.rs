//! mcp/trust.rs 的单测：TOFU 指纹计算与稳定性/敏感性、信任存
//! 储的 decide/is_trusted、 许可三元组命中与撤销。

use super::config::McpScope;
use super::trust::{
    LaunchDecision, ToolLicenseStore, TrustStore, TrustSubject, launch_fingerprint,
    schema_fingerprint,
};
use crate::mcp::config::McpScope as Scope;

// 断言：TOFU 指纹稳定（同输入同指纹）且字段级敏感（config 内容/名称/
// 路径/作用域任一变化 → 指纹变化， 元组逐字段），指纹是 64 位 hex；
// schema 哈希同 schema 同哈希、输入 schema 变化 → 哈希变化（不继承
// 旧免审批许可的判定输入）；信任存储 remember 后同指纹免问、revoke 后
// 重新询问（状态机 Pending/Trusted/Revoked）； 许可三元组全等命中、
// 任一元变化不命中、revoke_server 按指纹清全部工具许可。
#[test]
fn fingerprint_trust_and_license_semantics() {
    // 指纹稳定性与字段级敏感性。
    let base = TrustSubject {
        scope: McpScope::Global,
        project_id: None,
        server_name: "fetch".into(),
        canonical_config_path: "/home/u/.craftwave/mcp.json".into(),
        config_bytes: br#"{"mcpServers":{}}"#.to_vec(),
    };
    let f1 = launch_fingerprint(&base);
    let f2 = launch_fingerprint(&base);
    assert_eq!(f1, f2, "同输入同指纹");

    // config 内容变化
    let mut s = base.clone();
    s.config_bytes = br#"{"mcpServers":{"a":{}}}"#.to_vec();
    assert_ne!(launch_fingerprint(&s), f1);

    // 服务器名变化
    let mut s = base.clone();
    s.server_name = "fetch2".into();
    assert_ne!(launch_fingerprint(&s), f1);

    // canonical 路径变化（worktree 副本路径不同 → 指纹变；归一身份按
    // 字面元组参与哈希）
    let mut s = base.clone();
    s.canonical_config_path = "/worktree/.craftwave/mcp.json".into();
    assert_ne!(launch_fingerprint(&s), f1);

    // 作用域变化
    let mut s = base.clone();
    s.scope = Scope::Project;
    assert_ne!(launch_fingerprint(&s), f1);

    // 指纹是 64 位 hex
    assert_eq!(f1.len(), 64);
    assert!(f1.chars().all(|c| c.is_ascii_hexdigit()));

    // schema 哈希：同 schema 同哈希；输入 schema 变化 → 哈希变化。
    let a = serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}});
    let b = serde_json::json!({"type": "object", "properties": {"x": {"type": "number"}}});
    assert_eq!(schema_fingerprint(&a), schema_fingerprint(&a));
    assert_ne!(schema_fingerprint(&a), schema_fingerprint(&b));
    assert_eq!(schema_fingerprint(&a).len(), 16);

    // 信任存储：remember 后同指纹免问；revoke 后重新询问。
    let dir = tempfile::tempdir().expect("临时目录");
    let store = TrustStore::new(dir.path());
    assert!(!store.is_trusted("fp1"));
    assert_eq!(store.status_of("fp1"), super::trust::TrustStatus::Pending);
    store
        .decide("fp1", LaunchDecision::Remember, 1000)
        .expect("写入");
    assert!(store.is_trusted("fp1"));
    assert_eq!(store.status_of("fp1"), super::trust::TrustStatus::Trusted);
    // revoke → 撤销态（下次连接重新 TOFU）
    store
        .decide("fp1", LaunchDecision::Revoke, 2000)
        .expect("撤销");
    assert!(!store.is_trusted("fp1"));
    assert_eq!(store.status_of("fp1"), super::trust::TrustStatus::Revoked);

    // 许可三元组：全等命中；任一元变化不命中；revoke_server 按指纹
    // 清全部工具许可。
    let dir = tempfile::tempdir().expect("临时目录");
    let store = ToolLicenseStore::new(dir.path());
    store
        .grant("srv_fp", "orig_tool", "schema_hash_1", 1)
        .expect("授予");
    assert!(store.is_licensed("srv_fp", "orig_tool", "schema_hash_1"));
    // 工具名变化（碰撞后缀漂移的展示名）不命中
    assert!(!store.is_licensed("srv_fp", "orig_tool__2", "schema_hash_1"));
    // schema 变化（输入 schema 变化不继承旧许可）不命中
    assert!(!store.is_licensed("srv_fp", "orig_tool", "schema_hash_2"));
    // 服务器身份变化（配置指纹变）不命中
    assert!(!store.is_licensed("srv_fp2", "orig_tool", "schema_hash_1"));
    // 按服务器撤销
    store.grant("srv_fp", "another", "h", 2).expect("授予");
    store.revoke_server("srv_fp").expect("撤销");
    assert!(!store.is_licensed("srv_fp", "orig_tool", "schema_hash_1"));
    assert!(!store.is_licensed("srv_fp", "another", "h"));
}
