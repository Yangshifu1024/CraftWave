//! 会话树存取引擎单元测试（独立测试文件，源码零测试）。
//!
//! 覆盖 语义：save/load 往返一致（roundtrip，指保存后读回逐字段相等）、
//! 仅追加缺失节点、幂等全量同步、revision 乐观并发冲突、孤儿节点/未知活动叶/
//! 非法会话 id 参数校验、NotFound；以及 的 canonical 追加、崩溃恢复
//!（interrupted marker 修复）、索引与 toolres 边车往返、的会话删除清理、
//! 的 compaction 条目与 升级备份守卫。同型用例在单个 #[test] 内
//! 表驱动合并，断言消息带用例标识。

use std::fs;

use super::session_tree::{SessionStore, StorageError};
use crate::storage::entries::{
    CompleteReason, MessageEntry, MessageRole, MessageStatus, MetaEntry, SessionEntry, StoredEntry,
    WirePart,
};
use crate::storage::{StoredCompactionEntry, StoredEntry as Entry};

// 构造一个使用临时数据根的存储器实例（返回 TempDir 供测试检查磁盘文件）。
fn store() -> (SessionStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    (SessionStore::with_root(dir.path().to_path_buf()), dir)
}

// 取临时目录路径的辅助函数。
fn store_root_dir(dir: &tempfile::TempDir) -> std::path::PathBuf {
    dir.path().to_path_buf()
}

// 构造一条 user 角色消息的辅助函数。
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

// 构造一条 assistant 回复消息（带 runId 与 complete 状态）的辅助函数。
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

// 构造一条含工具调用 + 工具结果 + 文字三种 part 的 assistant 消息的辅助函数。
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

// 解析日志首行 session 条目的 created_at，用于比对期望值的辅助函数。
fn first_session_created(content: &str) -> i64 {
    let entry: StoredEntry = serde_json::from_str(content.lines().next().unwrap()).unwrap();
    match entry {
        StoredEntry::Session(s) => s.created_at,
        _ => panic!("首行应为 session 条目"),
    }
}

// 收集日志中全部 meta 行的辅助函数（按出现顺序）。
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

// ---- roundtrip、追加纪律与契约形状 ----

// 断言：save/load 全链路语义——roundtrip 逐条相等、session 头只写一次、幂等重
// 保存只追加 meta 行、分支追加既有行字节不变、空树可存读、load 序列化为
// camelCase 契约形状（null 字段省略）。
#[test]
fn save_load_roundtrip_and_append_discipline() {
    // roundtrip：逐条目一致（含 parts/status/runId），叶与修订号正确
    let (store, dir) = store();
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

    // 首行 session 头，且重复 save 不再追加新的头
    let file = store_root_dir(&dir).join("histories/s1/0001.jsonl");
    let content = fs::read_to_string(&file).unwrap();
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

    // 幂等重保存：既有行字节不变、只追加新 meta 行、消息不重复
    let after_first = content;
    let rev2 = store
        .save_session_tree("s1", &messages, Some("a2"), 1)
        .unwrap();
    assert_eq!(rev2, 2);
    assert!(
        fs::read_to_string(&file)
            .unwrap()
            .starts_with(after_first.trim_end()),
        "既有行字节不变（append-only），只追加新 meta 行"
    );
    let loaded = store.load_session_tree("s1").unwrap();
    assert_eq!(loaded.messages, messages, "消息不重复落盘");
    assert_eq!(loaded.revision, 2);

    // 分支追加（编辑重发）：新分支只落新行，meta 最新赢
    let before = fs::read(&file).unwrap();
    let v2 = [
        messages.as_slice(),
        &[
            user("u1-edited", None, "编辑后的问题"),
            assistant("a1-edited", "u1-edited", "编辑后的回复"),
        ],
    ]
    .concat();
    let rev = store
        .save_session_tree("s1", &v2, Some("a1-edited"), 2)
        .unwrap();
    assert_eq!(rev, 3);
    assert!(
        fs::read(&file).unwrap().starts_with(&before),
        "追加式：新分支只落新行"
    );
    let loaded = store.load_session_tree("s1").unwrap();
    assert_eq!(loaded.messages.len(), 6);
    assert_eq!(loaded.active_leaf_id.as_deref(), Some("a1-edited"));
    assert_eq!(
        loaded_messages_meta(&file).last().unwrap().revision,
        3,
        "meta 最新赢"
    );

    // 空树：保存成功、读回为空、无活跃叶
    let rev = store.save_session_tree("s-empty", &[], None, 0).unwrap();
    assert_eq!(rev, 1);
    let empty_loaded = store.load_session_tree("s-empty").unwrap();
    assert!(empty_loaded.messages.is_empty());
    assert_eq!(empty_loaded.active_leaf_id, None);
    assert_eq!(empty_loaded.revision, 1);

    // camelCase 契约形状：sessionId/activeLeafId 等，parentId 为 null 时字段省略
    let json = serde_json::to_value(&loaded).unwrap();
    assert_eq!(json["sessionId"], "s1");
    assert_eq!(json["revision"], 3);
    assert_eq!(json["activeLeafId"], "a1-edited");
    assert_eq!(json["messages"][0]["parts"][0]["type"], "text");
    assert!(
        json["messages"][0].get("parentId").is_none(),
        "null→省略字段"
    );
}

// ---- 参数校验与冲突（表驱动）----

// 断言：save 的校验与冲突面——baseRevision 过期、同 id 内容漂移均报
// RevisionConflict；孤儿消息、未知活动叶报 InvalidArg；路径穿越等非法会话
// id（用作目录名）被拒绝；加载不存在的会话报 NotFound。
#[test]
fn save_validation_errors() {
    let (store, _dir) = store();

    // baseRevision 落后于当前修订号（基线过期）→ E_SESSION_REV
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
        "：baseRevision 不等当前值 → E_SESSION_REV"
    );

    // 同 id 消息内容与已存版本不一致（内容漂移）→ E_SESSION_REV
    let err = store
        .save_session_tree("s1", &[user("u1", None, "v2（被改）")], Some("u1"), 1)
        .unwrap_err();
    assert!(
        matches!(err, StorageError::RevisionConflict { .. }),
        "：内容哈希变 → E_SESSION_REV"
    );

    // 孤儿消息（父节点不存在）→ InvalidArg（导出序必须父先子后）
    let err = store
        .save_session_tree("s2", &[user("u2", Some("ghost"), "孤儿")], None, 0)
        .unwrap_err();
    assert!(
        matches!(err, StorageError::InvalidArg(_)),
        "导出序必须父先子后"
    );

    // 活跃叶不在树中 → InvalidArg
    let err = store
        .save_session_tree("s2", &[user("u1", None, "你好")], Some("ghost-leaf"), 0)
        .unwrap_err();
    assert!(matches!(err, StorageError::InvalidArg(_)), "未知活动叶");

    // 非法会话 id（仅允许字母数字连字符下划线，防路径穿越）
    for bad in ["", "../evil", "a/b", "a\\b", ".."] {
        let err = store.save_session_tree(bad, &[], None, 0).unwrap_err();
        assert!(matches!(err, StorageError::InvalidArg(_)), "{bad:?}");
    }

    // 加载不存在的会话 → NotFound
    let err = store.load_session_tree("missing").unwrap_err();
    assert_eq!(err, StorageError::NotFound("missing".to_string()));
}

// ---- ：canonical 追加 + 崩溃恢复 ----

// 构造 canonical 形态一轮对话中第 turn 条 assistant 消息（id 规则 `{runId}:{turn}`）。
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

// 构造 run 终态 marker 消息（runMarker part，id 规则 `{runId}:marker`）的辅助函数。
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

// 断言：canonical 追加语义——自动写 session 头、revision 与消息计数递增、
// activeLeaf 推进为批内最后一条消息、空批次零追加只读返回；run 终态 marker
// 落盘后不再触发崩溃修复，崩溃现场（有 run 条目无 marker）在 load 时补
// interrupted marker（幂等），非 canonical 形态条目不触发修复。
#[test]
fn canonical_append_and_crash_recovery() {
    let (store, dir) = store();
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
    // run 终态 marker 落盘后 activeLeaf=marker
    let (rev3, count3) = store
        .append_canonical("s1", &[StoredEntry::Message(marker_entry("run-1"))])
        .unwrap();
    assert_eq!((rev3, count3), (3, 3));
    let tree = store.load_session_tree("s1").unwrap();
    assert_eq!(tree.active_leaf_id.as_deref(), Some("run-1:marker"));
    assert_eq!(tree.messages.len(), 3, "含 marker，无额外修复行");
    let raw =
        fs::read_to_string(dir.path().join("histories").join("s1").join("0001.jsonl")).unwrap();
    assert!(raw.contains(r#""kind":"session""#), "首行应为 session 头");

    // 空批次：不追加任何行，只读返回当前 revision 与消息数
    let (rev, count) = store.append_canonical("s1", &[]).unwrap();
    assert_eq!((rev, count), (3, 3), "空批次零追加");

    // 崩溃修复：canonical turn 已落、无 marker → load 补 endedAt=None 的修复行
    store
        .append_canonical(
            "s2",
            &[
                StoredEntry::Message(user("u1", None, "你好")),
                StoredEntry::Message(canonical_turn("run-9", 1, Some("u1"), "部分答复")),
            ],
        )
        .unwrap();
    let tree = store.load_session_tree("s2").unwrap();
    let marker = tree
        .messages
        .iter()
        .find(|m| m.id == "run-9:marker")
        .expect("应补 interrupted marker");
    assert!(
        marker
            .parts
            .iter()
            .any(|p| matches!(p, WirePart::RunMarker { ended_at: None, .. })),
        "修复行 endedAt=None（interrupted 语义）"
    );
    // 修复幂等：再次 load 不再重复补
    let tree2 = store.load_session_tree("s2").unwrap();
    assert_eq!(
        tree2
            .messages
            .iter()
            .filter(|m| m.id == "run-9:marker")
            .count(),
        1,
        "修复幂等"
    );

    // 非 canonical 形态（save_session_tree 直接落、id 不带 {runId}: 前缀）不触发修复
    store
        .save_session_tree(
            "s3",
            &[user("u1", None, "你好"), assistant("a1", "u1", "答复")],
            Some("a1"),
            0,
        )
        .unwrap();
    let tree = store.load_session_tree("s3").unwrap();
    assert!(
        !tree.messages.iter().any(|m| m.id.ends_with(":marker")),
        "非 canonical 形态条目不应触发修复"
    );
}

// ---- ：index/toolres 边车 + 会话删除 + save 同步索引 ----

// 断言：索引与删除面——index upsert/list 按 updatedAt 倒序；toolres 边车写后
// 可读回（缺文件调用不在返回表）；save 落盘新消息同步索引（缺条目兜底建档、
// 既有条目只推进计数保留域字段、幂等 save 不动 updatedAt）；delete_session
// 清掉索引条目/历史目录/prefs/toolres 且幂等、非法 id 被拒。
#[test]
fn index_toolres_delete_and_sync() {
    let (store, dir) = store();
    let root = dir.path().to_path_buf();
    let now = 1760000000000i64;
    let meta_of = |id: &str, updated_at: i64| crate::storage::SessionMeta {
        id: id.to_string(),
        project_id: None,
        title: "未命名会话".to_string(),
        created_at: now,
        updated_at,
        revision: 1,
        message_count: 2,
        total_tokens: 100,
        authorized_external_dirs: Vec::new(),
    };
    // index：两条后按 updatedAt 倒序
    store.index().upsert(meta_of("s1", now)).unwrap();
    store.index().upsert(meta_of("s2", now + 1)).unwrap();
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
    assert_eq!(loaded.len(), 1, "缺文件的调用编号不出现");
    assert_eq!(loaded["t1-c1"]["uiSummary"], "读取完成");
    assert_eq!(loaded["t1-c1"]["data"]["lineCount"], 3);

    // save 同步索引：缺条目兜底建档
    store
        .save_session_tree("s3", &[user("u1", None, "你好")], Some("u1"), 0)
        .unwrap();
    let meta = store.index().get("s3").expect("索引应有兜底条目");
    assert_eq!(meta.message_count, 1);
    assert_eq!(meta.revision, 1);

    // save 同步索引：既有条目只推进计数/修订/updatedAt，域字段原样保留
    store
        .index()
        .upsert(crate::storage::SessionMeta {
            id: "s4".to_string(),
            project_id: Some("p1".to_string()),
            title: "我的会话".to_string(),
            created_at: 1000,
            updated_at: 1000,
            revision: 0,
            message_count: 0,
            total_tokens: 42,
            authorized_external_dirs: Vec::new(),
        })
        .unwrap();
    store
        .save_session_tree(
            "s4",
            &[user("u1", None, "你好"), assistant("a1", "u1", "回复")],
            Some("a1"),
            0,
        )
        .unwrap();
    let meta = store.index().get("s4").unwrap();
    assert_eq!(meta.message_count, 2);
    assert_eq!(meta.revision, 1);
    assert_eq!(meta.project_id.as_deref(), Some("p1"), "域字段保留");
    assert_eq!(meta.title, "我的会话");
    assert_eq!(meta.created_at, 1000);
    assert_eq!(meta.total_tokens, 42);
    assert!(meta.updated_at >= 1000, "updatedAt 应推进到 save 时刻");

    // 幂等 save（无新节点）不推进 updatedAt（打开旧会话不发言不跳列表顶部）
    let before = store.index().get("s4").unwrap().updated_at;
    std::thread::sleep(std::time::Duration::from_millis(5));
    store
        .save_session_tree(
            "s4",
            &[user("u1", None, "你好"), assistant("a1", "u1", "回复")],
            Some("a1"),
            1,
        )
        .unwrap();
    assert_eq!(
        store.index().get("s4").unwrap().updated_at,
        before,
        "幂等 save 不动索引"
    );

    // 删除会话：造全量磁盘痕迹（历史树 + index + prefs + toolres）后全清
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
            authorized_external_dirs: Vec::new(),
        })
        .unwrap();
    fs::write(root.join("sessions").join("gone.prefs.json"), b"{}").unwrap();
    let outcome = crate::tools::ToolOutcome::success(serde_json::json!({}), "结果".to_string());
    crate::storage::write_outcome(&root, "gone", "call-1", &outcome);

    assert!(store.delete_session("gone").unwrap());
    assert_eq!(store.index().get("gone"), None, "索引条目清除");
    assert!(
        !root.join("histories").join("gone").exists(),
        "历史目录清除"
    );
    assert!(
        !root.join("sessions").join("gone.prefs.json").exists(),
        "prefs 清除"
    );
    assert!(
        !root.join("sessions").join("gone.toolres").exists(),
        "toolres 清除"
    );
    // 删除不存在 → Ok(false)；删除同样校验非法 id
    assert!(
        !store.delete_session("never-existed").unwrap(),
        "幂等 false"
    );
    let err = store.delete_session("../escape").unwrap_err();
    assert!(matches!(err, crate::storage::StorageError::InvalidArg(_)));
}

// ---- compaction 条目----

/// 造一条 compaction 条目。
fn compaction_entry(cut: &str, summary: &str) -> StoredCompactionEntry {
    StoredCompactionEntry {
        id: format!("manual:compact:{cut}"),
        cut_entry_id: cut.to_owned(),
        summary: summary.to_owned(),
        tokens_before: 1234,
        created_at: 42,
    }
}

/// 断言：compaction 全链路——条目 serde roundtrip（kind=compaction、camelCase）；
/// append_compaction 落盘 + load 投影（messages 不含、compactions 按追加序、
/// activeLeaf 不变、revision 推进、overview 取最近）+ camelCase 序列化面；
/// 备份守卫首次压缩创建可恢复备份、标记存在后不再重复备份。
#[test]
fn compaction_serde_projection_and_upgrade_backup() {
    // serde：行字节即契约
    let entry = Entry::Compaction(compaction_entry("a1", "压缩摘要"));
    let line = crate::storage::segment::entry_line(&entry).unwrap();
    assert!(
        line.starts_with(r#"{"kind":"compaction","id":"manual:compact:a1","cutEntryId":"a1""#),
        "行字节即契约：{line}"
    );
    let back: Entry = serde_json::from_str(&line).unwrap();
    assert_eq!(back, entry, "行往返等值");

    // 落盘 + 投影 + overview + 序列化面
    let (store, dir) = store();
    store
        .save_session_tree(
            "s",
            &[user("u1", None, "甲"), assistant("a1", "u1", "乙")],
            Some("a1"),
            0,
        )
        .unwrap();
    let revision_before = store.load_session_tree("s").unwrap().revision;
    store
        .append_compaction("s", compaction_entry("a1", "第一轮摘要"))
        .unwrap();
    let tree = store.load_session_tree("s").unwrap();
    assert_eq!(
        tree.messages
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        vec!["u1", "a1"],
        "message 面不含 compaction"
    );
    assert_eq!(
        tree.compactions.len(),
        1,
        "compactions 投影：{:?}",
        tree.compactions
    );
    assert_eq!(tree.compactions[0].cut_entry_id, "a1");
    assert_eq!(tree.compactions[0].summary, "第一轮摘要");
    assert_eq!(
        tree.active_leaf_id.as_deref(),
        Some("a1"),
        "压缩条目不改变活动叶"
    );
    assert!(tree.revision > revision_before, "追加即 revision++");
    // camelCase 序列化面（前端投影消费）
    let json = serde_json::to_value(&tree).unwrap();
    assert_eq!(
        json.get("compactions")
            .and_then(|v| v.as_array())
            .map(|a| a.len()),
        Some(1),
        "compactions 字段应在序列化面：{json}"
    );
    assert!(
        json["compactions"][0].get("cutEntryId").is_some(),
        "字段 camelCase：{json}"
    );

    // 第二次压缩（增量场景）：compactions 按追加序，最近在末位
    store
        .append_compaction("s", compaction_entry("a1", "合并后摘要"))
        .unwrap();
    let tree = store.load_session_tree("s").unwrap();
    assert_eq!(tree.compactions.len(), 2);
    assert_eq!(tree.compactions[1].summary, "合并后摘要");
    let (latest, count) = store.compaction_overview("s").unwrap();
    assert_eq!(count, 2);
    assert_eq!(latest.expect("有最近条目").summary, "合并后摘要");

    // 备份守卫：首次 append_compaction 前建备份（histories/config/index），
    // 标记存在后不再重复备份
    let root = store_root_dir(&dir);
    let backups = root.join("backups");
    let marker = backups.join(".upgrade-compaction.done");
    assert!(marker.exists(), "升级标记应存在");
    let recorded = std::fs::read_to_string(&marker).unwrap();
    let backup_dir = backups.join(recorded.trim());
    assert!(
        backup_dir.join("histories").join("s").exists(),
        "会话日志进了备份"
    );
    assert!(
        backup_dir.join("sessions").join("index.json").exists(),
        "索引进了备份"
    );
    let pre_dirs = std::fs::read_dir(&backups).unwrap().count();
    store
        .append_compaction("s", compaction_entry("u1", "第三次摘要"))
        .unwrap();
    let post_dirs = std::fs::read_dir(&backups).unwrap().count();
    assert_eq!(pre_dirs, post_dirs, "一次升级只备一次");
}
