//! mcp/commands.rs 的 headless 测试（08 号票）：8 命令的 core 面——双作用
//! 域配置读写（未知字段不丢）、TOFU 三路径（allow_once/remember/deny）、
//! mcp_test 草稿试连、mcp_snapshot、信任决策连带撤销。

use std::sync::Arc;

use super::commands::{
    self, McpListConfigArgs, McpServerRefArgs, McpSetLaunchTrustArgs, McpSnapshotArgs, McpTestArgs,
    mcp_connect_core, mcp_disconnect_core, mcp_list_config_core, mcp_save_config_core,
    mcp_set_launch_trust_core, mcp_snapshot_core, mcp_test_core, resolve_trust_ask,
};
use super::config::{McpConfigFile, McpScope};
use super::pool::{ConnState, McpHost};
use super::trust::LaunchDecision;
use crate::backend::ask::{AskAnswer, AskPayload};
use crate::host::AppState;
use crate::mcp::fixture::EchoFixture;
use crate::util::events::{RecordingEventSink, keys};

/// headless AppState（临时数据目录 + 录音事件汇）。
fn headless_app() -> (Arc<AppState>, tempfile::TempDir, Arc<RecordingEventSink>) {
    let dir = tempfile::tempdir().expect("临时目录");
    let events = Arc::new(RecordingEventSink::new());
    let mut state = AppState::new_in(dir.path().to_path_buf(), events.clone());
    // headless 下 MCP 宿主用同目录（AppState::new_in 已按 data_dir 构造）
    state.mcp = Arc::new(McpHost::new(dir.path().to_path_buf(), events.clone()));
    let app = Arc::new(state);
    (app, dir, events)
}

/// 写全局 mcp.json（stdio fixture 配置）。
fn write_global_config(dir: &tempfile::TempDir, config_name: &str) {
    let file: McpConfigFile = serde_json::from_value(serde_json::json!({
        "mcpServers": {
            config_name: {
                "command": config_name,
                "futureField": "未知字段保存不丢"
            }
        },
        "topNote": "顶层未知字段"
    }))
    .expect("配置构造");
    let store = crate::mcp::config::McpConfigStore::new(dir.path(), None);
    store.save(McpScope::Global, &file).expect("写入");
}

/// mcp_list_config / mcp_save_config round-trip：未知字段保存前后不丢
/// （FR-08-02 + 票面"保存前后未知字段不丢"）。
#[test]
fn list_and_save_config_preserve_unknown_fields() {
    let (app, dir, _events) = headless_app();
    write_global_config(&dir, "fetch");
    let listed = mcp_list_config_core(&app).expect("读取");
    assert_eq!(
        listed.global.extra.get("topNote"),
        Some(&serde_json::json!("顶层未知字段"))
    );
    let server_value = listed.global.mcp_servers.get("fetch").expect("fetch 在场");
    assert_eq!(
        server_value.get("futureField"),
        Some(&serde_json::json!("未知字段保存不丢"))
    );
    // 项目作用域：无项目上下文 → 空配置 + projectAvailable=false
    assert!(!listed.project_available);
    assert!(listed.project.mcp_servers.is_empty());
    // 整份保存（改一个字段、保留未知字段）→ 再读不丢
    let mut draft = listed.global.clone();
    if let Some(server) = draft.mcp_servers.get_mut("fetch")
        && let Some(obj) = server.as_object_mut()
    {
        obj.insert("timeout".to_owned(), serde_json::json!(30));
    }
    mcp_save_config_core(&app, McpScope::Global, &draft).expect("保存");
    let reloaded = mcp_list_config_core(&app).expect("再读");
    let server_value = reloaded.global.mcp_servers.get("fetch").expect("在场");
    assert_eq!(server_value.get("timeout"), Some(&serde_json::json!(30)));
    assert_eq!(
        server_value.get("futureField"),
        Some(&serde_json::json!("未知字段保存不丢")),
        "未知字段保存不丢"
    );
    assert_eq!(
        reloaded.global.extra.get("topNote"),
        Some(&serde_json::json!("顶层未知字段"))
    );
}

/// mcp_connect 的 TOFU 三路径：
/// - deny → E_CANCELLED 不连接；
/// - allow_once → 连接 Ready、信任表无记录（下次再问）；
/// - remember → trust/mcp.json 落盘（同指纹免问）。
#[tokio::test(flavor = "current_thread")]
async fn tofu_three_paths() {
    // 进程级注册表单例的互斥面（与 pool_test 共享同一把锁）
    let _guard = crate::mcp::fixture::test_lock_guard().await;
    // —— deny 路径 ——
    let (app, dir, events) = headless_app();
    write_global_config(&dir, "tofu-deny");
    let fixture = EchoFixture::new();
    fixture.install("tofu-deny");
    // 后台应答者：等 ask:opened 后投递 deny
    let app2 = app.clone();
    let events2 = events.clone();
    let answer_task = tokio::spawn(async move {
        for _ in 0..200 {
            let opened = events2
                .payloads_of(keys::ASK_OPENED)
                .into_iter()
                .find(|p| p["payload"]["kind"] == "mcpTrust");
            if let Some(payload) = opened {
                let ask_id = payload["askId"].as_str().expect("askId").to_owned();
                resolve_trust_ask(
                    &app2,
                    &ask_id,
                    AskAnswer::Approval {
                        option_id: "deny".to_owned(),
                        edited_command: None,
                    },
                )
                .await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("TOFU 询问未出现");
    });
    let err = mcp_connect_core(&app, McpScope::Global, "tofu-deny")
        .await
        .expect_err("deny → 错误");
    assert_eq!(err.code, crate::backend::run_loop::E_CANCELLED);
    answer_task.await.expect("应答任务");
    assert!(
        events
            .payloads_of(keys::ASK_CLOSED)
            .iter()
            .any(|p| p["answered"] == true),
        "ask:closed 在案"
    );
    // 载荷字段逐字（§3.2 mcpTrust：server/transport/fingerprint/commandOrUrl）
    let opened = events
        .payloads_of(keys::ASK_OPENED)
        .into_iter()
        .find(|p| p["payload"]["kind"] == "mcpTrust")
        .expect("mcpTrust 载荷");
    assert_eq!(opened["payload"]["server"], "tofu-deny");
    assert_eq!(opened["payload"]["transport"], "stdio");
    assert!(
        opened["payload"]["fingerprint"]
            .as_str()
            .is_some_and(|f| f.len() == 16),
        "指纹 16 位展示面"
    );
    assert_eq!(opened["payload"]["commandOrUrl"], "tofu-deny");
    assert!(
        !app.mcp
            .trust
            .entries()
            .iter()
            .any(|e| e.decision == "remember"),
        "deny 不产生信任记录"
    );

    // —— allow_once 路径：连接 Ready、下次连接再问（无 remember 记录）——
    let (app, dir, events) = headless_app();
    write_global_config(&dir, "tofu-once");
    let fixture = EchoFixture::new();
    fixture.install("tofu-once");
    let app2 = app.clone();
    let events2 = events.clone();
    let answer_task = tokio::spawn(async move {
        for _ in 0..300 {
            let opened = events2
                .payloads_of(keys::ASK_OPENED)
                .into_iter()
                .find(|p| p["payload"]["kind"] == "mcpTrust");
            if let Some(payload) = opened {
                let ask_id = payload["askId"].as_str().expect("askId").to_owned();
                resolve_trust_ask(
                    &app2,
                    &ask_id,
                    AskAnswer::Approval {
                        option_id: "allow_once".to_owned(),
                        edited_command: None,
                    },
                )
                .await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("TOFU 询问未出现");
    });
    mcp_connect_core(&app, McpScope::Global, "tofu-once")
        .await
        .expect("allow_once 后连接发起");
    answer_task.await.expect("应答任务");
    for _ in 0..200 {
        if let Some((ConnState::Ready, _)) = app.mcp.state_of(&crate::mcp::tools::ConnKey {
            scope: McpScope::Global,
            project_id: None,
            name: "tofu-once".into(),
            canonical_path: String::new(),
        }) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        !app.mcp
            .trust
            .entries()
            .iter()
            .any(|e| e.decision == "remember"),
        "allow_once 不写信任记录（下次再问）"
    );
    mcp_disconnect_core(&app, McpScope::Global, "tofu-once").expect("断开");

    // —— remember 路径：信任落盘，再次连接免问（无新 ask:opened）——
    let (app, dir, events) = headless_app();
    write_global_config(&dir, "tofu-remember");
    let fixture = EchoFixture::new();
    fixture.install("tofu-remember");
    let app2 = app.clone();
    let events2 = events.clone();
    let answer_task = tokio::spawn(async move {
        for _ in 0..300 {
            let opened = events2
                .payloads_of(keys::ASK_OPENED)
                .into_iter()
                .find(|p| p["payload"]["kind"] == "mcpTrust");
            if let Some(payload) = opened {
                let ask_id = payload["askId"].as_str().expect("askId").to_owned();
                resolve_trust_ask(
                    &app2,
                    &ask_id,
                    AskAnswer::Approval {
                        option_id: "remember".to_owned(),
                        edited_command: None,
                    },
                )
                .await;
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("TOFU 询问未出现");
    });
    mcp_connect_core(&app, McpScope::Global, "tofu-remember")
        .await
        .expect("remember 后连接发起");
    answer_task.await.expect("应答任务");
    assert!(
        app.mcp
            .trust
            .entries()
            .iter()
            .any(|e| e.decision == "remember"),
        "remember 写 trust/mcp.json（§7.7 trust 域）"
    );
    // 第二次连接：无询问（信任命中），直接到 Ready
    mcp_disconnect_core(&app, McpScope::Global, "tofu-remember").expect("断开");
    let before = events.payloads_of(keys::ASK_OPENED).len();
    mcp_connect_core(&app, McpScope::Global, "tofu-remember")
        .await
        .expect("免问重连");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        events.payloads_of(keys::ASK_OPENED).len(),
        before,
        "记住后免问"
    );
    mcp_disconnect_core(&app, McpScope::Global, "tofu-remember").expect("断开");
}

/// mcp_test 草稿试连：本地 fixture 命中 → ok + 工具清单；假命令 → ok=false
/// + error。总预算 60s（正常路径秒级）。
#[tokio::test(flavor = "current_thread")]
async fn mcp_test_draft_probe() {
    let _guard = crate::mcp::fixture::test_lock_guard().await;
    let fixture = EchoFixture::new();
    fixture.install("probe-ok");
    let result = mcp_test_core(McpTestArgs {
        draft: serde_json::from_value(serde_json::json!({ "command": "probe-ok" })).expect("配置"),
    })
    .await;
    assert!(result.ok, "试连成功：{:?}", result.error);
    let names: Vec<&str> = result.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        names.contains(&"echo") && names.contains(&"add"),
        "工具清单：{names:?}"
    );
    // 不进池（试连不占连接；headless 应用无配置 → 快照无行）
    let (app, _dir, _events) = headless_app();
    assert!(
        mcp_snapshot_core(&app).expect("快照").servers.is_empty(),
        "试连不进池"
    );

    // 假命令 → 失败 + error 文本
    let result = mcp_test_core(McpTestArgs {
        draft: serde_json::from_value(serde_json::json!({ "command": "no-such-craftwave-binary" }))
            .expect("配置"),
    })
    .await;
    assert!(!result.ok);
    assert!(result.error.is_some());
}

/// mcp_set_launch_trust：remember → 信任态 Trusted；revoke → 连带撤销该
/// 服务器全部工具许可（Q44：服务器身份不可信 → 许可不延续）。
#[test]
fn set_launch_trust_and_revoke_cascades() {
    let (app, _dir, _events) = headless_app();
    app.mcp
        .licenses
        .grant("fp-x", "tool_a", "h1", 1)
        .expect("先授一笔工具许可");
    mcp_set_launch_trust_core(&app, "fp-x", LaunchDecision::Remember).expect("记住");
    assert!(app.mcp.trust.is_trusted("fp-x"));
    mcp_set_launch_trust_core(&app, "fp-x", LaunchDecision::Revoke).expect("撤销");
    assert!(!app.mcp.trust.is_trusted("fp-x"));
    assert!(
        !app.mcp.licenses.is_licensed("fp-x", "tool_a", "h1"),
        "撤销信任连带撤销工具许可"
    );
}

/// mcp_snapshot：池状态行（name/state/toolCount + scope/fingerprint/
/// trustStatus 补充面）。连接键与指纹按生产口径（真实 canonical 路径 +
/// 配置文件字节）计算，保证与 mcp_snapshot_core 的 resolve 结果同 key。
#[tokio::test(flavor = "current_thread")]
async fn snapshot_reports_pool() {
    let _guard = crate::mcp::fixture::test_lock_guard().await;
    let (app, dir, _events) = headless_app();
    write_global_config(&dir, "snap-srv");
    let fixture = EchoFixture::new();
    fixture.install("snap-srv");
    // 生产口径的 canonical 路径与文件字节（连接/快照/信任三面共用同一 key）
    let config_path = dir.path().join("mcp.json");
    let canonical = std::fs::canonicalize(&config_path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| config_path.to_string_lossy().into_owned());
    let config_bytes = std::fs::read(&config_path).unwrap_or_default();
    // 预置信任直连（TOFU 链路已被 tofu_three_paths 覆盖）
    let fingerprint = super::trust::launch_fingerprint(&super::trust::TrustSubject {
        scope: McpScope::Global,
        project_id: None,
        server_name: "snap-srv".into(),
        canonical_config_path: canonical.clone(),
        config_bytes,
    });
    app.mcp
        .trust
        .decide(&fingerprint, LaunchDecision::Remember, 1)
        .expect("预置信任");
    let config: crate::mcp::config::McpServerConfig =
        serde_json::from_value(serde_json::json!({ "command": "snap-srv" })).expect("配置");
    let key = crate::mcp::tools::ConnKey {
        scope: McpScope::Global,
        project_id: None,
        name: "snap-srv".into(),
        canonical_path: canonical.clone(),
    };
    app.mcp.connect(key.clone(), config, fingerprint);
    for _ in 0..200 {
        let snap = mcp_snapshot_core(&app).expect("快照");
        if snap
            .servers
            .iter()
            .any(|s| s.name == "snap-srv" && s.state == "ready" && s.tool_count >= 2)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let snap = mcp_snapshot_core(&app).expect("快照");
    let row = snap
        .servers
        .iter()
        .find(|s| s.name == "snap-srv")
        .expect("行在场");
    assert_eq!(row.state, "ready");
    assert!(row.tool_count >= 2, "工具数：{}", row.tool_count);
    assert_eq!(row.scope, McpScope::Global);
    // 信任状态列（补充面）：快照按同一生产口径计算指纹——预置的"记住"
    // 记录命中 → 已记住。
    assert_eq!(row.trust_status, "trusted");
    assert_eq!(row.fingerprint.len(), 16, "展示面指纹取前 16 位");
    app.mcp.disconnect(&key);
}

/// 契约参数类型的形状钉住（空参/枚举 serde 面；与 TS 生成物同源）。
#[test]
fn contract_shapes() {
    let args: McpServerRefArgs =
        serde_json::from_value(serde_json::json!({ "scope": "global", "name": "x" }))
            .expect("解析");
    assert_eq!(args.scope, McpScope::Global);
    let trust: McpSetLaunchTrustArgs = serde_json::from_value(serde_json::json!({
        "fingerprint": "abc", "decision": "remember"
    }))
    .expect("解析");
    assert_eq!(trust.decision, LaunchDecision::Remember);
    let _ = McpListConfigArgs {};
    let _ = McpSnapshotArgs {};
    // AskPayload mcpTrust 的 serde 判别面（AskPanel 路由）
    let payload: AskPayload = serde_json::from_value(serde_json::json!({
        "kind": "mcpTrust",
        "server": "s",
        "transport": "http",
        "fingerprint": "0123456789abcdef",
        "commandOrUrl": "http://127.0.0.1:9/mcp",
        "options": []
    }))
    .expect("mcpTrust 载荷解析");
    match payload {
        AskPayload::McpTrust { transport, .. } => {
            assert_eq!(transport, crate::backend::ask::McpTransportKind::Http);
        }
        other => panic!("应为 mcpTrust：{other:?}"),
    }
    let _ = commands::TrustAskMap::new();
}
