//! 配额查询命令（§3.3 #53 `quota_snapshots`，FR-02-15，21 号票）。
//!
//! 薄壳：读当前配置 → 取代理工厂客户端 → 转调 `providers::quota`（领域面）。
//! 失败静默降级的纪律在领域面落实（一家失败不影响其余、不 emit 事件不弹
//! 提示），命令层不做二次错误包装——整命令只在配置读取失败时 reject。

use tauri::State;

use crate::host::AppState;
use crate::providers::quota::{self, QuotaSnapshot};
use crate::util::errors::IpcError;

/// §3.3 #53：全部供应商的额度快照（六态行，排序 = 配置序、可查询类在前
/// unsupported 殿后）。调用时并发实时拉取（每家独立 10s 超时，一家失败不
/// 影响其余）；批次末合并进 `quota.json` 快照（离线兜底面）。5 分钟周期
/// 刷新由前端配额页可见门控驱动，不建后端定时器（§11.5）。
#[tauri::command]
pub(crate) async fn quota_snapshots(
    app: State<'_, AppState>,
) -> Result<Vec<QuotaSnapshot>, IpcError> {
    let config = app.config.load()?.as_ref().clone();
    let client = app.http.client();
    Ok(quota::snapshots(&config, app.store.root(), &client).await)
}
