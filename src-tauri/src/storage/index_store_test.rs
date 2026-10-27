//! 会话索引单元测试（独立测试文件）：归档上限、get/mutate/remove。

use std::fs;

use super::index_store::{SessionIndex, SessionMeta};

fn index(dir: &std::path::Path) -> SessionIndex {
    SessionIndex::new(dir)
}

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

#[test]
fn get_finds_entry_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let idx = index(dir.path());
    idx.upsert(meta("s1", 10)).unwrap();
    assert_eq!(idx.get("s1").map(|m| m.id), Some("s1".to_string()));
    assert_eq!(idx.get("缺"), None);
}

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

#[test]
fn remove_drops_entry_and_reports_missing() {
    let dir = tempfile::tempdir().unwrap();
    let idx = index(dir.path());
    idx.upsert(meta("s1", 10)).unwrap();
    assert_eq!(idx.remove("s1").unwrap().map(|m| m.id), Some("s1".to_string()));
    assert!(idx.list().is_empty());
    assert_eq!(idx.remove("s1").unwrap(), None);
}

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
