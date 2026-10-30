//! config 域命令（§3.3 #6/#7）：get_config / save_config。
//!
//! 薄壳：参数即类型校验（serde 反序列化）+ 转调 providers::service。

use serde::Deserialize;
use tauri::State;

use crate::host::AppState;
use crate::providers::service;
use crate::providers::types::AppConfig;
use crate::util::errors::IpcError;

/// `save_config` 参数契约（§3.3 #7；文档命名锚点，命令形参按字段平铺展开）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SaveConfigArgs {
    pub config: AppConfig,
}

/// §3.3 #6：读取当前配置（协调器缓存优先）。
#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<AppConfig, IpcError> {
    service::get_config(&state.config)
}

/// §3.3 #7：整份配置经协调器归一化原子落盘，返回归一化结果。
#[tauri::command]
pub async fn save_config(
    state: State<'_, AppState>,
    config: AppConfig,
) -> Result<AppConfig, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = SaveConfigArgs { config };
    let normalized = service::save_config(&state.config, args.config)?;
    // 子代理全局并发上限热应用（10 号票 Q13：保存边界即生效——已启动任务
    // 继续不取消，降低暂停补入，提高立即放行排队任务）
    state.apply_subagent_limit();
    // 代理热生效（19 号票 FR-02-10）：重建 HTTP 客户端工厂缓存——进行中
    // run 不切换（客户端按 run 构建，§11.1），下一次 start_chat 用新代理
    state.apply_proxy_config()?;
    Ok(normalized)
}
