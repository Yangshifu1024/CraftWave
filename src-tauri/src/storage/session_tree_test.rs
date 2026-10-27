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
fn save_load_roundtrip_逐条目一致() {
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
fn 首存自动写session头条目且仅一条() {
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
fn 幂等全量同步_重复save仅追加meta行() {
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
fn 追加分支只落新行_既有行字节不变() {
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
fn 空树保存与加载() {
    let (store, _dir) = store();
    let rev = store.save_session_tree("s1", &[], None, 0).unwrap();
    assert_eq!(rev, 1);
    let loaded = store.load_session_tree("s1").unwrap();
    assert!(loaded.messages.is_empty());
    assert_eq!(loaded.active_leaf_id, None);
    assert_eq!(loaded.revision, 1);
}

#[test]
fn 多轮save跨段后load一致_revision最新赢() {
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
fn 基线修订号过期返回冲突() {
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
fn 同id内容漂移返回冲突() {
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
fn 孤儿消息与未知活动叶返回参数错误() {
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
fn 非法会话id被拒绝() {
    let (store, _dir) = store();
    for bad in ["", "../evil", "a/b", "a\\b", ".."] {
        let err = store.save_session_tree(bad, &[], None, 0).unwrap_err();
        assert!(matches!(err, StorageError::InvalidArg(_)), "{bad:?}");
    }
}

#[test]
fn 加载不存在会话返回未找到() {
    let (store, _dir) = store();
    let err = store.load_session_tree("missing").unwrap_err();
    assert_eq!(err, StorageError::NotFound("missing".to_string()));
}

#[test]
fn 加载返回结构可序列化为驼峰契约() {
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
