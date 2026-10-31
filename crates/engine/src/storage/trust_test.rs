//! storage/trust.rs 的单测：统一信任存储的 roundtrip、scope
//! 判别、revoke 覆盖、遗留文件迁入、损坏兜底与 清空面。
//! 全链路语义合并为单个 #[test]，断言消息带阶段标识。

use super::trust::{FileTrustStore, TrustDecision, TrustRecord, TrustScope, TrustStore};

/// 造一条记录的辅助（decided_at 固定）。
fn record(
    scope: TrustScope,
    name: &str,
    fingerprint: &str,
    decision: TrustDecision,
) -> TrustRecord {
    TrustRecord {
        scope,
        resource_name: name.to_owned(),
        fingerprint: fingerprint.to_owned(),
        decided_at: 1000,
        decision: decision.as_str().to_owned(),
    }
}

// 断言：统一信任存储全链路——空表未信任；remember 后同 (scope, name,
// fingerprint) 免问而换指纹/换资源名/跨作用域均不命中（scope 判别是第一键）；
// revoke 覆盖 remember 立即生效且同键覆盖不追加条目、再 remember 可
// 恢复；跨实例（重启口径）roundtrip 决策保留；遗留 mcp.json 以
// scope=Mcp 迁入视图且统一文件优先、旧文件保留不删；统一文件损坏按空表
// 处理（宁缺勿炸）且可重建；clear_all 清空全部记录（恢复流程接线面）。
#[test]
fn trust_store_semantics_end_to_end() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = FileTrustStore::new(dir.path());
    assert!(
        !store.is_trusted(TrustScope::Project, "guard", "fp1"),
        "空表未信任"
    );

    // remember 与 scope 判别：同名换内容 / 同内容换名 / 跨作用域均不命中
    store
        .decide(record(
            TrustScope::Project,
            "guard",
            "fp1",
            TrustDecision::Remember,
        ))
        .expect("写入");
    assert!(store.is_trusted(TrustScope::Project, "guard", "fp1"));
    assert!(
        !store.is_trusted(TrustScope::Project, "guard", "fp2"),
        "指纹变 → 重新询问"
    );
    assert!(
        !store.is_trusted(TrustScope::Project, "other", "fp1"),
        "资源名不同 → 不命中"
    );
    assert!(
        !store.is_trusted(TrustScope::Mcp, "guard", "fp1"),
        "项目信任不豁免 MCP 服务器（scope 判别）"
    );

    // revoke 覆盖：立即转 false、status_of 三态、再 remember 恢复、同键不追加
    assert_eq!(
        store.status_of(TrustScope::Project, "fp1").as_str(),
        "trusted"
    );
    store
        .decide(record(
            TrustScope::Project,
            "guard",
            "fp1",
            TrustDecision::Revoke,
        ))
        .expect("撤销");
    assert!(
        !store.is_trusted(TrustScope::Project, "guard", "fp1"),
        "撤销立即生效"
    );
    assert_eq!(
        store.status_of(TrustScope::Project, "fp1").as_str(),
        "revoked"
    );
    assert_eq!(
        store.status_of(TrustScope::Project, "fp-x").as_str(),
        "pending",
        "未见过同 scope 指纹 → pending"
    );
    store
        .decide(record(
            TrustScope::Project,
            "guard",
            "fp1",
            TrustDecision::Remember,
        ))
        .expect("重新记住");
    assert!(store.is_trusted(TrustScope::Project, "guard", "fp1"));
    assert_eq!(store.entries().len(), 1, "同键覆盖不追加");

    // roundtrip：跨实例（重启口径）读取同一文件，决策保留
    {
        let store = FileTrustStore::new(dir.path());
        store
            .decide(record(
                TrustScope::Mcp,
                "",
                "srv-fp",
                TrustDecision::Remember,
            ))
            .expect("写入 MCP 信任");
    }
    let reopened = FileTrustStore::new(dir.path());
    assert!(
        reopened.is_trusted(TrustScope::Project, "guard", "fp1"),
        "重启后保留"
    );
    assert!(reopened.is_trusted(TrustScope::Mcp, "", "srv-fp"));

    // 遗留 mcp.json 迁入：旧条目合并进视图、统一文件优先、旧文件保留
    let dir = tempfile::tempdir().expect("临时目录");
    let trust_dir = dir.path().join("trust");
    std::fs::create_dir_all(&trust_dir).expect("建 trust 目录");
    std::fs::write(
        trust_dir.join("mcp.json"),
        r#"[{"fingerprint":"srv-fp-legacy","decidedAt":100,"decision":"remember"}]"#,
    )
    .expect("写遗留文件");
    let store = FileTrustStore::new(dir.path());
    assert!(
        store.is_trusted(TrustScope::Mcp, "", "srv-fp-legacy"),
        "遗留条目迁入视图"
    );
    assert_eq!(
        store.status_of(TrustScope::Mcp, "srv-fp-legacy").as_str(),
        "trusted"
    );
    store
        .decide(record(
            TrustScope::Mcp,
            "",
            "srv-fp-legacy",
            TrustDecision::Revoke,
        ))
        .expect("覆盖撤销");
    assert_eq!(
        store.status_of(TrustScope::Mcp, "srv-fp-legacy").as_str(),
        "revoked",
        "统一文件记录优先于遗留文件"
    );
    assert_eq!(store.entries().len(), 1, "同指纹去重不重复");
    assert!(
        trust_dir.join("mcp.json").exists(),
        "遗留文件保留不删（只读迁移）"
    );
    assert!(trust_dir.join("project.json").exists(), "统一文件落盘");

    // 损坏兜底：按空表处理，可正常写入重建
    let dir = tempfile::tempdir().expect("临时目录");
    let trust_dir = dir.path().join("trust");
    std::fs::create_dir_all(&trust_dir).expect("建 trust 目录");
    std::fs::write(trust_dir.join("project.json"), b"{not json").expect("写坏文件");
    let store = FileTrustStore::new(dir.path());
    assert!(store.entries().is_empty(), "损坏按空表（宁缺勿炸）");
    assert!(!store.is_trusted(TrustScope::Project, "x", "fp"));
    store
        .decide(record(
            TrustScope::Project,
            "x",
            "fp",
            TrustDecision::Remember,
        ))
        .expect("重建");
    assert!(
        store.is_trusted(TrustScope::Project, "x", "fp"),
        "损坏后可重建"
    );

    // 清空面：clear_all 后全部记录消失
    let dir = tempfile::tempdir().expect("临时目录");
    let store = FileTrustStore::new(dir.path());
    store
        .decide(record(
            TrustScope::Project,
            "guard",
            "fp1",
            TrustDecision::Remember,
        ))
        .expect("写入");
    store
        .decide(record(
            TrustScope::Mcp,
            "",
            "srv-fp",
            TrustDecision::Remember,
        ))
        .expect("写入");
    store.clear_all().expect("清空");
    assert!(store.entries().is_empty(), "清空后无记录");
    assert!(!store.is_trusted(TrustScope::Project, "guard", "fp1"));
    assert!(!store.is_trusted(TrustScope::Mcp, "", "srv-fp"));
}
