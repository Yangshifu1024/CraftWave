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
// git 集成域（26 号票，§2.2 域清单新增）：只读状态/差异/历史查询（面板
// 数据面）；git 写策略在 safety（分类表）与 backend（审批门开关），不在此域
pub mod git;
pub mod host;
// knowledge 知识域（09 号票，§2.2 域清单）：技能系统与技能命令
pub mod knowledge;
// MCP 客户端（08 号票，FR-08 域；与 host 同层的独立域模块，§2.2/§9.7）
pub mod mcp;
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
    CompactSessionResult, InjectMode, InjectRunMessageArgs, InjectRunMessageResult,
    RemoveFollowupArgs, RemoveFollowupResult, ResolveAskArgs, ResumeFollowupQueueArgs,
    ResumeFollowupQueueResult, RunConfig, SessionPendingAsk, SessionPendingAskArgs,
    SessionRunSnapshotArgs, StartChatArgs, StartChatResult, approve_plan, attach_run, cancel_run,
    compact_session, inject_run_message, remove_followup, resolve_ask, resume_followup_queue,
    session_pending_ask, session_run_snapshot, start_chat,
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
// 会话偏好两命令（18 号票，§3.3 #24/#25）
#[allow(unused_imports)]
use host::commands::session::{
    GetSessionPrefsResult, SessionPrefsArgs, SetSessionPrefsArgs, get_session_prefs,
    set_session_prefs,
};
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
    AllowExternalDirArgs, CheckExternalPathArgs, CheckExternalPathResult, ExternalRootsResult,
    PathKindArgs, PathKindResult, ReadWorkspaceFileArgs, ReadWorkspaceResult,
    RevokeExternalDirArgs, SearchWorkspacePathsArgs, WorkspacePathHit,
};
// MCP 八命令（08 号票，§3.3 #40–47）：契约类型与命令实现同源进宏清单
#[allow(unused_imports)]
use mcp::commands::{
    McpListConfigArgs, McpListConfigResult, McpSaveConfigArgs, McpServerRefArgs,
    McpSetLaunchTrustArgs, McpSnapshotArgs, McpSnapshotResult, McpTestArgs, McpTestResult,
    mcp_connect, mcp_disconnect, mcp_list_config, mcp_reconnect, mcp_save_config,
    mcp_set_launch_trust, mcp_snapshot, mcp_test,
};
// 技能四命令（09 号票，§3.3 #48–51）+ 信任撤销（24 号票补充面）
#[allow(unused_imports)]
use knowledge::commands::{
    SkillInspectArgs, SkillInspectResult, SkillListEntry, SkillRevokeTrustArgs, SkillToggleArgs,
    skill_inspect, skill_list, skill_reload, skill_revoke_trust, skill_toggle,
};
// 子代理两命令（10 号票，§3.3 #58/#59）
#[allow(unused_imports)]
use host::commands::subagent::{
    LoadSubagentHistoryArgs, StopSubagentArgs, load_subagent_history, stop_subagent,
};
// 搜索连通测试命令（13 号票，§3.3 #59f）
#[allow(unused_imports)]
use host::commands::search::{SearchTestArgs, SearchTestResult, search_test};
// 供应商连接测试命令（19 号票，§3.3 #32）
#[allow(unused_imports)]
use host::commands::provider::{TestProviderArgs, TestProviderResult, test_provider};
// HTML 沙箱预览注册命令（14 号票，§3.3 #59g）
#[allow(unused_imports)]
use host::commands::preview::{PreviewRegisterArgs, PreviewRegisterResult, preview_register};
// 图片附件承载命令（17 号票，§3.3 #59h/#59i；宏展开仅 stringify 类型名，
// 导入不被消耗——与 preview 行同款 allow）
#[allow(unused_imports)]
use host::commands::image::{
    AttachImageArgs, AttachImageResult, ReadImageBlobArgs, ReadImageBlobResult,
};
// 用量统计两命令（20 号票，§3.3 #52/#59k）
#[allow(unused_imports)]
use host::commands::stats::{
    SessionUsageArgs, SessionUsageResult, StatsTokenArgs, StatsTokenResult, session_usage,
    stats_token,
};
// 日志三命令（32 号票，§3.3 #54–56）
#[allow(unused_imports)]
use host::commands::logs::{
    ReadSessionLogArgs, ReadSessionLogResult, list_log_files, open_logs_dir, read_session_log,
};
// 更新器支撑三命令（33 号票，§3.3 #59m/#59n/#59o）
#[allow(unused_imports)]
use host::commands::updater::{
    PauseFollowupQueueArgs, PauseFollowupQueueResult, UpdatePrecheckResult, UpdateStatusResult,
    pause_followup_queue, update_precheck, update_status,
};
// 配额快照命令（21 号票，§3.3 #53）：QuotaSnapshot 是 Vec<_> 元素（非名义
// 根，经 quota_snapshots 返回），导出需显式补根（ipc_manifest_test）
#[allow(unused_imports)]
use host::commands::quota::quota_snapshots;
// 产物面板与编辑器打开（28 号票，§3.3 #59l/#35/#36）
#[allow(unused_imports)]
use host::commands::artifacts::{
    EditorEntry, ListArtifactsArgs, OpenInEditorArgs, list_artifacts, list_editors, open_in_editor,
};
#[allow(unused_imports)]
use storage::ListArtifactsResult;
// git 三命令（26 号票，§3.3 #37–#39）
#[allow(unused_imports)]
use git::{
    GitDiffArgs, GitDiffResult, GitLogArgs, GitLogResult, GitStatusArgs, GitStatusResult, git_diff,
    git_recent_log, git_status,
};
#[allow(unused_imports)]
use providers::quota::QuotaSnapshot;
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
// 子代理历史命令返回面（10 号票：Vec<MessageEntry> 经 cfg(test) 的
// export_contract_ts 解析，需要真实类型路径在作用域内）
#[allow(unused_imports)]
use storage::MessageEntry;
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
    // 会话偏好两命令（18 号票，§3.3 #24/#25 实装）
    host::commands::session::get_session_prefs => args: SessionPrefsArgs, ret: GetSessionPrefsResult;
    host::commands::session::set_session_prefs => args: SetSessionPrefsArgs, ret: ();
    host::commands::workspace::search_workspace_paths => args: SearchWorkspacePathsArgs, ret: Vec<WorkspacePathHit>;
    host::commands::workspace::read_workspace_file => args: ReadWorkspaceFileArgs, ret: ReadWorkspaceResult;
    // 外部目录授权（24 号票，§3.3 #33/#34 + 撤销补充面）
    host::commands::workspace::check_external_path => args: CheckExternalPathArgs, ret: CheckExternalPathResult;
    host::commands::workspace::allow_external_dir => args: AllowExternalDirArgs, ret: ExternalRootsResult;
    host::commands::workspace::revoke_external_dir => args: RevokeExternalDirArgs, ret: ExternalRootsResult;
    host::commands::ui_state::get_ui_state => args: GetUiStateArgs, ret: Value;
    host::commands::ui_state::set_ui_state => args: SetUiStateArgs, ret: ();
    host::commands::config::get_config => args: (), ret: AppConfig;
    host::commands::config::save_config => args: SaveConfigArgs, ret: AppConfig;
    host::commands::provider::save_provider => args: SaveProviderArgs, ret: ProviderConfig;
    host::commands::provider::delete_provider => args: DeleteProviderArgs, ret: ();
    host::commands::chat::start_chat => args: StartChatArgs, ret: StartChatResult;
    host::commands::chat::cancel_run => args: CancelRunArgs, ret: bool;
    host::commands::chat::inject_run_message => args: InjectRunMessageArgs, ret: InjectRunMessageResult;
    host::commands::chat::resume_followup_queue => args: ResumeFollowupQueueArgs, ret: ResumeFollowupQueueResult;
    host::commands::chat::remove_followup => args: RemoveFollowupArgs, ret: RemoveFollowupResult;
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
    // MCP 八命令（08 号票，§3.3 #40–47 逐字）
    mcp::commands::mcp_list_config => args: McpListConfigArgs, ret: McpListConfigResult;
    mcp::commands::mcp_save_config => args: McpSaveConfigArgs, ret: ();
    mcp::commands::mcp_connect => args: McpServerRefArgs, ret: ();
    mcp::commands::mcp_disconnect => args: McpServerRefArgs, ret: ();
    mcp::commands::mcp_reconnect => args: McpServerRefArgs, ret: ();
    mcp::commands::mcp_test => args: McpTestArgs, ret: McpTestResult;
    mcp::commands::mcp_set_launch_trust => args: McpSetLaunchTrustArgs, ret: ();
    mcp::commands::mcp_snapshot => args: McpSnapshotArgs, ret: McpSnapshotResult;
    // 技能四命令（09 号票，§3.3 #48–51 逐字）+ 信任撤销（24 号票补充面）
    knowledge::commands::skill_list => args: (), ret: Vec<SkillListEntry>;
    knowledge::commands::skill_toggle => args: SkillToggleArgs, ret: ();
    knowledge::commands::skill_reload => args: (), ret: ();
    knowledge::commands::skill_inspect => args: SkillInspectArgs, ret: SkillInspectResult;
    knowledge::commands::skill_revoke_trust => args: SkillRevokeTrustArgs, ret: ();
    // 子代理两命令（10 号票，§3.3 #58/#59 逐字）
    host::commands::subagent::stop_subagent => args: StopSubagentArgs, ret: ();
    host::commands::subagent::load_subagent_history => args: LoadSubagentHistoryArgs, ret: Vec<MessageEntry>;
    // 网络搜索连通测试（13 号票，§3.3 #59f）
    host::commands::search::search_test => args: SearchTestArgs, ret: SearchTestResult;
    // 供应商连接测试（19 号票，§3.3 #32）
    host::commands::provider::test_provider => args: TestProviderArgs, ret: TestProviderResult;
    // HTML 沙箱预览注册（14 号票，§3.3 #59g）
    host::commands::preview::preview_register => args: PreviewRegisterArgs, ret: PreviewRegisterResult;
    // 图片附件承载（17 号票，§3.3 #59h/#59i/#59j）
    host::commands::image::attach_image => args: AttachImageArgs, ret: AttachImageResult;
    host::commands::image::read_image_blob => args: ReadImageBlobArgs, ret: ReadImageBlobResult;
    host::commands::workspace::path_kind => args: PathKindArgs, ret: PathKindResult;
    // 用量统计（20 号票，§3.3 #52/#59k）
    host::commands::stats::stats_token => args: StatsTokenArgs, ret: StatsTokenResult;
    host::commands::stats::session_usage => args: SessionUsageArgs, ret: SessionUsageResult;
    // 配额快照（21 号票，§3.3 #53）
    host::commands::quota::quota_snapshots => args: (), ret: Vec<QuotaSnapshot>;
    // 产物面板与编辑器打开（28 号票，§3.3 #59l 增补 + #35/#36 实装）
    host::commands::artifacts::list_artifacts => args: ListArtifactsArgs, ret: ListArtifactsResult;
    host::commands::artifacts::list_editors => args: (), ret: Vec<EditorEntry>;
    host::commands::artifacts::open_in_editor => args: OpenInEditorArgs, ret: ();
    // 日志三命令（32 号票，§3.3 #54–56：读取/清单/打开目录）
    host::commands::logs::read_session_log => args: ReadSessionLogArgs, ret: ReadSessionLogResult;
    host::commands::logs::list_log_files => args: (), ret: Vec<String>;
    host::commands::logs::open_logs_dir => args: (), ret: ();
    // 更新器支撑三命令（33 号票，§3.3 #59m/#59n/#59o：状态门/安装门/主动暂停）
    host::commands::updater::update_status => args: (), ret: UpdateStatusResult;
    host::commands::updater::update_precheck => args: (), ret: UpdatePrecheckResult;
    host::commands::updater::pause_followup_queue => args: PauseFollowupQueueArgs, ret: PauseFollowupQueueResult;
    // git 只读三命令（26 号票，§3.3 #37–#39）
    git::git_status => args: GitStatusArgs, ret: GitStatusResult;
    git::git_diff => args: GitDiffArgs, ret: GitDiffResult;
    git::git_recent_log => args: GitLogArgs, ret: GitLogResult;
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
        // 应用内更新（33 号票，FR-16-03/§12.4）：check/downloadAndInstall 由
        // 前端经插件 JS API 发起（实际下载与签名校验在 Rust 侧执行，不经
        // WebView——CSP connect-src 无需放开）；pubkey 占位空串期间 check
        // 必败，入口由 update_status.pubkeyConfigured 禁用兜底（票内 Answer）。
        .plugin(tauri_plugin_updater::Builder::new().build())
        // relaunch（安装完成后的「重启以完成更新」）：前端插件 JS API 直调，
        // capabilities 仅放行 process:allow-restart（NFR-10 最小权限）。
        .plugin(tauri_plugin_process::init())
        // 全局热键（29 号票，FR-13-11/§10.7）：插件的快捷键注册由前端按配置
        // 发起（冲突错误经插件 JS API 原生回传设置页），Rust 侧只挂回调——
        // 快捷键按下即 toggle 主窗（可见隐藏 / 隐藏唤起，shell::toggle_main_window）。
        .plugin({
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        host::shell::toggle_main_window(app);
                    }
                })
                .build()
        })
        // 窗口事件（29 号票）：关闭按配置分流（closeToTray）+ 主窗几何记忆
        // （防抖保存 / 关闭强写 / 销毁冲盘）。只认主窗：本应用单窗口，判定
        // 仍显式写出防未来多窗时误记。事件回调给的是 &Window（无 webview
        // 面），几何 API 统一走 WebviewWindow，经一次标签查找对齐。
        .on_window_event(|window, event| {
            if window.label() != host::shell::MAIN_WINDOW {
                return;
            }
            let Some(window) = window
                .app_handle()
                .get_webview_window(host::shell::MAIN_WINDOW)
            else {
                return;
            };
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    // 关闭面几何强写先行：走退出分支时进程随即将终，防抖
                    // 窗口里的最后一帧几何此刻必须落盘
                    if let Some(state) = window.app_handle().try_state::<host::AppState>() {
                        state.window_geometry.save_now(&window);
                    }
                    // closeToTray（§9.6/29 号票）：true（缺省）= hide 收进托盘
                    // 继续运行；false = 走退出拦截协议（活跃 run 先经
                    // app:exit_requested 前端确认）。配置读不到按现状 hide。
                    let close_to_tray = window
                        .app_handle()
                        .try_state::<host::AppState>()
                        .and_then(|state| state.config.load().ok())
                        .map(|config| config.ui.close_to_tray)
                        .unwrap_or(true);
                    if close_to_tray {
                        let _ = window.hide();
                    } else {
                        host::shell::request_exit(window.app_handle());
                    }
                }
                // 几何记忆（§10.7）：移动/缩放防抖 500ms 落盘；销毁时窗口
                // 方法已不可用，直接把缓存冲盘（终值语义）。
                tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
                    if let Some(state) = window.app_handle().try_state::<host::AppState>() {
                        state.window_geometry.on_geometry_event(&window);
                    }
                }
                tauri::WindowEvent::Destroyed => {
                    if let Some(state) = window.app_handle().try_state::<host::AppState>() {
                        state.window_geometry.flush();
                    }
                }
                _ => {}
            }
        })
        // HTML 沙箱预览协议（14 号票，§8.3 render_html 行/§9.5，FR-12-04）：
        // 自定义协议 `preview`（Windows 形态 http://preview.localhost）承载
        // render_html 产出与审批卡「查看预览」的 iframe 加载。为什么不用
        // srcdoc/blob:：这两类帧会继承主窗 CSP（`script-src 'self'` 拦预览页
        // 内联脚本，票内验证点定案）；自定义协议响应自带预览专用 CSP
        // （PREVIEW_CSP：放内联脚本/样式、禁一切外联防内网探测），且 iframe
        // 再套 sandbox="allow-scripts"（无 allow-same-origin，opaque origin）
        // 双重隔离。内容只在进程内存（PreviewRegistry，token 寻址），不落盘。
        .register_uri_scheme_protocol("preview", |_ctx, request| {
            let token = request
                .uri()
                .path()
                .trim_start_matches('/')
                .split('/')
                .next()
                .unwrap_or("")
                .to_string();
            // 响应构造统一附预览 CSP 与 nosniff/no-store（404 说明页同款）。
            let respond = |status: tauri::http::StatusCode, html: String| {
                tauri::http::Response::builder()
                    .status(status)
                    .header("Content-Type", "text/html; charset=utf-8")
                    .header(
                        "Content-Security-Policy",
                        crate::tools::render_html::PREVIEW_CSP,
                    )
                    .header("X-Content-Type-Options", "nosniff")
                    .header("Cache-Control", "no-store")
                    .body(html.into_bytes())
                    .expect("预览响应构造不失败（静态头 + 内存体）")
            };
            let registry = crate::tools::render_html::preview_registry();
            if crate::tools::render_html::is_valid_token(&token)
                && let Some(html) = registry.lookup(&token)
            {
                return respond(tauri::http::StatusCode::OK, html);
            }
            // token 失效（应用重启后历史卡片）或非法：说明页引导重新预览。
            respond(
                tauri::http::StatusCode::NOT_FOUND,
                "<!DOCTYPE html><meta charset=\"utf-8\">                 <body style=\"font-family:system-ui;color:#888;padding:16px\">                 预览内容已失效，请让 AI 重新生成或展开该卡片重新加载。</body>"
                    .to_string(),
            )
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
            // Q41 恢复保护序列（33 号票，§7.7）：数据目录根存在恢复标记
            // （用户手动恢复旧升级备份后按说明复制回 .restore-pending.json）
            // 时，先执行保护——信任/MCP 工具许可/外部目录授权/审批白名单
            // 清空、全部待发队列强制暂停——再进入正常装配，保证保护在
            // 首次启动前生效。普通重启（无标记）零动作。config 用同一份
            // Arc：白名单清空经 ConfigStore.update 走唯一写入口，缓存热
            // 替换后与后续全部读取一致。执行报告在启动序记账处落 app.log。
            let restore_report =
                storage::backup::apply_restore_protection(&data_dir, Some(config.as_ref()));
            let active_session = std::sync::Arc::new(std::sync::Mutex::new(None));
            let notify = std::sync::Arc::new(host::notify::NotifyGate::new(
                handle.clone(),
                active_session.clone(),
                config.clone(),
            ));
            host::notify::install_click_routing(&notify);
            let mut state = host::AppState::new_in(
                data_dir.clone(),
                std::sync::Arc::new(host::TauriEventSink {
                    app: handle.clone(),
                    notify: notify.clone(),
                }),
            );
            // 配置缓存句柄统一（32 号票 rebind_config）：通知门持有的这份
            // config 是唯一权威——AppState 与会话日志器必须同指一份，否则
            // save_config 的内存热替换到不了日志级别过滤（热生效失效）
            state.rebind_config(config.clone());
            state.active_session = active_session;
            // 内置技能根（09 号票，§9.8 目录清单）：打包态经 Resource 目录
            // 解析（bundle.resources 声明的 resources/skills/**）；解析失败
            // （dev/headless）留 None——knowledge::commands::resolve_builtin_root
            // 回退源码树路径。
            state.resource_skills_dir = app
                .path()
                .resolve("resources/skills", tauri::path::BaseDirectory::Resource)
                .ok();
            // 子代理全局并发上限（10 号票，§5.9 Q13）：启动时按配置应用一次
            // （save_config 修改后在保存边界热生效）
            state.apply_subagent_limit();
            // 技能注册表装配（09 号票）：三来源根一次配齐 + 首扫（进程级
            // 单例；后续 run 装配与命令面按活跃会话上下文重扫）。
            knowledge::commands::configure_registry(&state);
            // RunManager 以共享句柄单独 manage：start_chat/cancel_run/
            // resolve_ask/approve_plan 以 State<RunManager> 注入（Clone =
            // 同一注册表，§5.1）；漏 manage 会报 "state not managed for
            // field `runs`"（命令面 headless 测试不经 tauri，装配需此处钉死）。
            // SessionStore 同理共享 Arc：命令层与 run 落盘器（15 号票）同一锁表。
            app.manage(state.runs.clone());
            app.manage(state.store.clone());
            app.manage(state);
            host::shell::install_tray(&handle)?;
            // 启动序记账（32 号票，app.log：运行元数据——不涉会话正文）
            if let Some(state) = app.try_state::<host::AppState>() {
                // 恢复保护序列记账（33 号票，Q41）：有恢复才落账；失败面
                // 逐条 Warn（不静默——用户据此知道哪些面可能残留旧许可）
                if restore_report.applied {
                    for action in &restore_report.actions {
                        state.session_log.app_line(
                            crate::providers::types::LogLevel::Info,
                            action.clone(),
                            None,
                        );
                    }
                    for failure in &restore_report.failures {
                        state.session_log.app_line(
                            crate::providers::types::LogLevel::Warn,
                            failure.clone(),
                            None,
                        );
                    }
                }
                state.session_log.app_line(
                    crate::providers::types::LogLevel::Info,
                    "应用启动完成".to_owned(),
                    Some(serde_json::json!({
                        "dataDir": data_dir.display().to_string(),
                        "version": app.package_info().version.to_string(),
                    })),
                );
            }
            // 窗口几何恢复（29 号票，§10.7）：此刻窗口仍隐藏（visible:false、
            // 激活协议未亮窗），先读 window-state.json 应用几何——恢复天然
            // 先于亮窗；越界/损坏回 tauri.conf.json 默认。
            if let Some(state) = app.try_state::<host::AppState>()
                && let Some(window) = handle.get_webview_window(host::shell::MAIN_WINDOW)
            {
                state.window_geometry.restore(&window);
            }
            Ok(())
        })
        .invoke_handler(invoke_handler())
        .run(tauri::generate_context!())
        .expect("CraftWave 启动失败");
}
