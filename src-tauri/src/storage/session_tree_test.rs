//! 会话树存取引擎单元测试（独立测试文件，源码零测试）。
//!
//! 覆盖 §6.3/§6.4 语义：roundtrip、仅追加缺失节点、幂等全量同步、
//! revision 乐观并发、同 id 内容漂移拒绝、孤儿/未知叶参数校验、NotFound。

use std::fs;

use super::session_tree::{SessionStore, StorageError};
use crate::storage::entries::{
    CompleteReason, MessageEntry, MessageRole, MessageStatus, MetaEntry, SessionEntry, StoredEntry,
    WirePart,
};
use crate::storage::segment::SEGMENT_MAX_ENTRIES;

fn store() -> (SessionStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    (SessionStore::with_root(dir.path().to_path_buf()), dir)
}

fn user(id: &str, parent: Option<&str>, text: &str) -> MessageEntry {
    MessageEntry {
        id: id.to_string(),
        parent_id: parent.map(str::to_string),
        role: MessageRole::User,
        parts: vec![WirePart::Text {
            text: text.to_string(),
        }],
        created_at: 1760000000000,
        run_id: None,
        status: None,
    }
}

fn assistant(id: &str, parent: &str, text: &str) -> MessageEntry {
    MessageEntry {
        id: id.to_string(),
        parent_id: Some(parent.to_string()),
        role: MessageRole::Assistant,
        parts: vec![WirePart::Text {
            text: text.to_string(),
        }],
        created_at: 1760000000001,
        run_id: Some("run-1".to_string()),
        status: Some(MessageStatus::Complete {
            reason: CompleteReason::Unknown,
        }),
    }
}

fn tool_assistant(id: &str, parent: &str) -> MessageEntry {
    MessageEntry {
        parts: vec![
            WirePart::ToolCall {
                call_id: "call_1".to_string(),
                name: "read_workspace_file".to_string(),
                args_json: r#"{"path":"README.md"}"#.to_string(),
            },
            WirePart::ToolResult {
                call_id: "call_1".to_string(),
                ok: true,
                summary: "# CraftWave".to_string(),
            },
            WirePart::Text {
                text: "读完了".to_string(),
            },
        ],
        ..assistant(id, parent, "占位")
    }
}

// ---- roundtrip 与追加纪律 ----

#[test]
fn save_load_roundtrip_entries_equal() {
    let (store, _dir) = store();
    let messages = vec![
        user("u1", None, "你好"),
        assistant("a1", "u1", "你好！我是 CraftWave。"),
        user("u2", Some("a1"), "读一下 README"),
        tool_assistant("a2", "u2"),
    ];
    let revision = store
        .save_session_tree("s1", &messages, Some("a2"), 0)
        .unwrap();
    assert_eq!(revision, 1);

    let loaded = store.load_session_tree("s1").unwrap();
    assert_eq!(
        loaded.messages, messages,
        "逐条目一致（含 parts/status/runId）"
    );
    assert_eq!(loaded.active_leaf_id.as_deref(), Some("a2"));
    assert_eq!(loaded.revision, 1);
    assert_eq!(loaded.session_id, "s1");
}

#[test]
fn first_save_writes_single_session_header() {
    let (store, _dir) = store();
    store
        .save_session_tree("s1", &[user("u1", None, "你好")], Some("u1"), 0)
        .unwrap();
    let content =
        fs::read_to_string(store_root_dir(&_dir).join("histories/s1/0001.jsonl")).unwrap();
    let first: StoredEntry = serde_json::from_str(content.lines().next().unwrap()).unwrap();
    assert_eq!(
        first,
        StoredEntry::Session(SessionEntry {
            v: 1,
            id: "s1".into(),
            created_at: first_session_created(&content),
        })
    );
    assert_eq!(
        content
            .lines()
            .filter(|l| l.contains(r#""kind":"session""#))
            .count(),
        1,
        "重复 save 不再追加 session 头"
    );
}

fn store_root_dir(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().to_path_buf()
}

fn first_session_created(content: &str) -> i64 {
    let entry: StoredEntry = serde_json::from_str(content.lines().next().unwrap()).unwrap();
    match entry {
        StoredEntry::Session(s) => s.created_at,
        _ => panic!("首行应为 session 条目"),
    }
}

#[test]
fn idempotent_sync_repeat_save_appends_meta_only() {
    let (store, dir) = store();
    let messages = vec![user("u1", None, "你好"), assistant("a1", "u1", "回复")];
    let rev1 = store
        .save_session_tree("s1", &messages, Some("a1"), 0)
        .unwrap();
    let file = dir.path().join("histories/s1/0001.jsonl");
    let after_first = fs::read_to_string(&file).unwrap();

    let rev2 = store
        .save_session_tree("s1", &messages, Some("a1"), rev1)
        .unwrap();
    assert_eq!(rev2, 2);

    let after_second = fs::read_to_string(&file).unwrap();
    assert!(
        after_second.starts_with(after_first.trim_end()),
        "既有行字节不变（append-only），只追加新 meta 行"
    );
    let loaded = store.load_session_tree("s1").unwrap();
    assert_eq!(loaded.messages, messages, "消息不重复落盘");
    assert_eq!(loaded.revision, 2);
}

#[test]
fn branch_append_new_lines_existing_unchanged() {
    let (store, dir) = store();
    let v1 = vec![user("u1", None, "原问题"), assistant("a1", "u1", "原回复")];
    store.save_session_tree("s1", &v1, Some("a1"), 0).unwrap();
    let file = dir.path().join("histories/s1/0001.jsonl");
    let before = fs::read(&file).unwrap();

    // 编辑重发语义：u1 下兄弟分支 u1' → a1'
    let v2 = [
        v1.as_slice(),
        &[
            user("u1-edited", None, "编辑后的问题"),
            assistant("a1-edited", "u1-edited", "编辑后的回复"),
        ],
    ]
    .concat();
    let rev = store
        .save_session_tree("s1", &v2, Some("a1-edited"), 1)
        .unwrap();
    assert_eq!(rev, 2);
    assert!(
        fs::read(&file).unwrap().starts_with(&before),
        "追加式：新分支只落新行"
    );

    let loaded = store.load_session_tree("s1").unwrap();
    assert_eq!(loaded.messages.len(), 4);
    assert_eq!(loaded.active_leaf_id.as_deref(), Some("a1-edited"));
    // meta 最新赢：最后一次 meta 行的 activeLeafId
    let metas: Vec<MetaEntry> = loaded_messages_meta(&file);
    assert_eq!(metas.last().unwrap().revision, 2);
}

fn loaded_messages_meta(file: &std::path::Path) -> Vec<MetaEntry> {
    fs::read_to_string(file)
        .unwrap()
        .lines()
        .filter_map(|l| match serde_json::from_str::<StoredEntry>(l).unwrap() {
            StoredEntry::Meta(m) => Some(m),
            _ => None,
        })
        .collect()
}

#[test]
fn empty_tree_save_and_load() {
    let (store, _dir) = store();
    let rev = store.save_session_tree("s1", &[], None, 0).unwrap();
    assert_eq!(rev, 1);
    let loaded = store.load_session_tree("s1").unwrap();
    assert!(loaded.messages.is_empty());
    assert_eq!(loaded.active_leaf_id, None);
    assert_eq!(loaded.revision, 1);
}

#[test]
fn multi_save_across_segments_latest_revision_wins() {
    let (store, _dir) = store();
    // 第一轮：写满一段触发封口
    let mut messages = vec![user("u1", None, "根")];
    let mut rev = store
        .save_session_tree("s1", &messages, Some("u1"), 0)
        .unwrap();
    for i in 0..SEGMENT_MAX_ENTRIES {
        let id = format!("m{i}");
        messages.push(user(&id, Some("u1"), "分支"));
        rev = store
            .save_session_tree("s1", &messages, Some(&id), rev)
            .unwrap();
    }
    assert!(rev > 1);
    let loaded = store.load_session_tree("s1").unwrap();
    assert_eq!(loaded.messages, messages, "跨段 roundtrip 无损");
    assert_eq!(loaded.revision, rev);
    let expected_leaf = format!("m{}", SEGMENT_MAX_ENTRIES - 1);
    assert_eq!(
        loaded.active_leaf_id.as_deref(),
        Some(expected_leaf.as_str())
    );
}

// ---- 并发与校验（§6.4） ----

#[test]
fn stale_base_revision_conflict() {
    let (store, _dir) = store();
    store
        .save_session_tree("s1", &[user("u1", None, "你好")], Some("u1"), 0)
        .unwrap();
    let err = store
        .save_session_tree("s1", &[user("u1", None, "你好")], Some("u1"), 99)
        .unwrap_err();
    assert_eq!(
        err,
        StorageError::RevisionConflict {
            expected: 99,
            actual: 1
        },
        "§6.4：baseRevision 不等当前值 → E_SESSION_REV"
    );
}

#[test]
fn same_id_content_drift_conflict() {
    let (store, _dir) = store();
    store
        .save_session_tree("s1", &[user("u1", None, "v1")], Some("u1"), 0)
        .unwrap();
    let err = store
        .save_session_tree("s1", &[user("u1", None, "v2（被改）")], Some("u1"), 1)
        .unwrap_err();
    assert!(
        matches!(err, StorageError::RevisionConflict { .. }),
        "§6.3：内容哈希变 → E_SESSION_REV"
    );
}

#[test]
fn orphan_message_unknown_leaf_invalid_arg() {
    let (store, _dir) = store();
    let err = store
        .save_session_tree("s1", &[user("u2", Some("ghost"), "孤儿")], None, 0)
        .unwrap_err();
    assert!(
        matches!(err, StorageError::InvalidArg(_)),
        "导出序必须父先子后"
    );

    let err = store
        .save_session_tree("s2", &[user("u1", None, "你好")], Some("ghost-leaf"), 0)
        .unwrap_err();
    assert!(matches!(err, StorageError::InvalidArg(_)));
}

#[test]
fn invalid_session_id_rejected() {
    let (store, _dir) = store();
    for bad in ["", "../evil", "a/b", "a\\b", ".."] {
        let err = store.save_session_tree(bad, &[], None, 0).unwrap_err();
        assert!(matches!(err, StorageError::InvalidArg(_)), "{bad:?}");
    }
}

#[test]
fn load_missing_session_not_found() {
    let (store, _dir) = store();
    let err = store.load_session_tree("missing").unwrap_err();
    assert_eq!(err, StorageError::NotFound("missing".to_string()));
}

#[test]
fn load_result_serializes_camel_contract() {
    let (store, _dir) = store();
    store
        .save_session_tree("s1", &[user("u1", None, "你好")], Some("u1"), 0)
        .unwrap();
    let data = store.load_session_tree("s1").unwrap();
    let json = serde_json::to_value(&data).unwrap();
    assert_eq!(json["sessionId"], "s1");
    assert_eq!(json["revision"], 1);
    assert_eq!(json["activeLeafId"], "u1");
    assert_eq!(json["messages"][0]["parts"][0]["type"], "text");
    assert!(
        json["messages"][0].get("parentId").is_none(),
        "null→省略字段"
    );
}

// ---- 15 号票：canonical 追加 + 崩溃恢复 + index + toolres ----

fn canonical_turn(run_id: &str, turn: u32, parent: Option<&str>, text: &str) -> MessageEntry {
    MessageEntry {
        id: format!("{run_id}:{turn}"),
        parent_id: parent.map(str::to_string),
        role: MessageRole::Assistant,
        parts: vec![WirePart::Text {
            text: text.to_string(),
        }],
        created_at: 1760000000002,
        run_id: Some(run_id.to_string()),
        status: Some(MessageStatus::Complete {
            reason: CompleteReason::Stop,
        }),
    }
}

#[test]
fn append_canonical_advances_revision_and_active_leaf() {
    let (store, _dir) = store();
    // 首次 canonical 追加：自动写 session 头，revision 1，activeLeaf=批内最后条目
    let (rev, count) = store
        .append_canonical("s1", &[StoredEntry::Message(user("u1", None, "你好"))])
        .unwrap();
    assert_eq!((rev, count), (1, 1));
    let (rev2, count2) = store
        .append_canonical(
            "s1",
            &[StoredEntry::Message(canonical_turn(
                "run-1",
                1,
                Some("u1"),
                "答复",
            ))],
        )
        .unwrap();
    assert_eq!((rev2, count2), (2, 2));
    // run 终态 marker（runEnded 落盘点）后 activeLeaf=marker 且不再触发崩溃修复
    let (rev3, count3) = store
        .append_canonical("s1", &[StoredEntry::Message(marker_entry("run-1"))])
        .unwrap();
    assert_eq!((rev3, count3), (3, 3));
    let tree = store.load_session_tree("s1").unwrap();
    assert_eq!(tree.active_leaf_id.as_deref(), Some("run-1:marker"));
    assert_eq!(tree.messages.len(), 3, "含 marker，无额外修复行");
    // 头条目存在（首行 session）
    let raw =
        fs::read_to_string(_dir.path().join("histories").join("s1").join("0001.jsonl")).unwrap();
    assert!(raw.contains(r#""kind":"session""#), "首行应为 session 头");
}

fn marker_entry(run_id: &str) -> MessageEntry {
    MessageEntry {
        id: format!("{run_id}:marker"),
        parent_id: None,
        role: MessageRole::System,
        parts: vec![WirePart::RunMarker {
            run_id: run_id.to_string(),
            started_at: 1760000000002,
            ended_at: Some(1760000000003),
            duration_ms: Some(1),
            tool_count: Some(0),
        }],
        created_at: 1760000000003,
        run_id: None,
        status: None,
    }
}

#[test]
fn append_canonical_empty_batch_returns_current_state() {
    let (store, _dir) = store();
    store
        .append_canonical("s1", &[StoredEntry::Message(user("u1", None, "你好"))])
        .unwrap();
    let (rev, count) = store.append_canonical("s1", &[]).unwrap();
    assert_eq!((rev, count), (1, 1));
}

#[test]
fn load_repairs_interrupted_run_with_marker_row() {
    let (store, _dir) = store();
    // 崩溃现场：canonical turn 条目已落、无 marker（进程在 runEnded 落盘前死）
    store
        .append_canonical(
            "s1",
            &[
                StoredEntry::Message(user("u1", None, "你好")),
                StoredEntry::Message(canonical_turn("run-9", 1, Some("u1"), "部分答复")),
            ],
        )
        .unwrap();
    let tree = store.load_session_tree("s1").unwrap();
    // 修复行：endedAt=None（interrupted 语义）
    let marker = tree
        .messages
        .iter()
        .find(|m| m.id == "run-9:marker")
        .expect("应补 interrupted marker");
    assert!(
        marker
            .parts
            .iter()
            .any(|p| matches!(p, WirePart::RunMarker { ended_at: None, .. }))
    );
    // 幂等：再次 load 不再补
    let tree2 = store.load_session_tree("s1").unwrap();
    assert_eq!(
        tree2
            .messages
            .iter()
            .filter(|m| m.id == "run-9:marker")
            .count(),
        1
    );
}

#[test]
fn load_skips_repair_for_non_canonical_run_entries() {
    let (store, _dir) = store();
    // save_session_tree 直接落的条目（前端 id 带 runId）：不触发崩溃修复
    store
        .save_session_tree(
            "s1",
            &[user("u1", None, "你好"), assistant("a1", "u1", "答复")],
            Some("a1"),
            0,
        )
        .unwrap();
    let tree = store.load_session_tree("s1").unwrap();
    assert!(
        !tree.messages.iter().any(|m| m.id.ends_with(":marker")),
        "非 canonical 形态条目不应触发修复"
    );
}

#[test]
fn index_upsert_list_and_toolres_roundtrip() {
    let (store, dir) = store();
    let root = dir.path().to_path_buf();
    // index：首建 → 更新（totalTokens 累加）
    let now = 1760000000000i64;
    store
        .index()
        .upsert(crate::storage::SessionMeta {
            id: "s1".to_string(),
            project_id: None,
            title: "未命名会话".to_string(),
            created_at: now,
            updated_at: now,
            revision: 1,
            message_count: 2,
            total_tokens: 100,
        })
        .unwrap();
    store
        .index()
        .upsert(crate::storage::SessionMeta {
            id: "s2".to_string(),
            project_id: None,
            title: "未命名会话".to_string(),
            created_at: now,
            updated_at: now + 1,
            revision: 1,
            message_count: 1,
            total_tokens: 50,
        })
        .unwrap();
    let list = store.index().list();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, "s2", "updatedAt 倒序");
    // toolres：写 → 读 roundtrip；缺文件调用不在返回表
    let outcome = crate::tools::ToolOutcome::success(
        serde_json::json!({ "lineCount": 3 }),
        "读取完成".to_string(),
    );
    crate::storage::write_outcome(&root, "s1", "t1-c1", &outcome);
    let loaded =
        crate::storage::load_outcomes(&root, "s1", &["t1-c1".to_string(), "missing".to_string()]);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded["t1-c1"]["uiSummary"], "读取完成");
    assert_eq!(loaded["t1-c1"]["data"]["lineCount"], 3);
}

// ---- 会话删除（16 号票，§3.3 #16 存储面）----

#[test]
fn delete_session_clears_index_and_disk_artifacts() {
    let (store, dir) = store();
    let root = dir.path().to_path_buf();
    // 造全量磁盘痕迹：历史树 + index + prefs + toolres
    store
        .save_session_tree("gone", &[user("u1", None, "再见")], None, 0)
        .unwrap();
    store
        .index()
        .upsert(crate::storage::SessionMeta {
            id: "gone".to_string(),
            project_id: Some("p1".to_string()),
            title: "待删会话".to_string(),
            created_at: 0,
            updated_at: 0,
            revision: 1,
            message_count: 1,
            total_tokens: 0,
        })
        .unwrap();
    fs::write(root.join("sessions").join("gone.prefs.json"), b"{}").unwrap();
    let outcome = crate::tools::ToolOutcome::success(serde_json::json!({}), "结果".to_string());
    crate::storage::write_outcome(&root, "gone", "call-1", &outcome);

    assert!(store.delete_session("gone").unwrap());
    assert_eq!(store.index().get("gone"), None);
    assert!(!root.join("histories").join("gone").exists());
    assert!(!root.join("sessions").join("gone.prefs.json").exists());
    assert!(!root.join("sessions").join("gone.toolres").exists());
}

#[test]
fn delete_missing_session_is_idempotent_false() {
    let (store, _dir) = store();
    assert!(!store.delete_session("never-existed").unwrap());
}

#[test]
fn delete_session_rejects_invalid_id() {
    let (store, _dir) = store();
    let err = store.delete_session("../escape").unwrap_err();
    assert!(matches!(err, crate::storage::StorageError::InvalidArg(_)));
}
