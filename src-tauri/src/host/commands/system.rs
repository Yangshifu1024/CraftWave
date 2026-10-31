//! 系统域命令：退出确认回执；通知授权态查询与
//! 系统设置深链。
//!
//! 退出拦截的收口：托盘「退出」在存在活跃 run 时挂起并广播
//! `app:exit_requested`，前端弹确认框后调 `resolve_exit_request`——
//! `accepted=true` 放行真正退出，`false` 取消退出回到托盘常驻。
//! 补超时兜底（`shell::EXIT_CONFIRM_TIMEOUT`）：前端无响应时
//! 到点自动放行，用户退出意图优先，退出流程不因前端挂死而悬死。
//!
//! 通知引导：macOS 通知授权被拒/未决时设置页给引导——
//! `notification_permission_status` 返回真实授权态（插件自带的
//! permission_state 桌面端恒 Granted，不可用），`open_notification_settings`
//! 打开系统设置的通知面板。
//!
//! 数据目录入口：`open_dir` 以固定白名单
//! （数据目录 / 日志目录 / 已注册项目的项目数据目录）校验入参路径
//! 后交系统打开——排障第一入口（查看 config.json、日志、project.json）。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::State;

use craftwave_engine::app_state::AppState;
use craftwave_engine::util::errors::IpcError;
use craftwave_engine::safety::safe_join::PathGuard;
use craftwave_engine::util::errors::{E_INTERNAL, E_INVALID_ARG, E_IO, E_NOT_FOUND};

/// `resolve_exit_request` 参数契约（`{ accepted: boolean }`）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ResolveExitRequestArgs {
    /// true = 确认退出（结束活跃 run 并退出）；false = 取消退出。
    pub accepted: bool,
}

/// 退出确认回执。无挂起的退出请求时调用为无操作。
#[tauri::command]
pub(crate) async fn resolve_exit_request(
    app: State<'_, AppState>,
    handle: tauri::AppHandle,
    accepted: bool,
) -> Result<(), IpcError> {
    // 契约命名单一事实源：文档契约结构占位
    let _args = ResolveExitRequestArgs { accepted };
    // 解除挂起（超时兜底定时任务看到已解除即 no-op），再按回执放行
    let was_pending = {
        let mut confirm = app
            .exit_confirm
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let was = confirm.is_pending();
        confirm.resolve(accepted);
        was
    };
    if accepted && was_pending {
        // 活跃 run 由进程退出自然终止（取消传播全链在内核/运行时收口；
        // 数据安全由 append-only 崩溃恢复语义兜底，重启标「已中断」）
        handle.exit(0);
    }
    Ok(())
}

/// `notification_permission_status` 返回契约。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct NotificationPermissionStatus {
    /// 授权态：granted（可送达）/ denied（被拒）/ prompt（未决定）/
    /// unknown（查不出，如 dev 裸进程无 bundle id）。
    pub status: String,
    /// 平台是否有「打开系统设置」入口（macOS/Windows 有；Linux 无统一入口，
    /// 前端据此决定是否渲染引导按钮）。
    pub has_settings_entry: bool,
}

/// 通知授权态查询：macOS 实查 UNUserNotificationCenter；
/// Windows/Linux 桌面通知无授权门，恒 granted。
#[tauri::command]
pub(crate) async fn notification_permission_status(
    app: tauri::AppHandle,
) -> Result<NotificationPermissionStatus, IpcError> {
    #[cfg(target_os = "macos")]
    {
        // 发起查询须在主线程（UNUserNotificationCenter 约束）；结果经
        // channel 回传，完成回调在系统队列（毫秒级）——阻塞等待收在
        // spawn_blocking，不占 async worker；2s 等不到按 unknown 口径。
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = app.run_on_main_thread(move || {
            crate::host::notify::query_macos_authorization(tx);
        });
        let status = tauri::async_runtime::spawn_blocking(move || {
            rx.recv_timeout(std::time::Duration::from_secs(2))
                .unwrap_or(crate::host::notify::AUTH_UNKNOWN)
        })
        .await
        .map_err(|e| IpcError::new(E_INTERNAL, format!("授权态查询失败：{e}")))?;
        Ok(NotificationPermissionStatus {
            status: status.to_owned(),
            has_settings_entry: true,
        })
    }
    #[cfg(windows)]
    {
        let _ = app;
        Ok(NotificationPermissionStatus {
            status: "granted".to_owned(),
            has_settings_entry: true,
        })
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = app;
        Ok(NotificationPermissionStatus {
            status: "granted".to_owned(),
            has_settings_entry: false,
        })
    }
}

/// 打开系统通知设置（设置页引导动作）。macOS 深链直达「通知」面板；
/// Windows 走 ms-settings:；Linux 无统一入口，报错由前端兜底（正常不触达：
/// 前端只在 hasSettingsEntry=true 时渲染按钮）。spawn 后不等待设置应用退出。
#[tauri::command]
pub(crate) fn open_notification_settings() -> Result<(), IpcError> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.notifications")
            .spawn()
            .map(|_| ())
            .map_err(|e| IpcError::new(E_INTERNAL, format!("打开系统设置失败：{e}")))
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：不闪控制台窗口
        std::process::Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:notifications"])
            .creation_flags(0x0800_0000)
            .spawn()
            .map(|_| ())
            .map_err(|e| IpcError::new(E_INTERNAL, format!("打开系统设置失败：{e}")))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Err(IpcError::new(E_INTERNAL, "该平台没有统一的通知设置入口"))
    }
}

/// `open_dir` 参数契约（`{ path: string }`）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct OpenDirArgs {
    /// 要打开的目录绝对路径（必须落在白名单内，见 [`open_dir_roots`]）。
    pub path: String,
}

/// `open_dir` 白名单根集合（目录布局）：数据目录 `~/.craftwave/`、
/// 日志目录 `~/.craftwave/logs/`、各注册项目的项目数据目录
/// `<项目主目录>/.craftwave/`（save_project 注册即建，必存在）。
/// 日志目录应用启动不预建，此处幂等补建——白名单校验（PathGuard）要求
/// 根真实存在，且"打开日志目录"按钮的语义就是目录没建也能点开。
fn open_dir_roots(app: &AppState) -> Result<Vec<PathBuf>, IpcError> {
    let data_root = app.store.root().to_path_buf();
    let logs = data_root.join("logs");
    let _ = std::fs::create_dir_all(&logs);
    let mut roots = vec![data_root, logs];
    let projects = app
        .projects
        .list()
        .map_err(|e| IpcError::internal(format!("读取项目注册表失败：{e:?}")))?;
    for project in projects {
        roots.push(PathBuf::from(project.home_dir).join(".craftwave"));
    }
    // PathGuard::new 要求根可 canonicalize（真实存在）；项目主目录被用户
    // 手动删除等残缺场景下该根自动退出白名单（校验报越界，不 panic）。
    Ok(roots.into_iter().filter(|r| r.exists()).collect())
}

/// ToolError → IpcError（code 原样透传：E_PATH_OUTSIDE_ROOTS / E_INVALID_ARG）。
fn tool_error_to_ipc(err: craftwave_engine::util::errors::ToolError) -> IpcError {
    IpcError::new(err.code, err.message)
}

/// 展开 "~" 前缀为用户主目录（unix HOME / windows USERPROFILE）。前端不知
/// 道数据目录的绝对路径（固定在 `~/.craftwave/`），入口按钮以 `~/`
/// 语义路径传参，后端展开后照常走白名单校验；无 "~" 前缀（如项目数据目录
/// 的绝对路径）原样返回。环境变量缺失时原样返回，交白名单校验拒绝。
pub(crate) fn expand_home_prefix(path: &str) -> String {
    let Some(rest) = path.strip_prefix('~') else {
        return path.to_owned();
    };
    // 仅支持 "~" 与 "~/…"；"~user" 形态不支持，原样返回交白名单拒绝
    if !rest.is_empty() && !rest.starts_with('/') && !rest.starts_with('\\') {
        return path.to_owned();
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_default();
    format!("{home}{rest}")
}

/// `open_dir` 校验主体（headless 可测，无系统副作用）：白名单校验 → 目录
/// 存在性校验，返回解析后的目录路径。越界 E_PATH_OUTSIDE_ROOTS；路径不
/// 存在/不是目录 E_NOT_FOUND。表驱动测试见 system_test.rs。
pub(crate) fn resolve_open_dir(app: &AppState, path: &str) -> Result<PathBuf, IpcError> {
    if path.trim().is_empty() {
        return Err(IpcError::new(E_INVALID_ARG, "path 不能为空"));
    }
    // "~" 语义路径展开为绝对路径后再校验（前端不感知 home 位置）
    let expanded = expand_home_prefix(path);
    let guard =
        PathGuard::new(open_dir_roots(app)?).map_err(|e| IpcError::new(e.code, e.message))?;
    let resolved = guard.resolve(&expanded).map_err(tool_error_to_ipc)?;
    // 命令名义是 open_dir：只放行目录，文件路径不开放（防误传文件走默认应用打开）
    if !resolved.is_dir() {
        return Err(IpcError::new(
            E_NOT_FOUND,
            format!("目标不是目录：{}", resolved.display()),
        ));
    }
    Ok(resolved)
}

/// `open_dir` 主体：校验 → 交系统打开。OS 打开失败归 E_IO。
pub(crate) fn open_dir_core(app: &AppState, path: &str) -> Result<(), IpcError> {
    let resolved = resolve_open_dir(app, path)?;
    // 脱离进程打开（macOS /usr/bin/open 即交即返；Windows/Linux 不等待
    // 文件管理器退出），失败归 E_IO
    open::that_detached(&resolved)
        .map_err(|e| IpcError::new(E_IO, format!("系统打开目录失败：{e}")))
}

/// 系统打开目录：设置页"打开数据/日志/项目
/// 数据目录"三入口的统一承载。
#[tauri::command]
pub(crate) async fn open_dir(app: State<'_, AppState>, path: String) -> Result<(), IpcError> {
    // 契约命名单一事实源：文档契约结构占位
    let _args = OpenDirArgs { path: path.clone() };
    open_dir_core(&app, &path)
}
