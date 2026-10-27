//! §12.3 契约测试（Rust 侧产出）：
//!
//! 1. 遍历注册命令（`ipc_commands!` 单一事实源）落盘 `target/ipc-manifest.json`；
//! 2. 落盘跨语言 roundtrip fixture `target/session-tree-fixture.jsonl`
//!    （serde 序列化字节即 SegmentLog 落盘行格式），供前端
//!    `ui/src/ipc/contract.test.ts` 做 TS 镜像比对与树还原校验。
//!
//! 前端消费两文件前必须先跑 `cargo test`（§12.3 CI 顺序）。

use std::fs;
use std::path::PathBuf;

use crate::storage::entries::{
    CompleteReason, MessageEntry, MessageRole, MessageStatus, MetaEntry, SessionEntry, StoredEntry,
    WirePart,
};
use crate::storage::segment::entry_line;

fn target_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target")
}

#[test]
fn export_ipc_manifest_and_cross_lang_fixture() {
    // ---- 1. manifest：命令清单 + 契约类型名（§12.3 schema） ----
    let commands = crate::ipc_manifest();
    let find = |name: &str| {
        commands
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("manifest 缺命令 {name}"))
    };

    // 键面不变式：invoke 名唯一且 snake_case（§3.1/D14）
    let mut names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
    let before = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(before, names.len(), "命令名必须唯一");
    for name in &names {
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
            "命令名必须 snake_case：{name}"
        );
    }

    // 存储两命令的契约类型名锚点（10 号票）
    assert_eq!(find("save_session_tree").args_type, "SaveSessionTreeArgs");
    assert_eq!(find("save_session_tree").ret_type, "SaveSessionTreeResult");
    assert_eq!(find("load_session_tree").args_type, "LoadSessionTreeArgs");
    assert_eq!(find("load_session_tree").ret_type, "SessionTreeData");
    // provider/config 四命令的契约类型名锚点（12 号票，收编同一宏）
    assert_eq!(find("get_config").ret_type, "AppConfig");
    assert_eq!(find("save_config").args_type, "SaveConfigArgs");
    assert_eq!(find("save_config").ret_type, "AppConfig");
    assert_eq!(find("save_provider").args_type, "SaveProviderArgs");
    assert_eq!(find("save_provider").ret_type, "ProviderConfig");
    assert_eq!(find("delete_provider").args_type, "DeleteProviderArgs");
    // 核心链路与 ask/计划门命令锚点（13/14 号票）
    assert_eq!(find("start_chat").args_type, "StartChatArgs");
    assert_eq!(find("start_chat").ret_type, "StartChatResult");
    assert_eq!(find("cancel_run").args_type, "CancelRunArgs");
    assert_eq!(find("resolve_ask").args_type, "ResolveAskArgs");
    assert_eq!(find("approve_plan").args_type, "ApprovePlanArgs");
    // 标题栏命令（骨架既有）仍在注册表
    assert!(
        commands
            .iter()
            .any(|c| c.name == "activate_custom_titlebar")
    );

    let manifest = serde_json::json!({
        "commands": commands,
        // ChatFrame P0 全集 9 帧（§3.4）
        "frames": crate::host::commands::FRAMES,
        // 事件键（§3.5）：14 号票实装 5 键
        "events": crate::host::commands::EVENTS,
        // 镜像工具清单（§12.3）：工具注册表单一事实源
        "tools": crate::tools::TOOL_NAMES,
        // 工具规格（name + paramsSchema）：前端 toolkit 声明同源校验（§10.3，14 号票）
        "toolSpecs": crate::tools::build_registry()
            .into_iter()
            .map(|s| serde_json::json!({
                "name": s.name,
                "paramsSchema": s.params_schema,
            }))
            .collect::<Vec<_>>(),
    });
    assert_eq!(manifest["frames"].as_array().unwrap().len(), 9);
    assert_eq!(manifest["events"].as_array().unwrap().len(), 5);
    assert_eq!(manifest["tools"].as_array().unwrap().len(), 8);
    assert_eq!(manifest["toolSpecs"].as_array().unwrap().len(), 8);
    fs::create_dir_all(target_dir()).unwrap();
    fs::write(
        target_dir().join("ipc-manifest.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    // ---- 2. fixture：固定样例树（与 ui/src/ipc/contract.test.ts 的 EXPECTED_ENTRIES 一致） ----
    let entries = fixture_entries();
    let mut text = String::new();
    for entry in &entries {
        let line = entry_line(entry).unwrap();
        // 行必须可解析回等值条目（落盘字节即契约）
        assert_eq!(
            serde_json::from_str::<StoredEntry>(&line).unwrap(),
            *entry,
            "fixture 行往返失配: {line}"
        );
        text.push_str(&line);
        text.push('\n');
    }
    fs::write(target_dir().join("session-tree-fixture.jsonl"), text).unwrap();
}

/// 样例树：线性两轮 + 工具调用轮 + 重生成兄弟分支（覆盖 status/runId/toolCall 拆分/reasoning）。
/// 与 `ui/src/ipc/contract.test.ts` 中 EXPECTED_ENTRIES 逐字段一致，改动必须双侧同步。
fn fixture_entries() -> Vec<StoredEntry> {
    let user = |id: &str, parent: Option<&str>, text: &str, at: i64| {
        StoredEntry::Message(MessageEntry {
            id: id.to_string(),
            parent_id: parent.map(str::to_string),
            role: MessageRole::User,
            parts: vec![WirePart::Text {
                text: text.to_string(),
            }],
            created_at: at,
            run_id: None,
            status: None,
        })
    };
    vec![
        StoredEntry::Session(SessionEntry {
            v: 1,
            id: "sess-fixture".to_string(),
            created_at: 1_760_000_000_000,
        }),
        user("u1", None, "你好", 1_760_000_000_001),
        StoredEntry::Message(MessageEntry {
            id: "a1".to_string(),
            parent_id: Some("u1".to_string()),
            role: MessageRole::Assistant,
            parts: vec![
                WirePart::Reasoning {
                    text: "思考中".to_string(),
                },
                WirePart::Text {
                    text: "你好！我是 CraftWave。".to_string(),
                },
            ],
            created_at: 1_760_000_000_002,
            run_id: None,
            status: Some(MessageStatus::Complete {
                reason: CompleteReason::Unknown,
            }),
        }),
        user("u2", Some("a1"), "读一下 README", 1_760_000_000_003),
        StoredEntry::Message(MessageEntry {
            id: "a2".to_string(),
            parent_id: Some("u2".to_string()),
            role: MessageRole::Assistant,
            parts: vec![
                WirePart::ToolCall {
                    call_id: "call_1".to_string(),
                    name: "read_workspace_file".to_string(),
                    args_json: r#"{"path":"README.md"}"#.to_string(),
                },
                WirePart::ToolResult {
                    call_id: "call_1".to_string(),
                    ok: true,
                    summary: "# CraftWave\n第一版说明。".to_string(),
                },
                WirePart::Text {
                    text: "README 读完了。".to_string(),
                },
            ],
            created_at: 1_760_000_000_004,
            run_id: Some("run-1".to_string()),
            status: Some(MessageStatus::Complete {
                reason: CompleteReason::Stop,
            }),
        }),
        StoredEntry::Message(MessageEntry {
            id: "a3".to_string(),
            parent_id: Some("u2".to_string()),
            role: MessageRole::Assistant,
            parts: vec![WirePart::Text {
                text: "重试：README 共 3 个章节。".to_string(),
            }],
            created_at: 1_760_000_000_005,
            run_id: Some("run-2".to_string()),
            status: Some(MessageStatus::Complete {
                reason: CompleteReason::Unknown,
            }),
        }),
        // runMarker 条目（15 号票 canonical 收口行）：role=system、链尾节点，
        // 前端以 metadata.custom.runMarker 保真往返（漂移即 E_SESSION_REV）
        StoredEntry::Message(MessageEntry {
            id: "run-2:marker".to_string(),
            parent_id: Some("a3".to_string()),
            role: MessageRole::System,
            parts: vec![WirePart::RunMarker {
                run_id: "run-2".to_string(),
                started_at: 1_760_000_000_005,
                ended_at: Some(1_760_000_000_006),
                duration_ms: Some(1200),
                tool_count: Some(0),
            }],
            created_at: 1_760_000_000_006,
            run_id: None,
            status: None,
        }),
        StoredEntry::Meta(MetaEntry {
            active_leaf_id: Some("run-2:marker".to_string()),
            title: None,
            revision: 4,
        }),
    ]
}
