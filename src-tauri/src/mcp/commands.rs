//! MCP 命令实现（§3.3 #40–47 逐字；08 号票）+ TOFU 启动信任询问槽。
//!
//! TOFU 询问不属任何 run（管理页连接触发），因此挂在 AppState 的专用槽表
//! （`mcp_asks`）而非 RunHandle.ask；`resolve_ask` 命令在 run 槽未命中时
//! 落到这里。应答经既有 [`crate::backend::ask::AskAnswer::Approval`]（
//! optionId = allow_once/remember/deny），协议面不新增应答形态。

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use super::config::{McpConfigFile, McpConfigStore, McpScope, McpServerConfig};
use super::pool::{ConnState, McpHost, call_tool};
use super::tools::ConnKey;
use super::trust::{LaunchDecision, TrustSubject, launch_fingerprint};
use crate::backend::ask::{AskAnswer, AskOption, AskPayload, McpTransportKind};
use crate::backend::gate::{McpGateView, McpToolView};
use crate::backend::run_loop::E_CANCELLED;
use crate::host::AppState;
use crate::tools::ToolOutcome;
use crate::util::errors::IpcError;

/// MCP TOFU 询问槽（AppState.mcp_asks 的元素）：ask_id → 原始载荷 + 应答
/// 通道（oneshot 一次性通道）。
pub struct PendingTrustAsk {
    /// 原始载荷（mcpTrust 形态）。
    pub payload: AskPayload,
    /// 应答发送端。
    pub tx: oneshot::Sender<AskAnswer>,
}

/// mcpTrust 三选项（§3.2 options id 逐字）。
fn trust_options() -> Vec<AskOption> {
    vec![
        AskOption {
            id: "allow_once".to_owned(),
            label: "仅本次连接".to_owned(),
            recommended: None,
            mode: None,
        },
        AskOption {
            id: "remember".to_owned(),
            label: "记住此版本".to_owned(),
            recommended: None,
            mode: None,
        },
        AskOption {
            id: "deny".to_owned(),
            label: "拒绝".to_owned(),
            recommended: None,
            mode: None,
        },
    ]
}

// —— 命令契约类型（§3.3 #40–47；derive TS 进生成物）——

/// `mcp_list_config` 参数契约（§3.3 #40）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpListConfigArgs {}

/// `mcp_list_config` 返回契约：双作用域原样配置（未知字段保真）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpListConfigResult {
    /// 全局作用域文件内容（缺失 = 空配置）。
    pub global: McpConfigFile,
    /// 项目作用域文件内容（无项目上下文 = 空配置）。
    pub project: McpConfigFile,
    /// 项目作用域是否可编辑（当前活跃会话有归属项目）。
    pub project_available: bool,
}

/// `mcp_save_config` 参数契约（§3.3 #41）：整份保存一个作用域。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpSaveConfigArgs {
    /// 目标作用域。
    pub scope: McpScope,
    /// 整份配置（未知字段随 flatten round-trip）。
    pub config: McpConfigFile,
}

/// `mcp_connect` / `mcp_disconnect` / `mcp_reconnect` 参数契约（§3.3 #42–44）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpServerRefArgs {
    /// 目标作用域。
    pub scope: McpScope,
    /// 服务器配置名。
    pub name: String,
}

/// `mcp_test` 参数契约（§3.3 #45）：草稿条目（不经保存直接试连）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpTestArgs {
    /// 草稿服务器配置（未知字段允许，试连不校验它们）。
    pub draft: McpServerConfig,
}

/// `mcp_test` 的单工具行。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpTestTool {
    /// 工具名（原始名，未做归一）。
    pub name: String,
    /// 工具描述。
    pub description: String,
}

/// `mcp_test` 返回契约（§3.3 #45）：ok + 工具清单或错误（总预算 60s）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpTestResult {
    /// 是否连通（initialize + tools/list 完成）。
    pub ok: bool,
    /// 发现的工具清单。
    pub tools: Vec<McpTestTool>,
    /// 失败原因（ok=false 时）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub error: Option<String>,
}

/// `mcp_set_launch_trust` 参数契约（§3.3 #46）：管理页直接写/撤销信任。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpSetLaunchTrustArgs {
    /// 启动指纹（完整 hex）。
    pub fingerprint: String,
    /// 决策：remember（记住此版本）/ revoke（撤销）。
    pub decision: LaunchDecision,
}

/// `mcp_snapshot` 参数契约（§3.3 #47，空参）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpSnapshotArgs {}

/// `mcp_snapshot` 的单服务器行（§3.3 #47 逐字段 + 补充面：scope/detail/
/// fingerprint/trustStatus 为 08 号票管理页数据面扩展）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpSnapshotServer {
    /// 配置名。
    pub name: String,
    /// 作用域。
    pub scope: McpScope,
    /// 连接状态（"starting"|"ready"|"stopped"|"error"；池无条目 = "stopped"）。
    pub state: String,
    /// 工具数（Ready 时）。
    pub tool_count: usize,
    /// 状态明细。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub detail: Option<String>,
    /// 启动指纹前 16 位（信任列展示；配置条目解析即算）。
    pub fingerprint: String,
    /// 信任状态（"trusted"|"pending"|"revoked"）。
    pub trust_status: String,
}

/// `mcp_snapshot` 返回契约。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpSnapshotResult {
    /// 池内服务器状态行。
    pub servers: Vec<McpSnapshotServer>,
}

// —— AppState 侧的共享解析（host 装配面）——

/// 当前项目上下文（project 作用域的解析面）：活跃会话归属的项目。
/// 收敛点（票内记录）：设置页无会话参数，project 作用域锚定「当前活跃
/// 会话的项目」；无活跃会话/会话无项目 → project 不可用。
pub fn project_context_of(app: &AppState) -> Option<(String, PathBuf)> {
    let session_id = app
        .active_session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()?;
    let meta = app.store.index().get(&session_id)?;
    let project_id = meta.project_id.clone()?;
    let home = app.projects.home_of(&project_id)?;
    Some((project_id, home))
}

/// 构造作用域配置存储（全局路径 = 数据目录根；项目路径 = 当前项目）。
fn config_store_of(app: &AppState) -> McpConfigStore {
    let project_home = project_context_of(app).map(|(_, home)| home);
    McpConfigStore::new(app.mcp.data_dir(), project_home.as_deref())
}

/// 解析一个服务器条目：返回（配置, 连接键, 指纹）。信任计算读取配置文件
/// 原始字节（§9.7 sha256(config) = 文件整体）。
fn resolve_server(
    app: &AppState,
    scope: McpScope,
    name: &str,
) -> Result<(McpServerConfig, ConnKey, String), IpcError> {
    let store = config_store_of(app);
    let path = store
        .path_of(scope)
        .ok_or_else(|| IpcError::invalid_arg("project 作用域不可用：当前无项目上下文"))?;
    let bytes = std::fs::read(path).unwrap_or_default();
    let file: McpConfigFile = serde_json::from_slice(&bytes).map_err(|e| {
        IpcError::new(
            crate::util::errors::E_STORAGE_IO,
            format!("解析 mcp.json 失败：{e}"),
        )
    })?;
    let (servers, _bad) = file.parse_servers();
    let config = servers
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, c)| c)
        .ok_or_else(|| IpcError::not_found(format!("MCP 配置不存在：{scope:?}/{name}")))?;
    let canonical = std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string_lossy().into_owned());
    let project_id = project_context_of(app).map(|(id, _)| id);
    let key = ConnKey {
        scope,
        project_id: project_id.clone(),
        name: name.to_owned(),
        canonical_path: canonical.clone(),
    };
    let fingerprint = launch_fingerprint(&TrustSubject {
        scope,
        project_id,
        server_name: name.to_owned(),
        canonical_config_path: canonical,
        config_bytes: bytes,
    });
    Ok((config, key, fingerprint))
}

/// TOFU 信任询问（§9.7）：无信任记录 → 挂 ask 槽 + ask:opened → 等应答。
/// 返回 Some(选项 id)（allow_once/remember）；deny/取消 → None。
async fn ensure_launch_trust(
    app: &AppState,
    name: &str,
    config: &McpServerConfig,
    fingerprint: &str,
) -> Result<Option<String>, IpcError> {
    if app.mcp.trust.is_trusted(fingerprint) {
        return Ok(Some("remember".to_owned()));
    }
    let transport = if config.command.is_some() {
        McpTransportKind::Stdio
    } else {
        McpTransportKind::Http
    };
    let command_or_url = if let Some(cmd) = &config.command {
        let mut text = cmd.clone();
        for arg in &config.args {
            text.push(' ');
            text.push_str(arg);
        }
        text
    } else {
        config.url.clone().unwrap_or_default()
    };
    let payload = AskPayload::McpTrust {
        server: name.to_owned(),
        transport,
        fingerprint: fingerprint.chars().take(16).collect(),
        command_or_url,
        options: trust_options(),
    };
    let (tx, rx) = oneshot::channel::<AskAnswer>();
    let ask_id = uuid::Uuid::new_v4().to_string();
    {
        let mut asks = app
            .mcp_asks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        asks.insert(
            ask_id.clone(),
            PendingTrustAsk {
                payload: payload.clone(),
                tx,
            },
        );
    }
    app.events.emit(
        crate::util::events::keys::ASK_OPENED,
        serde_json::json!({
            "sessionId": "",
            "askId": ask_id,
            "payload": payload,
        }),
    );
    // 等待应答（resolve_ask 投递）；发送端 drop（应用退出/槽被清）→ 取消。
    let answer = rx.await;
    let option_id = match answer {
        Ok(AskAnswer::Approval { option_id, .. }) => option_id,
        Ok(AskAnswer::Question { .. }) => String::from("deny"),
        Err(_) => String::from("deny"),
    };
    // 单一出口：清槽 + ask:closed（answered=决策已达）。
    app.mcp_asks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&ask_id);
    app.events.emit(
        crate::util::events::keys::ASK_CLOSED,
        serde_json::json!({ "askId": ask_id, "answered": true }),
    );
    match option_id.as_str() {
        "allow_once" | "remember" => {
            if option_id == "remember" {
                app.mcp
                    .trust
                    .decide(fingerprint, LaunchDecision::Remember, now_ms())
                    .map_err(|e| IpcError::new(crate::util::errors::E_STORAGE_IO, e))?;
            }
            Ok(Some(option_id))
        }
        _ => Ok(None),
    }
}

/// 当前毫秒时间戳（epoch ms；与 chat.rs now_ms 同款实现）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// TOFU 询问槽的应答投递（resolve_ask 的 run 槽未命中兜底；host 装配）。
/// 命中即取走槽位（oneshot 发送端不可克隆；取走后等待方收到应答并自清
/// ask:closed——见 [`ensure_launch_trust`] 的单一出口）。
pub async fn resolve_trust_ask(app: &AppState, ask_id: &str, answer: AskAnswer) -> bool {
    let tx = app
        .mcp_asks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(ask_id)
        .map(|slot| slot.tx);
    match tx {
        Some(tx) => tx.send(answer).is_ok(),
        None => false,
    }
}

// —— 8 个命令（§3.3 #40–47 逐字）——

/// `mcp_list_config`（§3.3 #40）：双作用域原样配置。
#[tauri::command]
pub(crate) async fn mcp_list_config(
    app: tauri::State<'_, AppState>,
) -> Result<McpListConfigResult, IpcError> {
    mcp_list_config_core(&app)
}

/// mcp_list_config 主体（headless 测试直驱）。
pub fn mcp_list_config_core(app: &AppState) -> Result<McpListConfigResult, IpcError> {
    let store = config_store_of(app);
    let global = store
        .load(McpScope::Global)
        .map_err(IpcError::invalid_arg)?;
    let project = if store.project_path.is_some() {
        store
            .load(McpScope::Project)
            .map_err(IpcError::invalid_arg)?
    } else {
        McpConfigFile::default()
    };
    Ok(McpListConfigResult {
        global,
        project,
        project_available: store.project_path.is_some(),
    })
}

/// `mcp_save_config`（§3.3 #41）：整份原子保存（未知字段随文件保真）。
#[tauri::command]
pub(crate) async fn mcp_save_config(
    app: tauri::State<'_, AppState>,
    scope: McpScope,
    config: McpConfigFile,
) -> Result<(), IpcError> {
    mcp_save_config_core(&app, scope, &config)
}

/// mcp_save_config 主体。
pub fn mcp_save_config_core(
    app: &AppState,
    scope: McpScope,
    config: &McpConfigFile,
) -> Result<(), IpcError> {
    // 保存前校验全部条目可解析（防保存一份坏配置导致后续加载失败）。
    let (_, bad) = config.parse_servers();
    if !bad.is_empty() {
        return Err(IpcError::invalid_arg(format!(
            "配置条目解析失败：{}",
            bad.join("；")
        )));
    }
    let store = config_store_of(app);
    store.save(scope, config).map_err(IpcError::invalid_arg)
}

/// `mcp_connect`（§3.3 #42）：信任核验（TOFU）→ 发起连接（后台任务，
/// 状态经 mcp:status 可见）。
#[tauri::command]
pub(crate) async fn mcp_connect(
    app: tauri::State<'_, AppState>,
    scope: McpScope,
    name: String,
) -> Result<(), IpcError> {
    mcp_connect_core(&app, scope, &name).await
}

/// mcp_connect 主体。
pub async fn mcp_connect_core(app: &AppState, scope: McpScope, name: &str) -> Result<(), IpcError> {
    let (config, key, fingerprint) = resolve_server(app, scope, name)?;
    if !config.enabled {
        return Err(IpcError::invalid_arg(format!(
            "MCP 服务器 {name} 已停用（enabled=false）"
        )));
    }
    let host = app.mcp.clone();
    match ensure_launch_trust(app, name, &config, &fingerprint).await? {
        Some(_) => {
            // 信任达成 → 连接（后台任务；TOFU/连接失败都经 mcp:status 可见）。
            host.connect(key, config, fingerprint);
            Ok(())
        }
        None => Err(IpcError::new(
            E_CANCELLED,
            format!("MCP 服务器 {name} 的启动信任被拒绝，未连接"),
        )),
    }
}

/// `mcp_disconnect`（§3.3 #43）。
#[tauri::command]
pub(crate) async fn mcp_disconnect(
    app: tauri::State<'_, AppState>,
    scope: McpScope,
    name: String,
) -> Result<(), IpcError> {
    mcp_disconnect_core(&app, scope, &name)
}

/// mcp_disconnect 主体。
pub fn mcp_disconnect_core(app: &AppState, scope: McpScope, name: &str) -> Result<(), IpcError> {
    let (_, key, _) = resolve_server(app, scope, name)?;
    app.mcp.disconnect(&key);
    Ok(())
}

/// `mcp_reconnect`（§3.3 #44）：断开重连（指纹变更会经 TOFU 重问——
/// connect 内的 ensure_launch_trust 兜底）。
#[tauri::command]
pub(crate) async fn mcp_reconnect(
    app: tauri::State<'_, AppState>,
    scope: McpScope,
    name: String,
) -> Result<(), IpcError> {
    mcp_reconnect_core(&app, scope, &name).await
}

/// mcp_reconnect 主体。
pub async fn mcp_reconnect_core(
    app: &AppState,
    scope: McpScope,
    name: &str,
) -> Result<(), IpcError> {
    let (config, key, fingerprint) = resolve_server(app, scope, name)?;
    if !config.enabled {
        return Err(IpcError::invalid_arg(format!(
            "MCP 服务器 {name} 已停用（enabled=false）"
        )));
    }
    match ensure_launch_trust(app, name, &config, &fingerprint).await? {
        Some(_) => {
            app.mcp.reconnect(&key, config, fingerprint);
            Ok(())
        }
        None => Err(IpcError::new(
            E_CANCELLED,
            format!("MCP 服务器 {name} 的启动信任被拒绝，未重连"),
        )),
    }
}

/// `mcp_test`（§3.3 #45）：草稿试连（initialize + tools/list），总预算
/// 60s，不进池、不写信任（试连 = 只读探测）。
#[tauri::command]
pub(crate) async fn mcp_test(draft: McpTestArgs) -> Result<McpTestResult, IpcError> {
    Ok(mcp_test_core(draft).await)
}

/// mcp_test 主体（总预算 60s：握手 30s + 清单 30s 的和上界在此收口）。
pub async fn mcp_test_core(args: McpTestArgs) -> McpTestResult {
    let budget = tokio::time::Duration::from_secs(60);
    match tokio::time::timeout(budget, test_once(&args.draft)).await {
        Ok(Ok(tools)) => McpTestResult {
            ok: true,
            tools,
            error: None,
        },
        Ok(Err(e)) => McpTestResult {
            ok: false,
            tools: Vec::new(),
            error: Some(e),
        },
        Err(_) => McpTestResult {
            ok: false,
            tools: Vec::new(),
            error: Some("测试连接超时（总预算 60 秒）".to_owned()),
        },
    }
}

/// 一次草稿试连：临时连接 → tools/list → 断开（不进池、不写信任）。
async fn test_once(config: &McpServerConfig) -> Result<Vec<McpTestTool>, String> {
    // 试连 = 只读探测：不经信任门；stdio 同样会启动子进程——沙箱边界与
    // 正式连接一致（进程组 + 收割），临时宿主只承载清单变更回调。
    let dead_host = Arc::new(McpHost::new(
        std::env::temp_dir().join("craftwave-mcp-test"),
        Arc::new(crate::util::events::RecordingEventSink::new()),
    ));
    let connection = super::pool::open_for_test(config, dead_host).await?;
    let tools =
        super::pool::list_tools(&connection.peer, tokio::time::Duration::from_secs(30)).await?;
    drop(connection);
    Ok(tools
        .into_iter()
        .map(|(name, description, _, _)| McpTestTool { name, description })
        .collect())
}

/// `mcp_set_launch_trust`（§3.3 #46）：决策写 `trust/mcp.json`（§7.7 trust
/// 域；§9.7 keyring 字样已随 08 号票回写为文件存储）。
#[tauri::command]
pub(crate) async fn mcp_set_launch_trust(
    app: tauri::State<'_, AppState>,
    fingerprint: String,
    decision: LaunchDecision,
) -> Result<(), IpcError> {
    mcp_set_launch_trust_core(&app, &fingerprint, decision)
}

/// mcp_set_launch_trust 主体。revoke 时连带撤销该服务器全部工具许可
/// （服务器身份不可信 → 工具许可不得延续，Q44）。
pub fn mcp_set_launch_trust_core(
    app: &AppState,
    fingerprint: &str,
    decision: LaunchDecision,
) -> Result<(), IpcError> {
    app.mcp
        .trust
        .decide(fingerprint, decision, now_ms())
        .map_err(IpcError::invalid_arg)?;
    if decision == LaunchDecision::Revoke {
        app.mcp
            .licenses
            .revoke_server(fingerprint)
            .map_err(IpcError::invalid_arg)?;
    }
    Ok(())
}

/// `mcp_snapshot`（§3.3 #47）：配置 × 连接池 × 信任状态快照。
#[tauri::command]
pub(crate) async fn mcp_snapshot(
    app: tauri::State<'_, AppState>,
) -> Result<McpSnapshotResult, IpcError> {
    mcp_snapshot_core(&app)
}

/// mcp_snapshot 主体（管理页完整数据面：配置合并视图 × 连接池状态 × 信任
/// 状态。配置里有但从未连接的服务器同样成行——state="stopped"、
/// trustStatus 按 `trust/mcp.json` 实况；配置已删除的服务器不成行，其池内
/// 残留条目由断线监控/驱逐自然收口）。配置读取失败按参数错误上抛——
/// 管理页据错误提示用户修配置，静默空表会掩盖坏文件。
pub fn mcp_snapshot_core(app: &AppState) -> Result<McpSnapshotResult, IpcError> {
    let mut servers = Vec::new();
    for (name, scope, _config) in config_store_of(app)
        .merged()
        .map_err(IpcError::invalid_arg)?
    {
        // merged() 已保证条目可解析，resolve_server 失败（project 不可用等）
        // 跳过该行而非整体失败——单行故障不拖垮快照。
        let Ok((_, key, fingerprint)) = resolve_server(app, scope, &name) else {
            continue;
        };
        let (state, detail) = app
            .mcp
            .state_of(&key)
            .map(|(s, d)| (s.as_str().to_owned(), d))
            .unwrap_or_else(|| (ConnState::Stopped.as_str().to_owned(), None));
        servers.push(McpSnapshotServer {
            name,
            scope,
            state,
            tool_count: app.mcp.registry().tool_count(&key),
            detail,
            fingerprint: fingerprint.chars().take(16).collect(),
            trust_status: app.mcp.trust.status_of(&fingerprint).as_str().to_owned(),
        });
    }
    Ok(McpSnapshotResult { servers })
}

/// MCP 判定视图适配（实现 backend::gate::McpGateView；host 装配到
/// ApprovalGate）。
pub struct McpGateAdapter {
    /// 宿主句柄（信任/许可存储与注册表都在其上）。
    pub host: Arc<McpHost>,
}

impl McpGateView for McpGateAdapter {
    fn view(&self, normalized_name: &str) -> Option<McpToolView> {
        let identity = self.host.registry().identity(normalized_name)?;
        Some(McpToolView {
            server_name: identity.server_name,
            server_fingerprint: identity.server_fingerprint,
            original_name: identity.original_name,
            schema_hash: identity.schema_hash,
            read_only: identity.read_only,
        })
    }

    fn license_hit(&self, view: &McpToolView) -> bool {
        self.host.licenses.is_licensed(
            &view.server_fingerprint,
            &view.original_name,
            &view.schema_hash,
        )
    }
}

/// MCP 调用面适配（实现 backend::exec::McpCaller）。
pub struct McpCallerAdapter {
    /// 宿主句柄（连接池与注册表）。
    pub host: Arc<McpHost>,
}

impl crate::backend::exec::McpCaller for McpCallerAdapter {
    fn call(
        &self,
        normalized_name: &str,
        args_json: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolOutcome> + Send + '_>> {
        // Arc 克隆进 future（生命周期统一到 &self；入参字符串克隆避免
        // 多重 borrow 交叉）
        let host = self.host.clone();
        let normalized_name = normalized_name.to_owned();
        let args_json = args_json.to_owned();
        Box::pin(async move { call_tool(&host, &normalized_name, &args_json).await })
    }
}

/// 未连接错误码再导出（commands/chat.rs 审批与测试用）。
pub use super::pool::E_MCP_RESULT_UNKNOWN;

/// 连接状态查询（设置页信任状态列；headless 测试用）。
pub fn state_of(
    app: &AppState,
    scope: McpScope,
    name: &str,
) -> Option<(ConnState, Option<String>)> {
    let (_, key, _) = resolve_server(app, scope, name).ok()?;
    app.mcp.state_of(&key)
}

/// 测试辅助：trust ask 槽表类型（AppState 构造面）。
pub type TrustAskMap = std::collections::HashMap<String, PendingTrustAsk>;
