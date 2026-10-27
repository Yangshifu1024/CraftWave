//! 会话索引单元测试（独立测试文件）。
//!
//! 测试面覆盖：get 按 id 精确查找（含找不到返回 None）、mutate 字段级部分更新、
//! remove 删除条目、超过 2000 条上限后最旧条目按 updatedAt 顺序归档进
//! archive.jsonl 且索引中不再可见。

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
    }
}

// 断言：get 能按 id 找到已登记的会话，不存在的 id 返回 None。
#[test]
fn get_finds_entry_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let idx = index(dir.path());
    idx.upsert(meta("s1", 10)).unwrap();
    assert_eq!(idx.get("s1").map(|m| m.id), Some("s1".to_string()));
    assert_eq!(idx.get("缺"), None);
}

// 断言：mutate 能就地修改指定会话的字段并返回更新后的条目；会话不存在返回 None。
#[test]
fn mutate_updates_fields_and_reports_missing() {
    let dir = tempfile::tempdir().unwrap();
    let idx = index(dir.path());
    idx.upsert(meta("s1", 10)).unwrap();
    let updated = idx
        .mutate("s1", |m| {
            m.title = "改名".to_string();
            m.project_id = None;
        })
        .unwrap();
    assert_eq!(updated.map(|m| m.title), Some("改名".to_string()));
    assert_eq!(idx.mutate("缺", |_| {}).unwrap(), None);
}

// 断言：remove 删除指定会话并返回被删条目，删除后列表为空、再次删除返回 None。
#[test]
fn remove_drops_entry_and_reports_missing() {
    let dir = tempfile::tempdir().unwrap();
    let idx = index(dir.path());
    idx.upsert(meta("s1", 10)).unwrap();
    assert_eq!(
        idx.remove("s1").unwrap().map(|m| m.id),
        Some("s1".to_string())
    );
    assert!(idx.list().is_empty());
    assert_eq!(idx.remove("s1").unwrap(), None);
}

// 断言：索引超过 2000 条上限后再 upsert，最旧条目（按 updatedAt，其次按 id）
// 被追加进 archive.jsonl 并从索引移除，新条目正常保留。
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
