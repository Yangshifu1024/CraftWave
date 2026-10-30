//! storage/trust.rs 的单测（24 号票）：统一信任存储的 roundtrip、scope
//! 判别、revoke 覆盖、08 号票遗留文件迁入、损坏兜底与 Q41 清空面。

use super::trust::{FileTrustStore, TrustDecision, TrustRecord, TrustScope, TrustStore};

/// 造一条记录的辅助（默认 remember）。
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

/// 基本语义：remember 后同 (scope, name, fingerprint) 免问；不同指纹/不同
/// 资源名/不同作用域互不命中（scope 判别是记录的第一键）。
#[test]
fn remember_and_match_semantics() {
    let dir = tempfile::tempdir().expect("临时目录");
    let store = FileTrustStore::new(dir.path());
    assert!(
        !store.is_trusted(TrustScope::Project, "guard", "fp1"),
        "空表未信任"
    );
    store
        .decide(record(
            TrustScope::Project,
            "guard",
            "fp1",
            TrustDecision::Remember,
        ))
        .expect("写入");
    assert!(store.is_trusted(TrustScope::Project, "guard", "fp1"));
    // 同名换内容（指纹变）→ 不命中（内容变更重新询问，§9.8）
    assert!(!store.is_trusted(TrustScope::Project, "guard", "fp2"));
    // 同内容不同资源名 → 不命中
    assert!(!store.is_trusted(TrustScope::Project, "other", "fp1"));
    // 同名同指纹跨作用域 → 不命中（scope 判别：项目信任不豁免 MCP 服务器）
    assert!(!store.is_trusted(TrustScope::Mcp, "guard", "fp1"));
}

/// revoke 覆盖：同键旧 remember 被显式撤销覆盖（is_trusted 立即转 false，
/// Q20 安全权限撤销立即生效），再 remember 可恢复。
#[test]
fn revoke_overwrites_remember() {
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
    assert!(!store.is_trusted(TrustScope::Project, "guard", "fp1"));
    assert_eq!(
        store.status_of(TrustScope::Project, "fp1").as_str(),
        "revoked"
    );
    // 未见过同 scope 指纹 → pending
    assert_eq!(
        store.status_of(TrustScope::Project, "fp-x").as_str(),
        "pending"
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
    // 决策覆盖不产生重复条目（同键覆盖 decidedAt/decision）
    assert_eq!(store.entries().len(), 1, "同键覆盖不追加");
}

/// roundtrip：跨实例（重启口径）读取同一文件，决策保留。
#[test]
fn roundtrip_across_instances() {
    let dir = tempfile::tempdir().expect("临时目录");
    {
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
    }
    let reopened = FileTrustStore::new(dir.path());
    assert!(reopened.is_trusted(TrustScope::Project, "guard", "fp1"));
    assert!(reopened.is_trusted(TrustScope::Mcp, "", "srv-fp"));
    assert_eq!(reopened.entries().len(), 2);
}

/// 08 号票遗留 mcp.json 迁入：统一文件缺失时旧条目以 scope=Mcp 合并进
/// 视图（resourceName 补空串）；is_trusted/status_of 对旧指纹照常命中；
/// 新写入后旧文件仍在（只读迁移面），但统一文件优先不产生重复。
#[test]
fn legacy_mcp_file_merged_into_view() {
    let dir = tempfile::tempdir().expect("临时目录");
    let trust_dir = dir.path().join("trust");
    std::fs::create_dir_all(&trust_dir).expect("建 trust 目录");
    std::fs::write(
        trust_dir.join("mcp.json"),
        r#"[{"fingerprint":"srv-fp-legacy","decidedAt":100,"decision":"remember"}]"#,
    )
    .expect("写遗留文件");
    let store = FileTrustStore::new(dir.path());
    // 视图含迁入条目（无统一文件）
    assert!(store.is_trusted(TrustScope::Mcp, "", "srv-fp-legacy"));
    assert_eq!(
        store.status_of(TrustScope::Mcp, "srv-fp-legacy").as_str(),
        "trusted"
    );
    // 新写入：统一文件落盘；视图合并后同指纹去重（旧文件保留不删）
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
}

/// 损坏兜底：统一文件损坏按空表处理（信任面宁缺勿炸——用户重新确认即
/// 可恢复，不做崩溃性失败）。
#[test]
fn corrupt_file_falls_back_to_empty() {
    let dir = tempfile::tempdir().expect("临时目录");
    let trust_dir = dir.path().join("trust");
    std::fs::create_dir_all(&trust_dir).expect("建 trust 目录");
    std::fs::write(trust_dir.join("project.json"), b"{not json").expect("写坏文件");
    let store = FileTrustStore::new(dir.path());
    assert!(store.entries().is_empty(), "损坏按空表");
    assert!(!store.is_trusted(TrustScope::Project, "x", "fp"));
    // 损坏后可正常写入重建
    store
        .decide(record(
            TrustScope::Project,
            "x",
            "fp",
            TrustDecision::Remember,
        ))
        .expect("重建");
    assert!(store.is_trusted(TrustScope::Project, "x", "fp"));
}

/// Q41 清空面：clear_all 后全部记录消失（33 号票恢复流程的接线面；
/// 恢复不复活已记住/已撤销许可，普通重启不调用本面）。
#[test]
fn clear_all_for_backup_restore() {
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
    assert!(store.entries().is_empty());
    assert!(!store.is_trusted(TrustScope::Project, "guard", "fp1"));
    assert!(!store.is_trusted(TrustScope::Mcp, "", "srv-fp"));
}
