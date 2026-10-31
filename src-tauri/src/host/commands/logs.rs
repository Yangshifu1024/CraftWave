//! 日志命令：
//! `read_session_log`（会话日志尾部拉取）/ `list_log_files`（日志文件
//! 清单）/ `open_logs_dir`（系统打开日志目录）。
//!
//! 读取是纯运维诊断面（拉取式、无实时流）；文件布局与轮转见
//! `host/session_log.rs`（布局的实现落点）。

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::host::AppState;
use crate::host::IpcError;
use crate::util::errors::E_IO;

/// `read_session_log` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ReadSessionLogArgs {
    pub session_id: String,
    /// 尾行数（缺省 500、上限 5000——越界 clamp 到边界）。
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub tail: Option<u32>,
}

/// `read_session_log` 返回契约：尾部行清单（JSONL 原文行，
/// 前端解析渲染）。
#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ReadSessionLogResult {
    pub lines: Vec<String>,
}

/// tail 缺省值与上限（缺省 500、上限 5000）。
const TAIL_DEFAULT: u32 = 500;
const TAIL_MAX: u32 = 5000;

/// `read_session_log` 主体（headless 可测）。
pub(crate) fn read_session_log_core(
    app: &AppState,
    session_id: &str,
    tail: Option<u32>,
) -> ReadSessionLogResult {
    // tail clamp：缺省 500、上限 5000（0 视为缺省——拉 0 行无意义）
    let clamped = tail
        .filter(|t| *t > 0)
        .unwrap_or(TAIL_DEFAULT)
        .min(TAIL_MAX);
    ReadSessionLogResult {
        lines: app.session_log.read_tail(session_id, clamped as usize),
    }
}

/// 读取会话日志尾部：文件不存在 → 空清单不报错（幂等查询
/// 语义——verbose 从未开过的会话无日志文件属正常态）。
#[tauri::command]
pub(crate) async fn read_session_log(
    app: State<'_, AppState>,
    session_id: String,
    tail: Option<u32>,
) -> Result<ReadSessionLogResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = ReadSessionLogArgs { session_id, tail };
    Ok(read_session_log_core(&app, &args.session_id, args.tail))
}

/// `list_log_files`：logs/ 下全部日志文件相对名（app.log* +
/// sessions/*，相对 `~/.craftwave/logs/`；spill 子目录不在清单——
/// 布局定义两项）。目录不存在 → 空清单不报错。
#[tauri::command]
pub(crate) async fn list_log_files(app: State<'_, AppState>) -> Result<Vec<String>, IpcError> {
    Ok(app.session_log.list_files())
}

/// `open_logs_dir`：系统文件管理器打开 `~/.craftwave/logs/`
/// （复用 `open_dir` 的系统打开通道与白名单口径——日志目录本身
/// 就在 open_dir 白名单内，本命令固定打开该目录、免路径传参）。目录
/// 不存在先创建（首次使用即有）。
#[tauri::command]
pub(crate) async fn open_logs_dir(app: State<'_, AppState>) -> Result<(), IpcError> {
    open_logs_dir_inner(&app)
}

/// open_logs_dir 主体（headless 可测：目录创建面与系统打开分离——headless
/// 环境无文件管理器，打开失败属环境面）。
pub(crate) fn open_logs_dir_inner(app: &AppState) -> Result<(), IpcError> {
    let dir = app.session_log.root().to_path_buf();
    std::fs::create_dir_all(&dir)
        .map_err(|e| IpcError::new(E_IO, format!("创建日志目录失败：{e}")))?;
    // 脱离进程打开（同 open_dir_core 口径：macOS /usr/bin/open 即交即返）
    open::that_detached(&dir).map_err(|e| IpcError::new(E_IO, format!("系统打开目录失败：{e}")))
}

#[cfg(test)]
#[path = "logs_test.rs"]
mod logs_test;
