//! MCP 管理域 API：双作用域配置、连接池操作、草稿试连、启动信任（TOFU）
//! 与快照查询。
//!
//! 平移来源：`host/commands/mcp_cmds.rs`（全部框架无关 core）。TOFU 询问
//! 不属任何 run（管理页连接触发），挂在 `AppState.app_asks` 应用级槽表
//! （`backend::ask::AppAskSlot`）；`resolve_ask`（`crate::api::run`）在
//! run 槽未命中时落到这里。应答经既有
//! [`crate::backend::ask::AskAnswer::Approval`]（optionId =
//! allow_once/remember/deny），协议面不新增应答形态。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::app_state::AppState;
use crate::backend::ask::{AskAnswer, AskOption, AskPayload, McpTransportKind};
use crate::mcp::config::{McpConfigFile, McpConfigStore, McpScope, McpServerConfig};
use crate::mcp::pool::{ConnState, open_for_test, list_tools};
use crate::mcp::tools::ConnKey;
use crate::mcp::trust::{LaunchDecision, TrustSubject, launch_fingerprint};
use crate::util::errors::{E_STORAGE_IO, IpcError};
use crate::util::events::keys;
// 未连接/信任拒绝的错误码（backend run_loop 注册表；旧壳同源引用）。
use crate::backend::run_loop::E_CANCELLED;

/// MCP TOFU 询问槽（= `backend::ask::AppAskSlot` 别名，沿用旧名）。
pub type PendingTrustAsk = crate::backend::ask::AppAskSlot;

/// mcpTrust 三选项（options id 与旧前端逐字一致）。
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

// ---- 命令契约类型（与旧壳一字不差） ----

/// `mcp_list_config` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpListConfigArgs {}

/// `mcp_list_config` 返回契约：双作用域原样配置（未知字段保真）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpListConfigResult {
    /// 全局作用域文件内容（缺失 = 空配置）。
    pub global: McpConfigFile,
    /// 项目作用域文件内容（无项目上下文 = 空配置）。
    pub project: McpConfigFile,
    /// 项目作用域是否可编辑（当前活跃会话有归属项目）。
    pub project_available: bool,
}

/// `mcp_save_config` 参数契约：整份保存一个作用域。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpSaveConfigArgs {
    /// 目标作用域。
    pub scope: McpScope,
    /// 整份配置（未知字段随 flatten round-trip）。
    pub config: McpConfigFile,
}

/// `mcp_connect` / `mcp_disconnect` / `mcp_reconnect` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpServerRefArgs {
    /// 目标作用域。
    pub scope: McpScope,
    /// 服务器配置名。
    pub name: String,
}

/// `mcp_test` 参数契约：草稿条目（不经保存直接试连）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpTestArgs {
    /// 草稿服务器配置（未知字段允许，试连不校验它们）。
    pub draft: McpServerConfig,
}

/// `mcp_test` 的单工具行。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpTestTool {
    /// 工具名（原始名，未做归一）。
    pub name: String,
    /// 工具描述。
    pub description: String,
}

/// `mcp_test` 返回契约：ok + 工具清单或错误（总预算 60s）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpTestResult {
    /// 是否连通（initialize + tools/list 完成）。
    pub ok: bool,
    /// 发现的工具清单。
    pub tools: Vec<McpTestTool>,
    /// 失败原因（ok=false 时）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<String>,
}

/// `mcp_set_launch_trust` 参数契约：管理页直接写/撤销信任。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpSetLaunchTrustArgs {
    /// 启动指纹（完整 hex）。
    pub fingerprint: String,
    /// 决策：remember（记住此版本）/ revoke（撤销）。
    pub decision: LaunchDecision,
}

/// `mcp_snapshot` 参数契约（空参）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpSnapshotArgs {}

/// `mcp_snapshot` 的单服务器行（逐字段 + 管理页数据面扩展）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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
    #[cfg_attr(feature = "ts", ts(optional))]
    pub detail: Option<String>,
    /// 启动指纹前 16 位（信任列展示；配置条目解析即算）。
    pub fingerprint: String,
    /// 信任状态（"trusted"|"pending"|"revoked"）。
    pub trust_status: String,
}

/// `mcp_snapshot` 返回契约。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct McpSnapshotResult {
    /// 池内服务器状态行。
    pub servers: Vec<McpSnapshotServer>,
}

// ---- AppState 侧的共享解析 ----

/// 当前项目上下文（project 作用域的解析面）：活跃会话归属的项目。
/// 收敛点：设置页无会话参数，project 作用域锚定「当前活跃
/// 会话的项目」；无活跃会话/会话无项目 → project 不可用。
/// 构造作用域配置存储（全局路径 = 数据目录根；项目路径 = 当前项目）。
fn config_store_of(app: &AppState) -> McpConfigStore {
    let project_home =
        crate::knowledge::instructions::project_context_of(app).map(|(_, home)| home);
    McpConfigStore::new(app.mcp.data_dir(), project_home.as_deref())
}

/// 解析一个服务器条目：返回（配置, 连接键, 指纹）。信任计算读取配置文件
/// 原始字节（sha256(config) = 文件整体）。
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
        IpcError::new(E_STORAGE_IO, format!("解析 mcp.json 失败：{e}"))
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
    let project_id =
        crate::knowledge::instructions::project_context_of(app).map(|(id, _)| id);
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

/// TOFU 信任询问：无信任记录 → 挂 ask 槽 + ask:opened → 等应答。
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
            .app_asks
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
        keys::ASK_OPENED,
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
    app.app_asks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&ask_id);
    app.events.emit(
        keys::ASK_CLOSED,
        serde_json::json!({ "askId": ask_id, "answered": true }),
    );
    match option_id.as_str() {
        "allow_once" | "remember" => {
            if option_id == "remember" {
                app.mcp
                    .trust
                    .decide(fingerprint, LaunchDecision::Remember, crate::storage::trust::now_ms())
                    .map_err(|e| IpcError::new(E_STORAGE_IO, e))?;
            }
            Ok(Some(option_id))
        }
        _ => Ok(None),
    }
}

/// TOFU 询问槽的应答投递（`run::resolve_ask` 的 run 槽未命中兜底）：
/// 命中即取走槽位（oneshot 发送端不可克隆；取走后等待方收到应答并自清
/// ask:closed——见 `ensure_launch_trust` 的单一出口）。
pub fn resolve_trust_ask(app: &AppState, ask_id: &str, answer: AskAnswer) -> bool {
    app.resolve_app_ask(ask_id, answer)
}

// ---- 八命令 ----

/// `mcp_list_config`：双作用域原样配置。
pub fn mcp_list_config(app: &AppState, _args: McpListConfigArgs) -> Result<McpListConfigResult, IpcError> {
    let store = config_store_of(app);
    let global = store.load(McpScope::Global).map_err(IpcError::invalid_arg)?;
    let project = if store.project_path.is_some() {
        store.load(McpScope::Project).map_err(IpcError::invalid_arg)?
    } else {
        McpConfigFile::default()
    };
    Ok(McpListConfigResult {
        global,
        project,
        project_available: store.project_path.is_some(),
    })
}

/// `mcp_save_config`：整份原子保存（未知字段随文件保真）。
/// 保存前校验全部条目可解析（防保存一份坏配置导致后续加载失败）。
pub fn mcp_save_config(app: &AppState, args: McpSaveConfigArgs) -> Result<(), IpcError> {
    let (_, bad) = args.config.parse_servers();
    if !bad.is_empty() {
        return Err(IpcError::invalid_arg(format!(
            "配置条目解析失败：{}",
            bad.join("；")
        )));
    }
    let store = config_store_of(app);
    store.save(args.scope, &args.config).map_err(IpcError::invalid_arg)
}

/// `mcp_connect`：信任核验（TOFU）→ 发起连接（后台任务，
/// 状态经 `mcp:status` 事件可见）。
pub async fn mcp_connect(app: &AppState, args: McpServerRefArgs) -> Result<(), IpcError> {
    let (config, key, fingerprint) = resolve_server(app, args.scope, &args.name)?;
    if !config.enabled {
        return Err(IpcError::invalid_arg(format!(
            "MCP 服务器 {} 已停用（enabled=false）",
            args.name
        )));
    }
    let host = app.mcp.clone();
    match ensure_launch_trust(app, &args.name, &config, &fingerprint).await? {
        Some(_) => {
            // 信任达成 → 连接（后台任务；TOFU/连接失败都经 mcp:status 可见）。
            host.connect(key, config, fingerprint);
            Ok(())
        }
        None => Err(IpcError::new(
            E_CANCELLED,
            format!("MCP 服务器 {} 的启动信任被拒绝，未连接", args.name),
        )),
    }
}

/// `mcp_disconnect`：断开指定服务器（池无条目 = 无操作，幂等）。
pub fn mcp_disconnect(app: &AppState, args: McpServerRefArgs) -> Result<(), IpcError> {
    let (_, key, _) = resolve_server(app, args.scope, &args.name)?;
    app.mcp.disconnect(&key);
    Ok(())
}

/// `mcp_reconnect`：断开重连（指纹变更会经 TOFU 重问——
/// connect 内的 ensure_launch_trust 兜底）。
pub async fn mcp_reconnect(app: &AppState, args: McpServerRefArgs) -> Result<(), IpcError> {
    let (config, key, fingerprint) = resolve_server(app, args.scope, &args.name)?;
    if !config.enabled {
        return Err(IpcError::invalid_arg(format!(
            "MCP 服务器 {} 已停用（enabled=false）",
            args.name
        )));
    }
    match ensure_launch_trust(app, &args.name, &config, &fingerprint).await? {
        Some(_) => {
            app.mcp.reconnect(&key, config, fingerprint);
            Ok(())
        }
        None => Err(IpcError::new(
            E_CANCELLED,
            format!("MCP 服务器 {} 的启动信任被拒绝，未重连", args.name),
        )),
    }
}

/// `mcp_test`：草稿试连（initialize + tools/list），总预算
/// 60s，不进池、不写信任（试连 = 只读探测）。
pub async fn mcp_test(args: McpTestArgs) -> Result<McpTestResult, IpcError> {
    let budget = tokio::time::Duration::from_secs(60);
    match tokio::time::timeout(budget, test_once(&args.draft)).await {
        Ok(Ok(tools)) => Ok(McpTestResult {
            ok: true,
            tools,
            error: None,
        }),
        Ok(Err(e)) => Ok(McpTestResult {
            ok: false,
            tools: Vec::new(),
            error: Some(e),
        }),
        Err(_) => Ok(McpTestResult {
            ok: false,
            tools: Vec::new(),
            error: Some("测试连接超时（总预算 60 秒）".to_owned()),
        }),
    }
}

/// 一次草稿试连：临时连接 → tools/list → 断开（不进池、不写信任）。
async fn test_once(config: &McpServerConfig) -> Result<Vec<McpTestTool>, String> {
    // 试连 = 只读探测：不经信任门；stdio 同样会启动子进程——沙箱边界与
    // 正式连接一致（进程组 + 收割），临时宿主只承载清单变更回调。
    let dead_host = Arc::new(crate::mcp::pool::McpHost::new(
        std::env::temp_dir().join("craftwave-mcp-test"),
        Arc::new(crate::util::events::RecordingEventSink::new()),
    ));
    let connection = open_for_test(config, dead_host).await?;
    let tools = list_tools(&connection.peer, tokio::time::Duration::from_secs(30)).await?;
    drop(connection);
    Ok(tools
        .into_iter()
        .map(|(name, description, _, _)| McpTestTool { name, description })
        .collect())
}

/// `mcp_set_launch_trust`：决策写 `trust/mcp.json`。revoke 时连带撤销该
/// 服务器全部工具许可（服务器身份不可信 → 工具许可不得延续）。
pub fn mcp_set_launch_trust(app: &AppState, args: McpSetLaunchTrustArgs) -> Result<(), IpcError> {
    app.mcp
        .trust
        .decide(&args.fingerprint, args.decision, crate::storage::trust::now_ms())
        .map_err(IpcError::invalid_arg)?;
    if args.decision == LaunchDecision::Revoke {
        app.mcp
            .licenses
            .revoke_server(&args.fingerprint)
            .map_err(IpcError::invalid_arg)?;
    }
    Ok(())
}

/// `mcp_snapshot`：配置 × 连接池 × 信任状态快照（管理页完整数据面）。
/// 配置里有但从未连接的服务器同样成行——state="stopped"、
/// trustStatus 按 `trust/mcp.json` 实况；配置已删除的服务器不成行，其池内
/// 残留条目由断线监控/驱逐自然收口。配置读取失败按参数错误上抛——
/// 管理页据错误提示用户修配置，静默空表会掩盖坏文件。
pub fn mcp_snapshot(app: &AppState, _args: McpSnapshotArgs) -> Result<McpSnapshotResult, IpcError> {
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
