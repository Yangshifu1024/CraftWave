//! mcp/commands.rs 的 headless 测试：TOFU 三路径（allow_once/
//! remember/deny）与信任决策连带撤销工具许可。

use std::sync::Arc;

use super::commands::{
    mcp_connect_core, mcp_disconnect_core, mcp_set_launch_trust_core, resolve_trust_ask,
};
use super::config::{McpConfigFile, McpScope};
use super::pool::{ConnState, McpHost};
use super::trust::LaunchDecision;
use crate::backend::ask::AskAnswer;
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

/// 后台应答者：等 mcpTrust 询问出现后投递指定选项。
async fn answer_mcp_trust(app: Arc<AppState>, events: Arc<RecordingEventSink>, option_id: &str) {
    for _ in 0..300 {
        let opened = events
            .payloads_of(keys::ASK_OPENED)
            .into_iter()
            .find(|p| p["payload"]["kind"] == "mcpTrust");
        if let Some(payload) = opened {
            let ask_id = payload["askId"].as_str().expect("askId").to_owned();
            resolve_trust_ask(
                &app,
                &ask_id,
                AskAnswer::Approval {
                    option_id: option_id.to_owned(),
                    edited_command: None,
                },
            )
            .await;
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("TOFU 询问未出现");
}

// 断言（TOFU 指纹信任全链）：mcp_connect 首连弹出 mcpTrust 询问、载荷
// 字段逐字（server/transport/16 位指纹/commandOrUrl）——deny →
// E_CANCELLED 不连接且不产生信任记录；allow_once → 连接 Ready、信任表
// 无记录（下次再问）；remember → trust/mcp.json 落盘，同指纹免问重连；
// mcp_set_launch_trust 的 revoke 连带撤销该服务器全部工具许可
// （服务器身份不可信 → 许可不延续）。
#[tokio::test(flavor = "current_thread")]
async fn tofu_three_paths_and_revoke_cascade() {
    // 进程级注册表单例的互斥面（与 pool_test 共享同一把锁）
    let _guard = crate::mcp::fixture::test_lock_guard().await;
    // —— deny 路径 ——
    let (app, dir, events) = headless_app();
    write_global_config(&dir, "tofu-deny");
    let fixture = EchoFixture::new();
    fixture.install("tofu-deny");
    let answer_app = app.clone();
    let answer_events = events.clone();
    let answer_task =
        tokio::spawn(async move { answer_mcp_trust(answer_app, answer_events, "deny").await });
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
    // 载荷字段逐字（mcpTrust：server/transport/fingerprint/commandOrUrl）
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
    let answer_app = app.clone();
    let answer_events = events.clone();
    let answer_task =
        tokio::spawn(
            async move { answer_mcp_trust(answer_app, answer_events, "allow_once").await },
        );
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
    let answer_app = app.clone();
    let answer_events = events.clone();
    let answer_task =
        tokio::spawn(async move { answer_mcp_trust(answer_app, answer_events, "remember").await });
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
        "remember 写 trust/mcp.json（trust 域）"
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

    // —— set_launch_trust：revoke 连带撤销该服务器全部工具许可——
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
