//! 分段 JSONL 读写器单元测试（独立测试文件，源码零测试）。
//!
//! 测试面覆盖：①序列化字节形状——条目判别键 `kind`、字段 camelCase、可选字段
//! 省略规则必须与前端 TypeScript 镜像逐字节一致（跨语言契约）；②打开/追加/封口
//! 的正常流程与往返一致性；③两个封口阈值（200 条、512KB）触发的段滚动；④崩溃
//! 容错——半行/非法行只允许出现在最后一段且必须跳过、绝不续写损坏段；⑤指纹
//! 校验——篡改已封口段后指纹必须失配，且失配后只开新段不截断旧段。

use std::fs;

use super::entries::{
    CompleteReason, IncompleteReason, MessageEntry, MessageRole, MessageStatus, MetaEntry,
    SessionEntry, StoredEntry, WirePart,
};
use super::segment::{SEGMENT_MAX_ENTRIES, Segmeta, SessionLog, entry_line};

// 构造一条 user 消息条目的快捷辅助函数。
fn user_message(id: &str, parent: Option<&str>, text: &str) -> StoredEntry {
    StoredEntry::Message(MessageEntry {
        id: id.to_string(),
        parent_id: parent.map(str::to_string),
        role: MessageRole::User,
        parts: vec![WirePart::Text {
            text: text.to_string(),
        }],
        created_at: 1760000000000,
        run_id: None,
        status: None,
    })
}

// 构造一条 meta 条目（含指定修订号 revision 与当前活跃叶子消息 id）的辅助函数。
fn meta(revision: u64, leaf: Option<&str>) -> StoredEntry {
    StoredEntry::Meta(MetaEntry {
        active_leaf_id: leaf.map(str::to_string),
        title: None,
        revision,
    })
}

// 标准样例日志：一条 session 头条目 + 一条 user 消息，多个测试复用。
fn sample_log() -> Vec<StoredEntry> {
    vec![
        StoredEntry::Session(SessionEntry {
            v: 1,
            id: "s1".into(),
            created_at: 1760000000000,
        }),
        user_message("u1", None, "你好"),
    ]
}

// ---- 序列化形状（跨语言契约的字节级口径） ----

// 断言：消息条目序列化后判别键为 `kind`、字段为 camelCase、值为 None 的字段整段省略。
#[test]
fn entry_line_kind_tag_camel_fields() {
    let line = entry_line(&sample_log()[1]).unwrap();
    assert!(line.starts_with(r#"{"kind":"message""#), "tag=kind: {line}");
    assert!(line.contains(r#""createdAt":"#), "camelCase: {line}");
    assert!(!line.contains("parent_id"), "None 字段必须省略: {line}");

    let with_parent = user_message("u2", Some("u1"), "继续");
    let line = entry_line(&with_parent).unwrap();
    assert!(line.contains(r#""parentId":"u1""#), "{line}");
}

// 断言：toolCall/toolResult part 与 complete 状态的序列化形状与 TS 镜像逐字节一致。
#[test]
fn tool_part_and_status_shape() {
    let entry = StoredEntry::Message(MessageEntry {
        id: "a1".into(),
        parent_id: Some("u1".into()),
        role: MessageRole::Assistant,
        parts: vec![
            WirePart::ToolCall {
                call_id: "call_1".into(),
                name: "read_workspace_file".into(),
                args_json: r#"{"path":"README.md"}"#.into(),
            },
            WirePart::ToolResult {
                call_id: "call_1".into(),
                ok: true,
                summary: "12 行".into(),
            },
        ],
        created_at: 1760000000001,
        run_id: Some("run-1".into()),
        status: Some(MessageStatus::Complete {
            reason: CompleteReason::Unknown,
        }),
    });
    let line = entry_line(&entry).unwrap();
    assert!(line.contains(
        r#""type":"toolCall","callId":"call_1","name":"read_workspace_file","argsJson":"#
    ));
    assert!(line.contains(r#""type":"toolResult","callId":"call_1","ok":true,"summary":"12 行""#));
    assert!(
        line.contains(r#""status":{"type":"complete","reason":"unknown"}"#),
        "{line}"
    );
    assert!(line.contains(r#""runId":"run-1""#), "{line}");
}

// 断言：requires-action/incomplete 状态与 planSnapshot part 的判别键为 kebab-case、字段为 camelCase。
#[test]
fn requires_action_incomplete_plan_items_shape() {
    let status = MessageStatus::RequiresAction {
        reason: super::entries::RequiresActionReason::ToolCalls,
    };
    let line = serde_json::to_string(&status).unwrap();
    assert_eq!(line, r#"{"type":"requires-action","reason":"tool-calls"}"#);

    let status = MessageStatus::Incomplete {
        reason: IncompleteReason::ContentFilter,
        error: Some(serde_json::json!({ "msg": "x" })),
    };
    let line = serde_json::to_string(&status).unwrap();
    assert_eq!(
        line,
        r#"{"type":"incomplete","reason":"content-filter","error":{"msg":"x"}}"#
    );

    let entry = StoredEntry::Message(MessageEntry {
        id: "a3".into(),
        parent_id: None,
        role: MessageRole::Assistant,
        parts: vec![WirePart::PlanSnapshot {
            items: vec![super::entries::PlanItem {
                id: "p1".into(),
                text: "第一步".into(),
                status: super::entries::PlanItemStatus::InProgress,
            }],
        }],
        created_at: 5,
        run_id: None,
        status: None,
    });
    let line = entry_line(&entry).unwrap();
    assert!(
        line.contains(r#"{"type":"planSnapshot","items":[{"id":"p1","text":"第一步","status":"in_progress"}]}"#),
        "{line}"
    );
}

// ---- 打开/追加/封口 ----

// 断言：追加后重新打开能完整读回同一批条目（干净落盘、无损坏、指纹通过）。
#[test]
fn append_read_roundtrip_clean_after_fsync() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1");
    let mut log = SessionLog::open(path.clone()).unwrap();
    assert!(log.is_empty());
    let batch = [
        sample_log()[0].clone(),
        sample_log()[1].clone(),
        meta(1, Some("u1")),
    ];
    log.append(&batch).unwrap();

    let reopened = SessionLog::open(path).unwrap();
    assert_eq!(reopened.entries(), &batch);
    assert_eq!(reopened.revision(), Some(1));
    assert!(!reopened.corrupt_tail_skipped());
    assert!(reopened.fingerprint_matched());
}

// 断言：单批追加超过 200 条时第 200 条后滚动到新段，segmeta 只登记已封口的第一段，
// 且跨段读取顺序无损、指纹仍校验通过。
#[test]
fn two_hundred_entries_roll_new_segment() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1");
    let mut log = SessionLog::open(path.clone()).unwrap();
    let mut batch = sample_log();
    for i in 0..(SEGMENT_MAX_ENTRIES + 1) {
        batch.push(user_message(&format!("m{i}"), Some("u1"), "x"));
    }
    batch.push(meta(1, Some("u1")));
    log.append(&batch).unwrap();

    let files: Vec<String> = fs::read_dir(&path)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(files.contains(&"0001.jsonl".to_string()), "{files:?}");
    assert!(files.contains(&"0002.jsonl".to_string()), "{files:?}");

    let segmeta: Segmeta =
        serde_json::from_str(&fs::read_to_string(path.join(super::SEGMETA_FILE)).unwrap()).unwrap();
    assert_eq!(segmeta.segments.len(), 1, "只有第一段封口");
    assert_eq!(segmeta.segments[0].file, "0001.jsonl");
    assert_eq!(segmeta.segments[0].count, SEGMENT_MAX_ENTRIES);

    let reopened = SessionLog::open(path).unwrap();
    assert_eq!(reopened.entries().len(), batch.len());
    assert_eq!(reopened.entries(), &batch, "跨段顺序无损");
    assert!(reopened.fingerprint_matched());
}

// 断言：单行超大内容累计超过 512KB 字节阈值时同样触发封口并滚动新段。
#[test]
fn large_batch_seal_byte_threshold_rolls() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1");
    let mut log = SessionLog::open(path.clone()).unwrap();
    let big = "字".repeat(64_000); // 每行 ~192KB（UTF-8 每字 3 字节），四行 > 512KB
    let batch = vec![
        user_message("u1", None, &big),
        user_message("u2", None, &big),
        user_message("u3", None, &big),
        user_message("u4", None, &big),
    ];
    log.append(&batch).unwrap();
    let files: Vec<String> = fs::read_dir(&path)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        files.contains(&"0002.jsonl".to_string()),
        "512KB 封口: {files:?}"
    );
}

// ---- 损坏尾段容错 ----

// 断言：崩溃留下的无换行半行被跳过且不入指纹链；之后追加绝不续写含半行的
// 既有段（0001 字节不变），而是滚动到新段 0002，重开后条目完整。
#[test]
fn crashed_half_line_skipped_new_segment() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1");
    let mut log = SessionLog::open(path.clone()).unwrap();
    log.append(&sample_log()).unwrap();

    let file = path.join("0001.jsonl");
    let mut broken = fs::read(&file).unwrap();
    broken.extend_from_slice(br#"{"kind":"message","id":"half"#); // 无换行的半行
    fs::write(&file, &broken).unwrap();

    let mut reopened = SessionLog::open(path.clone()).unwrap();
    assert!(reopened.corrupt_tail_skipped());
    assert_eq!(reopened.entries().len(), 2, "半行不计入条目");
    assert_eq!(reopened.fingerprint(), log.fingerprint(), "半行不入指纹链");

    let first_bytes = fs::read(&file).unwrap();
    reopened
        .append(&[user_message("u2", Some("u1"), "新分支")])
        .unwrap();
    assert_eq!(
        fs::read(&file).unwrap(),
        first_bytes,
        "绝不续写含损坏尾行的既有段（§7.2）"
    );
    assert!(path.join("0002.jsonl").exists(), "应滚动到新段");

    let final_log = SessionLog::open(path).unwrap();
    assert_eq!(final_log.entries().len(), 3);
}

// 断言：最后一段内首个非法 JSON 行起的内容全部跳过（损坏尾段容错）。
#[test]
fn tail_invalid_line_skips_rest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1");
    let mut log = SessionLog::open(path.clone()).unwrap();
    log.append(&sample_log()).unwrap();
    let file = path.join("0001.jsonl");
    let mut broken = fs::read(&file).unwrap();
    broken.extend_from_slice(b"not-json\n{\"kind\":\"message\",\"id\":\"lost\"}\n");
    fs::write(&file, &broken).unwrap();

    let reopened = SessionLog::open(path).unwrap();
    assert!(reopened.corrupt_tail_skipped());
    assert_eq!(reopened.entries().len(), 2);
}

// 断言：非最后一段（已封口段）中部损坏时打开必须直接报错，不做任何容错。
#[test]
fn non_tail_segment_corrupt_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1");
    let mut log = SessionLog::open(path.clone()).unwrap();
    let mut batch = sample_log();
    for i in 0..SEGMENT_MAX_ENTRIES {
        batch.push(user_message(&format!("m{i}"), Some("u1"), "x"));
    }
    log.append(&batch).unwrap(); // 0001 满 202 条封口

    let file = path.join("0001.jsonl");
    let content = fs::read_to_string(&file).unwrap();
    let tampered = content.replacen("\"你好\"", "\" 破损", 1);
    fs::write(&file, tampered).unwrap();

    let err = SessionLog::open(path).unwrap_err();
    assert!(err.to_string().contains("损坏"), "{err}");
}

// ---- 指纹 ----

// 断言：篡改已封口段的内容（保持 JSON 合法）后指纹失配；失配时追加只写新段
// 0002，绝不续写被篡改的既有段。
#[test]
fn fingerprint_mismatch_tampered_segment_no_append() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1");
    let mut log = SessionLog::open(path.clone()).unwrap();
    let mut batch = sample_log();
    for i in 0..SEGMENT_MAX_ENTRIES {
        batch.push(user_message(&format!("m{i}"), Some("u1"), "x"));
    }
    log.append(&batch).unwrap(); // 封口并写 segmeta

    let file = path.join("0001.jsonl");
    let content = fs::read_to_string(&file).unwrap();
    // 保持 JSON 合法但改动内容（模拟外部篡改未被长度校验发现）
    let tampered = content.replacen(r#""text":"你好""#, r#""text":"被改""#, 1);
    assert_ne!(content, tampered);
    fs::write(&file, tampered).unwrap();

    let reopened = SessionLog::open(path.clone()).unwrap();
    assert!(!reopened.fingerprint_matched(), "篡改后指纹必须失配");

    let mut reopened = reopened;
    reopened
        .append(&[user_message("after", Some("u1"), "y")])
        .unwrap();
    assert!(
        path.join("0002.jsonl").exists(),
        "指纹失配只新增新段，绝不续写"
    );
}

// 断言：从未封口（无 segmeta）的日志视为可信，开放段可直接续写。
#[test]
fn no_segmeta_passes_open_segment_appendable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1");
    let mut log = SessionLog::open(path.clone()).unwrap();
    log.append(&sample_log()).unwrap();
    assert!(
        !path.join(super::SEGMETA_FILE).exists(),
        "未封口不应有 segmeta"
    );

    let mut reopened = SessionLog::open(path).unwrap();
    assert!(reopened.fingerprint_matched());
    reopened.append(&[meta(2, Some("u1"))]).unwrap();
    assert_eq!(reopened.revision(), Some(2), "未封口段直接续写");
}
