//! 更新器支撑命令。
//!
//! 三命令分工：
//! - `update_status`：更新入口的**禁用门**——tauri.conf.json 的
//!   `plugins.updater.pubkey` 是构建装配面（HITL 提供密钥后才实配），不是
//!   用户配置（AppConfig），所以不复用 get_config 而单独查询；pubkey 为
//!   占位空串期间前端把「检查更新」入口整体禁用（骨架
//!   先行，secrets 未配时以配置开关 + 构建面双门兜底，不为占位值放行）。
//! - `update_precheck`：安装/重启的**运行门**（2026-10-29 用户决策
//!   ——运行时可检查下载，安装重启前须无活跃任务/待审批请求、待发队列须
//!   明确暂停）。后端权威面判定（RunManager 注册表 + 应用级询问槽 + 磁盘
//!   全量队列扫描），`reasons` 为人话阻止原因清单供前端直接展示。检查与
//!   下载不受本门限制；安装前与重启前均须重新调用。
//! - `pause_followup_queue`：用户**主动暂停**待发队列（更新门配套
//!   ——「有待发队列时，用户须先明确暂停并持久保留队列才能更新」；暂停
//!   写 queue.json 持久化，更新重启后仍等待用户明确继续）。
//!
//! 更新本体（check/downloadAndInstall/relaunch）不经本仓自定义命令：前端
//! 直用官方 `@tauri-apps/plugin-updater` / `@tauri-apps/plugin-process` JS
//! API（插件命令经 capabilities 最小权限放行），插件在 lib.rs 注册。

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::host::AppState;
use crate::host::IpcError;

/// `update_status` 返回契约。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatusResult {
    /// updater 公钥是否已实配（非占位空串）：false = 前端更新入口禁用
    ///（HITL 密钥未配，pubkey 占位期间 updater 校验必败，不能放行入口）。
    pub pubkey_configured: bool,
    /// 配置的更新源 endpoint（诊断展示面；恒来自 tauri.conf.json 装配）。
    pub endpoints: Vec<String>,
}

/// 从 tauri 装配配置（tauri.conf.json 的 plugins.updater 原始 JSON）提取
/// pubkey 与 endpoints（core 函数收提取好的字段，headless 可测）。
pub(crate) fn update_status_core(
    pubkey: Option<&str>,
    endpoints: Vec<String>,
) -> UpdateStatusResult {
    UpdateStatusResult {
        pubkey_configured: pubkey.is_some_and(|k| !k.trim().is_empty()),
        endpoints,
    }
}

/// `update_status` 命令：读 tauri 装配配置（tauri.conf.json）
/// 的 plugins.updater 原始 JSON——`AppHandle::config()` 是 tauri 构建期生成
/// 的只读装配面，运行时稳定可查。插件配置缺失（未写 plugins.updater）= 全
/// 空结果（pubkeyConfigured=false，入口禁用兜底）。
#[tauri::command]
pub(crate) async fn update_status(app: tauri::AppHandle) -> Result<UpdateStatusResult, IpcError> {
    let plugin = app
        .config()
        .plugins
        .0
        .get("updater")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(update_status_core(
        plugin.get("pubkey").and_then(|v| v.as_str()),
        plugin
            .get("endpoints")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    ))
}

/// `update_precheck` 返回契约。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct UpdatePrecheckResult {
    /// 是否阻止安装与重启（任一阻止原因命中即 true）。
    pub blocked: bool,
    /// 人话阻止原因清单（前端直接逐行展示；空 = 放行）。
    pub reasons: Vec<String>,
    /// 活跃 run 数（含审批挂起中的 run）。
    pub running_count: usize,
    /// 应用级未决请求数（MCP 启动信任 / 技能项目信任等不属任何 run 的询问）。
    pub pending_ask_count: usize,
    /// 未暂停的待发队列数（磁盘全量扫描，含从未打开过界面的会话）。
    pub running_queue_count: usize,
    /// 已暂停的待发队列数（更新重启后仍保持暂停等待用户明确继续）。
    pub paused_queue_count: usize,
}

/// `update_precheck` 判定主体（headless 可测——纯读面，无系统
/// 副作用）。判定口径：
/// - 活跃 run（任一会话，审批挂起中算活跃）→ 阻止；
/// - 应用级未决询问（app_asks 槽非空）→ 阻止；
/// - 未暂停的待发队列 → 阻止（已暂停队列不阻止——持久保留在 queue.json，
///   更新重启后仍等待用户明确继续）。
pub(crate) fn update_precheck_core(
    running_count: usize,
    pending_ask_count: usize,
    queues: &[(String, crate::host::followup::QueueSnapshot)],
) -> UpdatePrecheckResult {
    let running_queue_count = queues
        .iter()
        .filter(|(_, q)| !q.paused && q.size > 0)
        .count();
    let paused_queue_count = queues.iter().filter(|(_, q)| q.paused).count();
    let mut reasons = Vec::new();
    if running_count > 0 {
        reasons.push(format!(
            "有 {running_count} 个会话的任务正在运行（或等待审批），安装重启会中断运行"
        ));
    }
    if pending_ask_count > 0 {
        reasons.push(format!(
            "有 {pending_ask_count} 个待确认的应用级请求（MCP/技能信任等），请先处理后重试"
        ));
    }
    if running_queue_count > 0 {
        reasons.push(format!(
            "有 {running_queue_count} 个会话的待发队列未暂停，请先在队列条上点「暂停」并保留队列后再更新"
        ));
    }
    UpdatePrecheckResult {
        blocked: !reasons.is_empty(),
        reasons,
        running_count,
        pending_ask_count,
        running_queue_count,
        paused_queue_count,
    }
}

/// `update_precheck` 命令：后端权威面实时判定（安装前与重启前
/// 均须重新调用——下载完成后新出现的活跃 run 仍能阻止安装）。
#[tauri::command]
pub(crate) async fn update_precheck(
    app: State<'_, AppState>,
) -> Result<UpdatePrecheckResult, IpcError> {
    let pending_ask_count = app
        .app_asks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .len();
    Ok(update_precheck_core(
        app.runs.len(),
        pending_ask_count,
        &app.followups.all_snapshots(),
    ))
}

/// `pause_followup_queue` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PauseFollowupQueueArgs {
    pub session_id: String,
}

/// `pause_followup_queue` 返回契约：变更后队列快照（size =
/// 待发条数；paused 恒 true）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PauseFollowupQueueResult {
    pub size: usize,
    pub paused: bool,
}

/// `pause_followup_queue` 主体：置暂停态并持久化（queue.json
/// 即权威，FollowUpManager.set_paused 原子写）。公开给集成测试。空 sessionId
/// 拒绝（E_INVALID_ARG，与 resume 同款守门）。
pub(crate) fn pause_followup_queue_core(
    app: &AppState,
    session_id: &str,
) -> Result<PauseFollowupQueueResult, IpcError> {
    if session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    let snapshot = app.followups.set_paused(session_id, true);
    Ok(PauseFollowupQueueResult {
        size: snapshot.size,
        paused: snapshot.paused,
    })
}

/// `pause_followup_queue` 命令：用户主动暂停待发队列（更新门
/// 配套）。前端收到 run:queue 事件同款形状，队列条与暂停横幅状态复用
/// queuePaused 桶接缝。
#[tauri::command]
pub(crate) async fn pause_followup_queue(
    app: State<'_, AppState>,
    session_id: String,
) -> Result<PauseFollowupQueueResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let _args = PauseFollowupQueueArgs {
        session_id: session_id.clone(),
    };
    pause_followup_queue_core(&app, &session_id)
}
