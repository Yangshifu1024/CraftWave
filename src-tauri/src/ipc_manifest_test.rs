//! §12.3 契约测试（Rust 侧产出）：
//!
//! 1. 遍历注册命令（`ipc_commands!` 单一事实源）落盘 `target/ipc-manifest.json`；
//! 2. 落盘跨语言 roundtrip fixture `target/session-tree-fixture.jsonl`
//!    （serde 序列化字节即 SegmentLog 落盘行格式），供前端
//!    `ui/src/ipc/contract.test.ts` 做 TS 镜像比对与树还原校验；
//! 3. 导出契约负载类型的 TS 声明到 `ui/src/ipc/generated/`（02 号票，C-2
//!    定案：ts-rs 12 生成物进 git，cargo build 不依赖生成步骤，导出由本测试
//!    统一驱动——宏清单按类型统一调用，避免每类型散落导出测试）。
//!
//! 前端消费产物前必须先跑 `cargo test`（§12.3 CI 顺序）。

use std::fs;
use std::path::PathBuf;

use crate::storage::entries::{
    CompleteReason, MessageEntry, MessageRole, MessageStatus, MetaEntry, SessionEntry, StoredEntry,
    WirePart,
};
use crate::storage::segment::entry_line;

/// 测试产物输出目录：crate 的 target/（与前端的比对脚本约定一致）。
fn target_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target")
}

/// 契约 TS 生成物目录（进 git；types.ts 逐文件 re-export，前端单一来源）。
fn generated_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/src/ipc/generated")
}

/// 递归列出目录下全部 .ts 文件（相对 generated_dir 的斜杠路径，含 serde_json/ 子目录）。
fn list_generated_files(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("读生成目录失败：{e}")) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            list_generated_files(&path, base, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("ts") {
            out.push(
                path.strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

/// 02 号票：契约负载类型的 ts-rs 导出 + manifest ↔ 生成物正向一致 + 冒烟。
/// 正向（manifest 名义类型 ⊆ 生成文件）在本测试钉住；反向（生成文件 ⊆
/// types.ts 再导出面，防类型删除后遗留孤儿文件）由前端 contract.test 扫描
/// ——cargo test → vitest 的 CI 顺序保证本测试先更新生成物、前端再校验。
#[test]
fn export_contract_types_and_match_manifest() {
    let out_dir = generated_dir();
    let cfg = ts_rs::Config::default()
        .with_large_int("number") // i64/u64 等按镜像口径渲染 number（时间戳/计数均安全整数内）
        .with_out_dir(out_dir.clone());

    // ① 宏清单驱动导出：命令名义负载连带依赖闭包写出；非名义类型
    //    （bool/String/Vec<_>/Option<_>/HashMap<_,_> 等）无独立生成文件，
    //    返回 CannotBeExported 占位；() 直接成功跳过。
    for (name, result) in crate::export_contract_ts(&cfg) {
        match result {
            Ok(()) => {}
            Err(ts_rs::ExportError::CannotBeExported(_)) => {}
            Err(e) => panic!("契约类型 {name} 的 TS 导出失败：{e}"),
        }
    }
    // 帧面/询问面/错误面/边车面补充根：不经命令清单（帧经 Channel 推送、
    // 询问载荷经事件推送、错误面与边车形状是命令负载的组成部分）；存储
    // 信封 StoredEntry（session/message/meta 全集）也不在命令负载闭包内
    // （命令走 MessageEntry/SessionTreeData），但它与子类型是 fixture 与
    // 前端树持久化层的直接消费面。WorkspacePathHit 只出现在非名义清单类型
    // Vec<WorkspacePathHit> 内，而 export_all 对非名义根类型不递归访问依赖，
    // 须显式补根。
    <crate::backend::frames::ChatFrame as ts_rs::TS>::export_all(&cfg).unwrap();
    <crate::backend::frames::SequencedFrame as ts_rs::TS>::export_all(&cfg).unwrap();
    // 运行快照（05 号票，§3.2/§3.4）：session_run_snapshot 返回 Option<RunSnapshot>
    // 与 runSnapshot 摘要帧负载都是非名义根（Option<_>/帧内 flatten），显式补根
    <crate::backend::frames::RunSnapshot as ts_rs::TS>::export_all(&cfg).unwrap();
    <crate::backend::ask::AskPayload as ts_rs::TS>::export_all(&cfg).unwrap();
    // 待审批/询问恢复查询（05 号票，§3.3 #59c）：返回 Option<SessionPendingAsk>
    // 非名义，显式补根导出类型声明
    <crate::host::commands::chat::SessionPendingAsk as ts_rs::TS>::export_all(&cfg).unwrap();
    <crate::storage::StoredEntry as ts_rs::TS>::export_all(&cfg).unwrap();
    <crate::host::commands::workspace::WorkspacePathHit as ts_rs::TS>::export_all(&cfg).unwrap();
    <crate::util::errors::IpcError as ts_rs::TS>::export_all(&cfg).unwrap();
    <crate::tools::ToolOutcome as ts_rs::TS>::export_all(&cfg).unwrap();
    // 内置预设类型（03 号票）：预设不在任何命令负载闭包内，显式补根导出
    // 类型声明（ProviderPreset.ts + 依赖 PresetModel.ts）。
    <crate::providers::presets::ProviderPreset as ts_rs::TS>::export_all(&cfg).unwrap();
    // 技能清单行（09 号票，§3.3 #48）：skill_list 返回 Vec<SkillListEntry>
    // 非名义根，显式补根导出（连带 SkillSource 枚举）。
    <crate::knowledge::commands::SkillListEntry as ts_rs::TS>::export_all(&cfg).unwrap();
    // 子代理契约（10 号票）：SubSpawnArgs 是 subagent 工具参数（非命令负载），
    // SubMeta 是子代理历史 meta（回看面），显式补根导出（连带 SubOutcome/
    // SubHistoryLine 等依赖）。
    <crate::host::commands::subagent::SubSpawnArgs as ts_rs::TS>::export_all(&cfg).unwrap();
    <crate::host::commands::subagent::SubMeta as ts_rs::TS>::export_all(&cfg).unwrap();
    // 配额快照行（21 号票，§3.3 #53）：quota_snapshots 返回 Vec<QuotaSnapshot>
    // 非名义根，显式补根导出（连带 QuotaStatus/QuotaData/QuotaEntry 依赖）。
    // 类型本体在 providers::quota（命令壳只是转调，契约类型不重复归属）。
    <crate::providers::quota::QuotaSnapshot as ts_rs::TS>::export_all(&cfg).unwrap();
    // 产物面板契约（28 号票，§3.3 #59l/#35）：list_editors 返回
    // Vec<EditorEntry> 非名义根，显式补根导出。
    <crate::host::commands::artifacts::EditorEntry as ts_rs::TS>::export_all(&cfg).unwrap();

    // ①b 内置预设清单常量（03 号票单源定稿）：serde 序列化权威清单写入
    //    BuiltinPresets.ts（前端 settings/presets.ts re-export 消费）——
    //    改 Rust 预设 → cargo test 重新生成 → 前端类型与内容即时感知。
    let presets_json = serde_json::to_string_pretty(crate::providers::presets::builtin_presets())
        .expect("内置预设序列化失败");
    let presets_ts = format!(
        "// 本文件由 cargo test 生成（src-tauri/src/ipc_manifest_test.rs），勿手改。\n\
         // 权威来源：src-tauri/src/providers/presets.rs 的 builtin_presets()（§3.3 预设单源注记）。\n\
         import type {{ ProviderPreset }} from \"./ProviderPreset\";\n\n\
         /** 内置供应商预设全集（顺序即设置页「从预设添加」的展示顺序）。 */\n\
         export const BUILTIN_PRESETS: ProviderPreset[] = {presets_json};\n"
    );
    fs::write(out_dir.join("BuiltinPresets.ts"), presets_ts).expect("写入 BuiltinPresets.ts 失败");

    // ② 冒烟 + 关键类型存在
    let mut files = Vec::new();
    list_generated_files(&out_dir, &out_dir, &mut files);
    files.sort();
    assert!(
        files.len() >= 40,
        "生成物目录异常偏少（{} 个文件）：{:?}",
        files.len(),
        files
    );
    for key in [
        "WirePart.ts",
        "MessageEntry.ts",
        "StoredEntry.ts",
        "SessionTreeData.ts",
        "SessionMeta.ts",
        "AppConfig.ts",
        "ProviderConfig.ts",
        "RunConfig.ts",
        "StartChatArgs.ts",
        "ChatFrame.ts",
        "SequencedFrame.ts",
        "AskPayload.ts",
        "AskAnswer.ts",
        "IpcError.ts",
        "ToolOutcome.ts",
        // 预设单源（03 号票）：类型声明 + 清单常量
        "ProviderPreset.ts",
        "PresetModel.ts",
        "BuiltinPresets.ts",
        // 技能契约（09 号票）
        "SkillListEntry.ts",
        "SkillSource.ts",
        "SkillToggleArgs.ts",
        "SkillInspectResult.ts",
        "serde_json/JsonValue.ts",
    ] {
        assert!(
            files.iter().any(|f| f == key),
            "生成物缺关键类型 {key}（现有：{files:?}）"
        );
    }

    // ③ 正向一致：manifest 每个名义 args_type/ret_type 必须有同名生成文件。
    //    非名义类型（()、bool、Vec<_>、Option<_>、HashMap<_,_>）无独立文件，
    //    天然跳过；清单里的 `Value` 是 serde_json::Value 的短名，其 TS 形状
    //    由 serde_json/JsonValue.ts 承载，同样豁免。
    for command in crate::ipc_manifest() {
        for (label, ty) in [
            ("&args", command.args_type.as_str()),
            ("&ret", command.ret_type.as_str()),
        ] {
            let trimmed = ty.trim();
            // 名义判定与前端 contract.test 同口径：字母/下划线开头、后随
            // 字母/数字/下划线的单标识符（排除 ()、bool、Vec<_>、Value 等）
            let nominal = !trimmed.is_empty()
                && trimmed != "bool"
                && trimmed != "Value"
                && trimmed.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && trimmed
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_');
            if nominal {
                let expected = format!("{trimmed}.ts");
                assert!(
                    files.contains(&expected),
                    "manifest 命令 {} 的 {label} 类型 {trimmed} 缺生成物 {expected}（现有：{files:?}）",
                    command.name
                );
            }
        }
    }
}

/// 契约导出主测试：先校验 manifest 的不变式（命令名唯一且 snake_case）与
/// 各命令的契约类型名锚点，落盘 ipc-manifest.json；再构造固定样例树做
/// 行级往返校验（落盘字节即契约），落盘 session-tree-fixture.jsonl。两个
/// 文件供前端 TS 侧镜像比对（§12.3）。
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
    // 会话偏好两命令（18 号票，§3.3 #24/#25）：#25 返回 () 非名义，载荷
    // 类型名只锚 args 面（SetSessionPrefsArgs → SetSessionPrefsPayload）
    assert_eq!(find("get_session_prefs").args_type, "SessionPrefsArgs");
    assert_eq!(find("get_session_prefs").ret_type, "GetSessionPrefsResult");
    assert_eq!(find("set_session_prefs").args_type, "SetSessionPrefsArgs");
    assert_eq!(find("set_session_prefs").ret_type, "()");
    // steer/followUp 队列三命令（06 号票，§3.3 #21/#59d/#59e）
    assert_eq!(find("inject_run_message").args_type, "InjectRunMessageArgs");
    assert_eq!(
        find("inject_run_message").ret_type,
        "InjectRunMessageResult"
    );
    assert_eq!(
        find("resume_followup_queue").args_type,
        "ResumeFollowupQueueArgs"
    );
    assert_eq!(
        find("resume_followup_queue").ret_type,
        "ResumeFollowupQueueResult"
    );
    assert_eq!(find("remove_followup").args_type, "RemoveFollowupArgs");
    assert_eq!(find("remove_followup").ret_type, "RemoveFollowupResult");
    // 通知引导两命令（19 号票）
    assert_eq!(
        find("notification_permission_status").ret_type,
        "NotificationPermissionStatus"
    );
    assert_eq!(find("open_notification_settings").ret_type, "()");
    // 数据目录入口（03 号票，§3.3 #3）
    assert_eq!(find("open_dir").args_type, "OpenDirArgs");
    assert_eq!(find("open_dir").ret_type, "()");
    // 手动压缩命令（04 号票，§3.3 #59b）
    assert_eq!(find("compact_session").args_type, "CompactSessionArgs");
    assert_eq!(find("compact_session").ret_type, "CompactSessionResult");
    // 帧重放与热重连三命令（05 号票，§3.3 #20/#57/#59c）
    assert_eq!(find("attach_run").args_type, "AttachRunArgs");
    assert_eq!(find("attach_run").ret_type, "AttachRunResult");
    assert_eq!(
        find("session_run_snapshot").args_type,
        "SessionRunSnapshotArgs"
    );
    assert_eq!(find("session_run_snapshot").ret_type, "Option<RunSnapshot>");
    assert_eq!(
        find("session_pending_ask").args_type,
        "SessionPendingAskArgs"
    );
    assert_eq!(
        find("session_pending_ask").ret_type,
        "Option<SessionPendingAsk>"
    );
    // MCP 八命令（08 号票，§3.3 #40–47）
    assert_eq!(find("mcp_list_config").args_type, "McpListConfigArgs");
    // 技能四命令（09 号票，§3.3 #48–51 逐字；skill_list 返回非名义 Vec，
    // ret_type 记 Vec<SkillListEntry>，元素类型经下方显式补根导出）
    assert_eq!(find("skill_list").ret_type, "Vec<SkillListEntry>");
    assert_eq!(find("skill_toggle").args_type, "SkillToggleArgs");
    assert_eq!(find("skill_toggle").ret_type, "()");
    assert_eq!(find("skill_reload").args_type, "()");
    assert_eq!(find("skill_reload").ret_type, "()");
    assert_eq!(find("skill_inspect").args_type, "SkillInspectArgs");
    assert_eq!(find("skill_inspect").ret_type, "SkillInspectResult");
    // 子代理两命令（10 号票，§3.3 #58/#59 逐字；#59 返回 Vec 非名义）
    assert_eq!(find("stop_subagent").args_type, "StopSubagentArgs");
    assert_eq!(find("stop_subagent").ret_type, "()");
    assert_eq!(
        find("load_subagent_history").args_type,
        "LoadSubagentHistoryArgs"
    );
    assert_eq!(find("load_subagent_history").ret_type, "Vec<MessageEntry>");
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
        "tools": crate::tools::STATIC_TOOL_NAMES,
        // 工具规格（name + paramsSchema）：前端 toolkit 声明同源校验（§10.3，14 号票）
        "toolSpecs": crate::tools::build_registry()
            .into_iter()
            .map(|s| serde_json::json!({
                "name": s.name,
                "paramsSchema": s.params_schema,
            }))
            .collect::<Vec<_>>(),
    });
    // 帧：P0 9 帧 + 【M2】压缩/摘要/子代理 4 帧 + 36 号票 toolAudit（16）
    assert_eq!(manifest["frames"].as_array().unwrap().len(), 16);
    // 事件键：14 号票 5 键 + 17 号票 5 键（run 终态三键 + notify/退出两键）
    // + 04 号票压缩三键（run:compacting/compacted/compact_failed）
    // + 06 号票三键（run:start / run:inject / run:queue）+ 08 号票 mcp:status
    // + 10 号票子代理六键（sub:spawn/step/report/usage/done/error）
    // + 27 号票 session:title（自动起名，§3.5 P0 键位收口）
    assert_eq!(manifest["events"].as_array().unwrap().len(), 24);
    // 08 号票：注册表分「静态段 + 动态段」两断言——静态段钉死（14 号票起
    // 17 个：+batch_read/glob/calculate/render_html，§8.3 M2 表小工具包
    // 四行）；动态段（MCP）运行时可变，manifest 落盘时为当前快照
    // （恒 ≥ 静态段）。
    assert_eq!(manifest["tools"].as_array().unwrap().len(), 17);
    assert_eq!(manifest["toolSpecs"].as_array().unwrap().len(), 17);
    assert_eq!(
        crate::tools::STATIC_TOOL_NAMES.len(),
        17,
        "静态段工具数漂移"
    );
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

/// 样例树：线性两轮 + 工具调用轮 + 历史兄弟分支（覆盖 status/runId/toolCall 拆分/reasoning）。
/// 与 `ui/src/ipc/contract.test.ts` 中 EXPECTED_ENTRIES 逐字段一致，改动必须双侧同步。
fn fixture_entries() -> Vec<StoredEntry> {
    // 造一条用户消息条目的简写闭包（固定 role=User、单文本 part）。
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
        // 带图 user 消息（17 号票，§3.2 image part）：JSONL 行只含 blobHash/
        // mimeType 引用，不含图片本体（体积不膨胀的验收面）；行序在分支
        // 链（a2/a3/marker）之后——父先子后的存储纪律保持
        StoredEntry::Message(MessageEntry {
            id: "u3".to_string(),
            parent_id: Some("a2".to_string()),
            role: MessageRole::User,
            parts: vec![
                WirePart::Text {
                    text: "看这张截图".to_string(),
                },
                WirePart::Image {
                    blob_hash: "a".repeat(32),
                    mime_type: "image/png".to_string(),
                },
            ],
            created_at: 1_760_000_000_008,
            run_id: None,
            status: None,
        }),
        // compaction 条目（04 号票，§6.1）：前端树持久化层须跳过（不投影为
        // 树节点，§6.3），跨语言行格式在此钉住
        StoredEntry::Compaction(crate::storage::StoredCompactionEntry {
            id: "manual:compact:1".to_string(),
            cut_entry_id: "a3".to_string(),
            summary: "早期对话摘要".to_string(),
            tokens_before: 9000,
            created_at: 1_760_000_000_007,
        }),
        StoredEntry::Meta(MetaEntry {
            active_leaf_id: Some("run-2:marker".to_string()),
            title: None,
            revision: 4,
        }),
    ]
}
