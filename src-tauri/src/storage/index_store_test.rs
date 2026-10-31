//! 会话索引单元测试（独立测试文件）。
//!
//! 测试面覆盖：get 按 id 精确查找（含找不到返回 None）、mutate 字段级部分更新、
//! remove 删除条目、超过 2000 条上限后最旧条目按 updatedAt 顺序归档进
//! archive.jsonl 且索引中不再可见。
//!
//! 性能/确定性教训（红线保留）：大索引场景必须「直接落 index.json 一次写盘 +
//! 单次 upsert 触发归档」，禁止逐条 upsert 造 2000 条（O(n²) 慢测试）；归档
//! 行序必须按 updatedAt 显式写入确定，不依赖 HashSet 迭代序。

use std::fs;

use super::index_store::{SessionIndex, SessionMeta};

// 构造指向临时目录的索引实例的辅助函数。
fn index(dir: &std::path::Path) -> SessionIndex {
    SessionIndex::new(dir)
}

// 构造一条会话元数据样例的辅助函数（id 与 updatedAt 可指定）。
fn meta(id: &str, updated_at: i64) -> SessionMeta {
    SessionMeta {
        id: id.to_string(),
        project_id: None,
        title: format!("会话{id}"),
        created_at: 0,
        updated_at,
        revision: 1,
        message_count: 2,
        total_tokens: 100,
        authorized_external_dirs: Vec::new(),
    }
}

// 断言：索引基础操作语义——get 按 id 找到已登记会话、不存在的 id 返回 None；
// mutate 就地修改字段并返回更新后条目、不存在返回 None；remove 删除并返回
// 被删条目、删除后列表为空、再次删除返回 None。
#[test]
fn get_mutate_remove_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let idx = index(dir.path());
    idx.upsert(meta("s1", 10)).unwrap();

    // get：精确命中 + 未命中 None
    assert_eq!(idx.get("s1").map(|m| m.id), Some("s1".to_string()));
    assert_eq!(idx.get("缺"), None);

    // mutate：字段级部分更新 + 不存在返回 None
    let updated = idx
        .mutate("s1", |m| {
            m.title = "改名".to_string();
            m.project_id = None;
        })
        .unwrap();
    assert_eq!(updated.map(|m| m.title), Some("改名".to_string()));
    assert_eq!(idx.mutate("缺", |_| {}).unwrap(), None);

    // remove：删除返回被删条目、列表清空、重复删除 None
    assert_eq!(
        idx.remove("s1").unwrap().map(|m| m.id),
        Some("s1".to_string())
    );
    assert!(idx.list().is_empty());
    assert_eq!(idx.remove("s1").unwrap(), None);
}

// 断言：索引超过 2000 条上限后再 upsert，最旧条目（按 updatedAt，其次按 id）
// 被追加进 archive.jsonl 并从索引移除，新条目正常保留。
//
// 性能/确定性教训（红线保留）：2001 条直接落 index.json 一次写盘（逐条
// upsert 是 O(n²)，测试不走这个量）；候选归档行序按 updatedAt 显式写入
// 确定，断言不依赖 HashSet 迭代序。
#[test]
fn upsert_beyond_limit_archives_oldest() {
    let dir = tempfile::tempdir().unwrap();
    // 直接落 2001 条的 index.json（逐条 upsert 是 O(n²)，测试不走这个量）
    let file = serde_json::json!({
        "version": 1,
        "sessions": (0..2001)
            .map(|i| serde_json::json!({
                "id": format!("s{i}"),
                "title": format!("会话{i}"),
                "createdAt": 0,
                "updatedAt": 1000 + i,
                "revision": 1,
                "messageCount": 2,
                "totalTokens": 100,
            }))
            .collect::<Vec<_>>()
    });
    let index_path = dir.path().join("sessions").join("index.json");
    std::fs::create_dir_all(index_path.parent().unwrap()).unwrap();
    std::fs::write(&index_path, serde_json::to_vec(&file).unwrap()).unwrap();

    // 再进一条触发归档：2002 条 → 归档最旧两条（s0/s1）
    let idx = index(dir.path());
    idx.upsert(meta("s2001", 999_999)).unwrap();

    let listed = idx.list();
    assert_eq!(listed.len(), 2000);

    let archive_path = dir.path().join("sessions").join("archive.jsonl");
    let raw = fs::read_to_string(&archive_path).unwrap();
    let lines: Vec<&str> = raw.lines().collect();
    assert_eq!(lines.len(), 2);
    let first: SessionMeta = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(first.id, "s0", "最旧条目先进归档");
    let second: SessionMeta = serde_json::from_str(lines[1]).unwrap();
    assert_eq!(second.id, "s1");
    // 归档后索引里不再有 s0/s1，新条目在
    assert_eq!(idx.get("s0"), None);
    assert_eq!(idx.get("s1"), None);
    assert_eq!(idx.get("s2001").map(|m| m.id), Some("s2001".to_string()));
}
