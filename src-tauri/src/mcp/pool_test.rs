//! mcp/pool.rs 的测试（08 号票）：fixture 全链（connect→发现→调用→状态
//! 事件→断连重连→list_changed→Q14 结果未知）+ LRU 驱逐 + generation 失效。
//!
//! 全链测试经 [`crate::mcp::fixture`] 的进程内 echo server（§12.5 测试
//! 基建）：字节走内存管道，握手/清单同步/断线重连语义与生产一致。
//! 串行锁：McpToolRegistry 是进程级单例，fixture 测试互相串行（避免
//! 动态段互相污染）。

use std::sync::Arc;

use super::config::{McpScope, McpServerConfig};
use super::pool::{ConnState, McpHost};
use super::tools::ConnKey;
use super::trust::{LaunchDecision, TrustSubject, launch_fingerprint};
use crate::mcp::fixture::EchoFixture;
use crate::util::events::RecordingEventSink;
use crate::util::events::keys;

/// fixture 全链测试的串行锁（fixture 模块共享——与 commands_test 同一把，
/// 进程级注册表单例的互斥面；tokio guard 设计为跨 await 持有整个测试体）。
async fn chain_lock() -> tokio::sync::MutexGuard<'static, ()> {
    crate::mcp::fixture::test_lock_guard().await
}

/// 全链测试环境：临时数据目录 + host + fixture + 连接键。
struct ChainEnv {
    host: Arc<McpHost>,
    fixture: Arc<EchoFixture>,
    key: ConnKey,
    fingerprint: String,
    config: McpServerConfig,
    config_name: String,
    _dir: tempfile::TempDir,
    events: Arc<RecordingEventSink>,
}

/// 搭一个已信任、已连接（Ready）的全链环境。
async fn ready_env(config_name: &str) -> ChainEnv {
    let dir = tempfile::tempdir().expect("临时目录");
    let events = Arc::new(RecordingEventSink::new());
    let host = Arc::new(McpHost::new(dir.path().to_path_buf(), events.clone()));
    // 配置：stdio 形态（command = fixture 名；池测试钩子按名注入进程内
    // server），未知字段顺带覆盖 round-trip 面
    let config: McpServerConfig = serde_json::from_value(serde_json::json!({
        "command": config_name,
        "args": [],
        "timeout": 5,
        "probeField": "round-trip 无关紧要但必须在"
    }))
    .expect("配置构造");
    let fingerprint = launch_fingerprint(&TrustSubject {
        scope: McpScope::Global,
        project_id: None,
        server_name: config_name.to_owned(),
        canonical_config_path: dir.path().join("mcp.json").to_string_lossy().into_owned(),
        config_bytes: b"{}".to_vec(),
    });
    // 预置信任（TOFU 弹窗链路在 commands 测试覆盖）
    host.trust
        .decide(&fingerprint, LaunchDecision::Remember, 1)
        .expect("预置信任");
    let fixture = EchoFixture::new();
    fixture.install(config_name);
    let key = ConnKey {
        scope: McpScope::Global,
        project_id: None,
        name: config_name.to_owned(),
        canonical_path: String::new(),
    };
    host.connect(key.clone(), config.clone(), fingerprint.clone());
    wait_state(&host, &key, ConnState::Ready, 200).await;
    ChainEnv {
        host,
        fixture,
        key,
        fingerprint,
        config,
        config_name: config_name.to_owned(),
        _dir: dir,
        events,
    }
}

/// 轮询等待连接到某状态（事件驱动界面上是 mcp:status；测试面轮询状态机）。
async fn wait_state(host: &Arc<McpHost>, key: &ConnKey, want: ConnState, ticks: usize) {
    for _ in 0..ticks {
        if let Some((state, _)) = host.state_of(key)
            && state == want
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("连接未到达 {want:?}（当前 {:?}）", host.state_of(key));
}

/// 全链 A：connect → 工具发现注册（进模型工具表）→ 调用回喂 → 状态四态
/// 事件可见。
#[tokio::test(flavor = "current_thread")]
async fn connect_discover_call_and_status_events() {
    let _guard = chain_lock().await;
    let env = ready_env("fixture-call").await;
    // 工具发现：echo/add 进入动态注册表（模型工具表动态段）
    let echo = env.host.registry().identity("echo").expect("echo 已注册");
    assert_eq!(echo.original_name, "echo");
    assert_eq!(echo.conn_key.name, env.config_name);
    assert!(
        env.host
            .registry()
            .snapshot()
            .iter()
            .any(|s| s.name == "add"),
        "add 也在动态段"
    );
    // 调用回喂（pool::call_tool）
    let outcome = super::pool::call_tool(&env.host, "echo", r#"{"message":"你好"}"#).await;
    assert!(outcome.ok, "echo 调用成功：{:?}", outcome.error);
    assert!(
        outcome.ui_summary.contains("echo 调用成功"),
        "UI 摘要：{}",
        outcome.ui_summary
    );
    let data = outcome.data.expect("data 在场");
    assert_eq!(data["content"], "echo: 你好");
    assert_eq!(env.fixture.count(), 1);
    // add 工具
    let outcome = super::pool::call_tool(&env.host, "add", r#"{"a":2,"b":40}"#).await;
    assert!(outcome.ok);
    assert_eq!(outcome.data.expect("data")["content"], "42");
    // 状态事件：Starting → Ready 至少两次迁移，state 值逐字（§3.5）
    let statuses: Vec<serde_json::Value> = env
        .events
        .payloads_of(keys::MCP_STATUS)
        .into_iter()
        .collect();
    assert!(
        statuses
            .iter()
            .any(|p| p["state"] == "starting" && p["name"] == env.config_name),
        "starting 态在事件里：{statuses:?}"
    );
    assert!(
        statuses.iter().any(|p| p["state"] == "ready"
            && p["scope"] == "global"
            && p["name"] == env.config_name),
        "ready 态在事件里：{statuses:?}"
    );
    env.host.disconnect(&env.key);
    assert!(
        env.events
            .payloads_of(keys::MCP_STATUS)
            .iter()
            .any(|p| p["state"] == "stopped"),
        "stopped 态在事件里"
    );
}

/// 全链 B：服务端断连 → 自动重连（新连接）→ 工具再次可用；用户显式
/// disconnect 后不再自动重连（Stopped 恒定）。
#[tokio::test(flavor = "current_thread")]
async fn auto_reconnect_after_server_kill() {
    let _guard = chain_lock().await;
    let env = ready_env("fixture-reconnect").await;
    // kill server → Error → 退避重连 → Ready（指数退避 1s 起：首次等待
    // 覆盖 1s+ 重连握手，ticks 放宽到 10s）
    env.fixture.kill_server();
    wait_state(&env.host, &env.key, ConnState::Error, 100).await;
    wait_state(&env.host, &env.key, ConnState::Ready, 500).await;
    // 重连后（新连接、新清单同步）工具再次可用
    let outcome = super::pool::call_tool(&env.host, "echo", r#"{"message":"重连后"}"#).await;
    assert!(outcome.ok, "重连后 echo 可用：{:?}", outcome.error);
    // error 态事件在案（mcp:status 全程可见）
    assert!(
        env.events
            .payloads_of(keys::MCP_STATUS)
            .iter()
            .any(|p| p["state"] == "error"),
        "error 态在事件里"
    );
    // 用户显式断开：desired=false → server 再崩也不重连
    env.host.disconnect(&env.key);
    env.fixture.kill_server();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        env.host.state_of(&env.key).map(|(s, _)| s),
        Some(ConnState::Stopped),
        "断开后停在 Stopped"
    );
}

/// 全链 C（Q14 结果未知不重放）：arm die → 调用 → server 记录执行后挂住
/// → kill → 调用收到 E_MCP_RESULT_UNKNOWN（可能已执行）；计数保持 1；
/// read_only 声明同样覆盖（不豁免）。
#[tokio::test(flavor = "current_thread")]
async fn result_unknown_never_replayed() {
    let _guard = chain_lock().await;
    let env = ready_env("fixture-die").await;
    env.fixture.arm_die_after_record();
    // 调用在后台走（server 会记录后挂住）
    let host = env.host.clone();
    let call_task =
        tokio::spawn(async move { super::pool::call_tool(&host, "die_after_record", "{}").await });
    // 等 server 记录执行（计数 1）——「记录已执行操作后、返回响应前」
    for _ in 0..100 {
        if env.fixture.count() == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(env.fixture.count(), 1, "server 已执行一次");
    // 断连：响应永远发不出去
    env.fixture.kill_server();
    let outcome = call_task.await.expect("调用任务收口");
    assert!(!outcome.ok);
    let err = outcome.error.expect("错误面");
    assert_eq!(err.code, super::pool::E_MCP_RESULT_UNKNOWN);
    assert!(err.message.contains("可能已经执行"), "行动指引在消息里");
    assert_eq!(env.fixture.count(), 1, "连接恢复路径没有重放调用");
    // detail 结构化标记（UI/边车识别面）
    assert_eq!(outcome.detail.expect("detail")["resultUnknown"], true);
    // 连接自动恢复后，显式重试（由主代理决定）计数 +1
    wait_state(&env.host, &env.key, ConnState::Ready, 500).await;
    let outcome = super::pool::call_tool(&env.host, "echo", r#"{"message":"核查后重试"}"#).await;
    assert!(outcome.ok);
    assert_eq!(env.fixture.count(), 2, "显式重试才第二次执行");
    env.host.disconnect(&env.key);
}

/// 全链 D：tools/list_changed → 重拉清单热更新注册表（动态新增工具出现，
/// Q44 下新工具不继承旧许可——身份三元组按新清单现算）。
#[tokio::test(flavor = "current_thread")]
async fn list_changed_hot_sync() {
    let _guard = chain_lock().await;
    let env = ready_env("fixture-sync").await;
    assert!(
        !env.host
            .registry()
            .snapshot()
            .iter()
            .any(|s| s.name == "brand_new"),
        "新工具初始不在场"
    );
    // 经工具调用触发 server 动态新增 + 通知
    let outcome = super::pool::call_tool(&env.host, "add_tool", r#"{"name":"brand_new"}"#).await;
    assert!(outcome.ok, "add_tool 调用成功");
    // 通知 → 重拉清单 → 热更新（轮询等）
    for _ in 0..100 {
        if env
            .host
            .registry()
            .snapshot()
            .iter()
            .any(|s| s.name == "brand_new")
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        env.host
            .registry()
            .snapshot()
            .iter()
            .any(|s| s.name == "brand_new"),
        "新工具经 tools/list_changed 热更新进动态段"
    );
    // 就绪态事件带清单更新明细（toolSpecs 重导）
    assert!(
        env.events
            .payloads_of(keys::MCP_STATUS)
            .iter()
            .any(|p| p["state"] == "ready"
                && p["detail"]
                    .as_str()
                    .is_some_and(|d| d.contains("工具清单已更新"))),
        "清单热更新有 ready 事件"
    );
    env.host.disconnect(&env.key);
}

/// LRU 驱逐：超容（>16）淘汰非期望保持 + 最久未用优先。
#[tokio::test(flavor = "current_thread")]
async fn lru_eviction() {
    let _guard = chain_lock().await;
    let dir = tempfile::tempdir().expect("临时目录");
    let events = Arc::new(RecordingEventSink::new());
    let host = Arc::new(McpHost::new(dir.path().to_path_buf(), events));
    // 纯状态条目（不经真实连接：connect_task 会失败进重连——驱逐逻辑只看
    // 表长与 last_used/desired，失败条目同样可被驱逐）。造 17 个条目：
    // 直连 connect 会 spawn 任务；用 disconnect 后的手工形态太绕——这里
    // 走真实 connect（fixture 未注册 → 连接失败 → Error），驱逐断言照常。
    let mut keys = Vec::new();
    for i in 0..(super::pool::POOL_CAPACITY + 1) {
        let key = ConnKey {
            scope: McpScope::Global,
            project_id: None,
            name: format!("srv-{i}"),
            canonical_path: String::new(),
        };
        let config: McpServerConfig =
            serde_json::from_value(serde_json::json!({ "command": "no-such-binary" }))
                .expect("配置");
        let fingerprint = format!("fp-{i}");
        host.trust
            .decide(&fingerprint, LaunchDecision::Remember, 1)
            .expect("预置信任");
        host.connect(key.clone(), config, fingerprint);
        keys.push(key);
    }
    // 等第一批都进表（Starting/Error 均在表内）且超容驱逐发生
    for _ in 0..200 {
        if host.snapshot().len() <= super::pool::POOL_CAPACITY {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let snapshot = host.snapshot();
    assert!(
        snapshot.len() <= super::pool::POOL_CAPACITY,
        "池容量受 LRU 上限约束：{}",
        snapshot.len()
    );
    assert!(
        !snapshot.iter().any(|s| s.name == "srv-0"),
        "最旧的条目被驱逐"
    );
    for key in &keys {
        host.disconnect(key);
    }
}

/// generation 失效：断开后旧断线回调/旧握手不得改写新一代状态（防旧握手
/// 复活）。用 disconnect 后直接断言 Stopped 稳定 + 事件流不再出现 ready。
#[tokio::test(flavor = "current_thread")]
async fn generation_invalidates_stale_callbacks() {
    let _guard = chain_lock().await;
    let env = ready_env("fixture-generation").await;
    // 断开（generation+1）后立刻杀 server：旧连接的断线回调带旧 generation，
    // 不得把状态改写为 Error/Ready（desired=false 也不再重连）。
    env.host.disconnect(&env.key);
    env.fixture.kill_server();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        env.host.state_of(&env.key).map(|(s, _)| s),
        Some(ConnState::Stopped),
        "旧回调未复活状态"
    );
    // 事件流的最后状态是 stopped（无 error/ready 反扑）
    let statuses = env.events.payloads_of(keys::MCP_STATUS);
    let last = statuses.last().expect("有状态事件");
    assert_eq!(last["state"], "stopped");
    // 再 connect（新一代）→ ready 照常可达（generation 前进不受旧回调拖累）
    env.host
        .connect(env.key.clone(), env.config.clone(), env.fingerprint.clone());
    wait_state(&env.host, &env.key, ConnState::Ready, 200).await;
    env.host.disconnect(&env.key);
}

/// always_allow 自动授权（Q44）：清单同步后批量授予当前版本许可；
/// 输入 schema 有未确认变更（许可表存在同工具不同 hash 的记录）的工具
/// 不自动放行——等用户首次调用重新确认后才命中新版本。
#[tokio::test(flavor = "current_thread")]
async fn always_allow_auto_grant_respects_schema_changes() {
    let _guard = chain_lock().await;
    let dir = tempfile::tempdir().expect("临时目录");
    let host = Arc::new(McpHost::new(
        dir.path().to_path_buf(),
        Arc::new(RecordingEventSink::new()),
    ));
    let key = ConnKey {
        scope: McpScope::Global,
        project_id: None,
        name: "grant-srv".into(),
        canonical_path: String::new(),
    };
    let fingerprint = "grant-fp".to_owned();
    let mk_list = |schema: serde_json::Value| {
        vec![(
            "grantprobe".to_owned(),
            "自动授权探针".to_owned(),
            schema,
            crate::tools::ToolAnnotations {
                read_only_hint: false,
                destructive_hint: false,
                idempotent_hint: false,
                open_world_hint: false,
            },
        )]
    };
    let cfg = McpServerConfig {
        always_allow: true,
        ..McpServerConfig::default()
    };
    let registry = crate::mcp::tools::McpToolRegistry::global();
    // v1 schema 同步 + 自动授权 → 命中（免审批）
    registry.sync(
        key.clone(),
        fingerprint.clone(),
        &cfg,
        mk_list(serde_json::json!({ "type": "object" })),
    );
    host.auto_grant_licenses(&key, &fingerprint);
    let hash_v1 = registry
        .identity("grantprobe")
        .expect("v1 在场")
        .schema_hash;
    assert!(
        host.licenses
            .is_licensed(&fingerprint, "grantprobe", &hash_v1),
        "首次同步自动授予当前版本许可"
    );
    // v2 schema（变更未确认）：自动授权不得授予新 hash——首次调用仍挂起
    registry.sync(
        key.clone(),
        fingerprint.clone(),
        &cfg,
        mk_list(
            serde_json::json!({ "type": "object", "properties": { "a": { "type": "string" } } }),
        ),
    );
    host.auto_grant_licenses(&key, &fingerprint);
    let hash_v2 = registry
        .identity("grantprobe")
        .expect("v2 在场")
        .schema_hash;
    assert_ne!(hash_v1, hash_v2);
    assert!(
        !host
            .licenses
            .is_licensed(&fingerprint, "grantprobe", &hash_v2),
        "schema 变更未确认不自动放行（Q44：always_allow 不覆盖新版本）"
    );
    // 用户在审批面板点「始终允许」→ 新版本许可落地 → 命中
    host.licenses
        .grant(&fingerprint, "grantprobe", &hash_v2, 1)
        .expect("用户确认");
    assert!(
        host.licenses
            .is_licensed(&fingerprint, "grantprobe", &hash_v2)
    );
    // 用例尾：摘除动态段（进程级单例复原）
    registry.remove(&key);
}

/// streamable HTTP 传输全链（FR-08-01 双传输验收的 HTTP 侧）：本地 loopback
/// axum 服务承载 rmcp StreamableHttpService（复用 fixture 的 EchoServer 工具
/// 面，127.0.0.1 随机端口、零外网），客户端按 url 配置走生产
/// `open_connection` 的 HTTP 分支——握手、tools/list 与 stdio 全链同规。
/// 仅测试构建（rmcp server/HTTP feature 只在 dev-dependencies）。
#[tokio::test(flavor = "current_thread")]
async fn http_transport_connects_and_lists_tools() {
    use crate::mcp::fixture::{EchoServer, EchoState};

    // —— loopback server：axum 承载 rmcp 的 streamable HTTP 服务 ——
    let state = Arc::new(EchoState::default());
    let server = EchoServer::new(
        state.clone(),
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    );
    let service = rmcp::transport::streamable_http_server::StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(
            rmcp::transport::streamable_http_server::session::local::LocalSessionManager::default(),
        ),
        Default::default(),
    );
    let app = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地回环端口");
    let addr = listener.local_addr().expect("端口");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("HTTP 服务收口");
    });

    // —— 客户端：url-only 配置走生产 HTTP 分支（fixture 注册表按 command
    // 命中，url-only 不命中即真实 streamable HTTP 传输）——
    let dir = tempfile::tempdir().expect("临时目录");
    let host = Arc::new(McpHost::new(
        dir.path().to_path_buf(),
        Arc::new(RecordingEventSink::new()),
    ));
    let config: McpServerConfig = serde_json::from_value(serde_json::json!({
        "url": format!("http://{addr}/mcp"),
    }))
    .expect("HTTP 配置");
    let connection = super::pool::open_for_test(&config, host.clone())
        .await
        .expect("streamable HTTP 握手");
    let tools = super::pool::list_tools(&connection.peer, std::time::Duration::from_secs(10))
        .await
        .expect("tools/list");
    let names: Vec<&str> = tools.iter().map(|(name, ..)| name.as_str()).collect();
    assert!(
        names.contains(&"echo") && names.contains(&"add"),
        "HTTP 传输工具清单：{names:?}"
    );
    // 调用回喂由 stdio 全链覆盖（TestConnection 不进池，池路径 call_tool
    // 不适用临时连接）；Drop 收口 = 取消服务任务
    drop(connection);
}
