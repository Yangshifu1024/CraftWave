//! CraftWave 桌面端 Rust 侧（Tauri）crate 根：模块声明、IPC 命令注册与应用装配。
//!
//! 模块划分（§2.2）：host（Tauri 命令与窗口/托盘）、agent 与 backend（run 循环）、
//! providers（模型接入与配置）、tools（工具实现）、safety（路径围栏）、
//! storage（会话落盘）、spike（早期原型验证代码的保留存档）、util（最底层通用件）。
//!
//! 装配流程见 [`run`]：单实例插件最先注册 → 窗口关闭一律拦截为隐藏 →
//! setup 回调内建数据目录、事件汇、通知门与 AppState → 挂上 IPC 处理器 →
//! 进入事件循环。每一步的"为什么"在函数体内以行内注释解释。
//!
//! IPC 注册的核心设计（单一事实源防漂移）：`invoke_handler`（真正接收前端
//! invoke 调用的注册表）与 `ipc_manifest`（供前端比对契约的清单）由同一份
//! `ipc_commands!` 清单宏展开。单一事实源：同一份信息只在一处维护；
//! 漂移：两份各自手写的清单逐渐不一致。宏展开保证新增命令必然同时出现
//! 在两处，命令名与契约类型名不可能漂移（§12.3/NFR-12）。清单（manifest）：
//! 机器可读的契约文件，列出全部命令/帧/事件的名字与类型名，落盘
//! target/ipc-manifest.json 供前端扫描比对。

use tauri::{Manager, WebviewWindow};
use tauri_plugin_decoration::WebviewWindowExt;

// 业务模块声明（划分依据见本文件头部说明）
pub mod agent;
pub mod backend;
pub mod host;
pub mod providers;
pub mod safety;
pub mod spike;
pub mod storage;
pub mod tools;
pub mod util;

// 契约类型在 `ipc_commands!` 清单中被两处引用：非测试构建经 stringify!（仅取
// 类型名字符串），测试构建由 export_contract_ts 真实解析（ts-rs 导出）——
// 后者仅在 cfg(test) 存在，故非测试构建下编译器仍视为未用。
#[allow(unused_imports)]
use backend::frames::RunSnapshot;
#[allow(unused_imports)]
use host::commands::chat::{
    ApprovePlanArgs, AttachRunArgs, AttachRunResult, CancelRunArgs, CompactSessionArgs,
    CompactSessionResult, ResolveAskArgs, RunConfig, SessionPendingAsk, SessionPendingAskArgs,
    SessionRunSnapshotArgs, StartChatArgs, StartChatResult, approve_plan, attach_run, cancel_run,
    compact_session, resolve_ask, session_pending_ask, session_run_snapshot, start_chat,
};
#[allow(unused_imports)]
use host::commands::config::{SaveConfigArgs, get_config, save_config};
#[allow(unused_imports)]
use host::commands::project::{DeleteProjectArgs, SaveProjectArgs};
#[allow(unused_imports)]
use host::commands::provider::{
    DeleteProviderArgs, SaveProviderArgs, delete_provider, save_provider,
};
#[allow(unused_imports)]
use host::commands::session::{CreateSessionArgs, DeleteSessionArgs, RenameSessionArgs};
#[allow(unused_imports)]
use host::commands::session_tree::{
    ListSessionsArgs, LoadSessionTreeArgs, LoadToolOutcomesArgs, SaveSessionTreeArgs,
    SaveSessionTreeResult,
};
use host::commands::session_tree::{load_session_tree, save_session_tree};
#[allow(unused_imports)]
use host::commands::system::{NotificationPermissionStatus, OpenDirArgs, ResolveExitRequestArgs};
#[allow(unused_imports)]
use host::commands::ui_state::{GetUiStateArgs, SetUiStateArgs};
#[allow(unused_imports)]
use host::commands::workspace::{
    ReadWorkspaceFileArgs, ReadWorkspaceResult, SearchWorkspacePathsArgs, WorkspacePathHit,
};
#[allow(unused_imports)]
use providers::types::{AppConfig, ProviderConfig};
#[allow(unused_imports)]
use serde_json::Value;
#[allow(unused_imports)]
use std::collections::HashMap;
#[allow(unused_imports)]
use storage::ProjectMeta;
#[allow(unused_imports)]
use storage::SessionMeta;
#[allow(unused_imports)]
use storage::SessionTreeData;
#[allow(unused_imports)]
use storage::config_store::ConfigStore;

#[cfg(test)]
#[path = "ipc_manifest_test.rs"]
mod ipc_manifest_test;
#[cfg(debug_assertions)]
mod mock_chat;
#[cfg(test)]
#[path = "mock_chat_test.rs"]
mod mock_chat_test;

/// IPC 命令规格（§12.3 manifest 的 commands 元素；字段名沿用文档口径）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct IpcCommandSpec {
    pub name: String,
    pub args_type: String,
    pub ret_type: String,
}

/// IPC 命令注册单一事实源（§12.3/NFR-12 防漂移）：`invoke_handler` 与 `ipc_manifest`
/// 由同一份清单宏展开，命令名与契约类型名不可能漂移；dev-only 命令带 cfg 同源注册。
macro_rules! ipc_commands {
    ($($(#[$cfg:meta])* $path:path => args: $args:ty, ret: $ret:ty;)*) => {
        /// 注册全部 IPC 命令（与 manifest 同源）。
        pub fn invoke_handler() -> impl Fn(tauri::ipc::Invoke) -> bool {
            tauri::generate_handler![ $( $(#[$cfg])* $path , )* ]
        }

        /// IPC 契约清单：cargo test 落盘 target/ipc-manifest.json（§12.3），前端双向扫描。
        /// frames/events 键随对应模块实装时由各自清单填充，当前 P0 未实装先占位空集。
        pub fn ipc_manifest() -> Vec<IpcCommandSpec> {
            vec![ $(
                $(#[$cfg])*
                IpcCommandSpec {
                    name: command_name(stringify!($path)),
                    args_type: stringify!($args).to_string(),
                    ret_type: stringify!($ret).to_string(),
                },
            )* ]
        }

        /// 测试构建专用：按命令清单导出全部契约负载类型的 TS 声明（02 号票，
        /// C-2 定案）——名义类型经 ts-rs 连带依赖闭包写出 ui/src/ipc/generated/
        /// （生成物进 git，cargo build 不依赖生成步骤）；非名义类型（bool、
        /// Vec<_>、Option<_>、HashMap<_,_> 等，含空参命令的 `()`）无独立生成
        /// 文件，返回 CannotBeExported 占位，由调用方按名义名单甄别。
        /// push 而非 vec![]：cfg 属性只能挂在语句上（dev-only 命令按构建
        /// 配置同源进退）。
        #[cfg(test)]
        #[allow(clippy::vec_init_then_push)]
        pub fn export_contract_ts(
            cfg: &ts_rs::Config,
        ) -> Vec<(&'static str, Result<(), ts_rs::ExportError>)> {
            #[allow(unused_mut)]
            let mut results: Vec<(&'static str, Result<(), ts_rs::ExportError>)> = Vec::new();
            $(
                $(#[$cfg])*
                results.push((stringify!($args), <$args as ts_rs::TS>::export_all(cfg)));
                $(#[$cfg])*
                results.push((stringify!($ret), <$ret as ts_rs::TS>::export_all(cfg)));
            )*
            results
        }
    };
}

/// 命令路径 → 命令名（取末段；D14：命令名 snake_case）。
fn command_name(path: &str) -> String {
    path.rsplit("::").next().unwrap_or(path).to_string()
}

// IPC 命令清单本体：一行一个命令（可带 cfg 属性控制平台/构建）；新增或删除
// 命令只改这里，注册表与导出清单同时生效——这正是单一事实源的落点。
ipc_commands! {
    activate_custom_titlebar => args: (), ret: ();
    restore_native_titlebar => args: (), ret: ();
    save_session_tree => args: SaveSessionTreeArgs, ret: SaveSessionTreeResult;
    load_session_tree => args: LoadSessionTreeArgs, ret: SessionTreeData;
    host::commands::session_tree::list_sessions => args: ListSessionsArgs, ret: Vec<SessionMeta>;
    host::commands::session_tree::load_tool_outcomes => args: LoadToolOutcomesArgs, ret: HashMap<String, Value>;
    host::commands::project::list_projects => args: (), ret: Vec<ProjectMeta>;
    host::commands::project::save_project => args: SaveProjectArgs, ret: ProjectMeta;
    host::commands::project::delete_project => args: DeleteProjectArgs, ret: ();
    host::commands::session::create_session => args: CreateSessionArgs, ret: SessionMeta;
    host::commands::session::delete_session => args: DeleteSessionArgs, ret: ();
    host::commands::session::rename_session => args: RenameSessionArgs, ret: SessionMeta;
    host::commands::workspace::search_workspace_paths => args: SearchWorkspacePathsArgs, ret: Vec<WorkspacePathHit>;
    host::commands::workspace::read_workspace_file => args: ReadWorkspaceFileArgs, ret: ReadWorkspaceResult;
    host::commands::ui_state::get_ui_state => args: GetUiStateArgs, ret: Value;
    host::commands::ui_state::set_ui_state => args: SetUiStateArgs, ret: ();
    host::commands::config::get_config => args: (), ret: AppConfig;
    host::commands::config::save_config => args: SaveConfigArgs, ret: AppConfig;
    host::commands::provider::save_provider => args: SaveProviderArgs, ret: ProviderConfig;
    host::commands::provider::delete_provider => args: DeleteProviderArgs, ret: ();
    host::commands::chat::start_chat => args: StartChatArgs, ret: StartChatResult;
    host::commands::chat::cancel_run => args: CancelRunArgs, ret: bool;
    host::commands::chat::resolve_ask => args: ResolveAskArgs, ret: ();
    host::commands::chat::approve_plan => args: ApprovePlanArgs, ret: ();
    host::commands::chat::compact_session => args: CompactSessionArgs, ret: CompactSessionResult;
    host::commands::chat::attach_run => args: AttachRunArgs, ret: AttachRunResult;
    host::commands::chat::session_run_snapshot => args: SessionRunSnapshotArgs, ret: Option<RunSnapshot>;
    host::commands::chat::session_pending_ask => args: SessionPendingAskArgs, ret: Option<SessionPendingAsk>;
    host::commands::system::resolve_exit_request => args: ResolveExitRequestArgs, ret: ();
    host::commands::system::notification_permission_status => args: (), ret: NotificationPermissionStatus;
    host::commands::system::open_notification_settings => args: (), ret: ();
    host::commands::system::open_dir => args: OpenDirArgs, ret: ();
    #[cfg(debug_assertions)]
    mock_chat::mock_chat_run => args: (), ret: ();
    select_dir => args: (), ret: Option<String>;
}

/// 原生目录选择（§3.3 #5）：同步命令在主线程执行（macOS NSOpenPanel 的
/// 线程约束）；用户取消 → null。
#[tauri::command]
fn select_dir() -> Option<String> {
    rfd::FileDialog::new()
        .pick_folder()
        .map(|path| path.to_string_lossy().into_owned())
}

/// 激活覆盖式自绘标题栏；macOS 交通灯由自研模块摆到 ZCode 同款 (22, 23)
/// （decoration 插件的 inset_y 是「上下边距」且容器改动受裁剪上限约束，
/// 实测摆不出该位置——语义与绕开原因见 traffic_lights 模块注释）。
#[tauri::command]
async fn activate_custom_titlebar(window: WebviewWindow) -> Result<(), String> {
    window
        .activate_decoration()
        .await
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    host::commands::traffic_lights::activate(&window)?;
    Ok(())
}

/// 看门狗（watchdog：定时检查某操作是否卡死、卡死即走备用路径的机制）
/// 超时后的退化路径：放弃自绘标题栏，恢复系统原生标题栏。
#[tauri::command]
async fn restore_native_titlebar(window: WebviewWindow) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    host::commands::traffic_lights::deactivate();
    window.restore_decoration().await.map_err(|e| e.to_string())
}

/// Tauri 应用装配与启动入口。装配顺序即语义顺序：单实例 → 装饰/通知插件 →
/// 关闭拦截 → setup（数据目录、事件汇、通知门、AppState、托盘）→ IPC 处理器。
/// 各步的"为什么这么做"见函数体内的行内注释。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 单实例必须最先注册（FR-13-10）：二次启动不落 WebView，直接唤起既有主窗
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            host::shell::activate_main_window(app);
        }))
        .plugin(tauri_plugin_decoration::init())
        .plugin(tauri_plugin_notification::init())
        // 关闭到托盘（FR-13-06/07，§9.6）：close 请求一律拦截为隐藏，
        // 真正退出只经托盘菜单（存在活跃 run 时先经 app:exit_requested 确认）
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            // §7.1：数据目录 ~/.craftwave（config.json + histories + sessions）
            let home = app.path().home_dir()?;
            let data_dir = home.join(".craftwave");
            std::fs::create_dir_all(&data_dir)?;
            let handle = app.handle().clone();
            // 通知门三件套先独立构造（AppHandle/配置缓存/活跃会话标识），
            // 再与 AppState 共享同一份 Arc（配置改写后通知偏好即读即生效）
            let config = std::sync::Arc::new(ConfigStore::new(data_dir.join("config.json")));
            let active_session = std::sync::Arc::new(std::sync::Mutex::new(None));
            let notify = std::sync::Arc::new(host::notify::NotifyGate::new(
                handle.clone(),
                active_session.clone(),
                config.clone(),
            ));
            host::notify::install_click_routing(&notify);
            let mut state = host::AppState::new_in(
                data_dir,
                std::sync::Arc::new(host::TauriEventSink {
                    app: handle.clone(),
                    notify: notify.clone(),
                }),
            );
            state.config = config;
            state.active_session = active_session;
            // RunManager 以共享句柄单独 manage：start_chat/cancel_run/
            // resolve_ask/approve_plan 以 State<RunManager> 注入（Clone =
            // 同一注册表，§5.1）；漏 manage 会报 "state not managed for
            // field `runs`"（命令面 headless 测试不经 tauri，装配需此处钉死）。
            // SessionStore 同理共享 Arc：命令层与 run 落盘器（15 号票）同一锁表。
            app.manage(state.runs.clone());
            app.manage(state.store.clone());
            app.manage(state);
            host::shell::install_tray(&handle)?;
            Ok(())
        })
        .invoke_handler(invoke_handler())
        .run(tauri::generate_context!())
        .expect("CraftWave 启动失败");
}
