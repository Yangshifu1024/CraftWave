//! engine 对 UI 的 API 面（gpui-rewrite 阶段 2.A「接口三件套」之一）。
//!
//! ## 定位
//!
//! 旧栈里 UI 经 Tauri IPC（`invoke` 命令 + `Channel` 帧通道）访问引擎；
//! GPUI 壳与引擎同进程，不需要 IPC 层——本模块把旧壳
//! `src-tauri/src/lib.rs` 的 `ipc_commands!` 清单**逐条转译**为可直调的
//! Rust 函数（阶段 2.A「接口先行」，spec 定稿原则 6：阶段 2 全部切片
//! 对着本模块的签名开发，接口变更走显式提交）。
//!
//! ## 形态约定
//!
//! - 函数第一参数普遍是 `&AppState`（engine 的应用级共享状态，GPUI 壳
//!   负责构造并注入 `EventSink` 生产实现），需要 `RunManager` 的从
//!   `app.runs` 取——与旧壳 `git_cmds.rs` 的 `resolve_session_roots(&app, …)`
//!   同款参数传入用法；
//! - 命令名 snake_case → 函数名同名（`start_chat` → [`run::start_chat`]）；
//!   Args/Result 载荷类型沿用旧壳的契约结构（camelCase serde），字段
//!   不改名不删减——wire 契约锁定不变（spec 定稿原则 4）；
//! - 纯同步的命令就是同步 `fn`（旧壳为统一 async 而包了一层；GPUI 调用方
//!   自行决定 `cx.background_executor` / `spawn_blocking`），涉及网络或
//!   await 的保持 `async fn`；
//! - 旧壳命令层里框架无关的 `*_core` 函数与校验逻辑**原样平移**进本模块
//!   （业务核心仍全部住在 storage/backend/git/providers 等引擎域，一行不
//!   复制）；旧壳文件本身不动（冻结），双份文本在阶段 5 删壳时自然归一；
//! - 帧下发端：旧壳的 `Channel<SequencedFrame>` 换成 [`SequencedSink`]
//!   trait 参数（GPUI 壳实现它把帧推给自己的 Entity）。
//!
//! ## 实现状态标注
//!
//! [`ApiStatus::Ready`] = 本模块内已有完整实现，GPUI 壳可直接调用
//! （阶段 2.C-2a 起全部 68 条非壳命令 Ready——run 装配树见 `run` 模块
//! 文档的子模块划分）；[`ApiStatus::Planned`] = 签名已定、函数体待平移
//! （当前已清零，枚举变体保留供后续切片复用）；[`ApiStatus::ShellOnly`] =
//! 纯壳命令，不进 API 面（见下方排除清单）。
//!
//! ## 旧命令 → 新 API 映射表（契约文档，一条一行）
//!
//! ### 会话树持久化（engine storage 直转）
//!
//! | 旧命令 | 新 API | 状态 |
//! |---|---|---|
//! | `save_session_tree` | [`session::save_session_tree`] | Ready |
//! | `load_session_tree` | [`session::load_session_tree`] | Ready |
//! | `list_sessions` | [`session::list_sessions`] | Ready |
//! | `load_tool_outcomes` | [`session::load_tool_outcomes`] | Ready |
//!
//! ### 项目（engine ProjectStore 直转）
//!
//! | 旧命令 | 新 API | 状态 |
//! |---|---|---|
//! | `list_projects` | [`session::list_projects`] | Ready |
//! | `save_project` | [`session::save_project`] | Ready |
//! | `delete_project` | [`session::delete_project`] | Ready |
//!
//! ### 会话生命周期与偏好
//!
//! | 旧命令 | 新 API | 状态 |
//! |---|---|---|
//! | `create_session` | [`session::create_session`] | Ready |
//! | `delete_session` | [`session::delete_session`] | Ready |
//! | `rename_session` | [`session::rename_session`] | Ready |
//! | `get_session_prefs` | [`session::get_session_prefs`] | Ready |
//! | `set_session_prefs` | [`session::set_session_prefs`] | Ready |
//! | `attach_image` | [`session::attach_image`] | Ready |
//! | `read_image_blob` | [`session::read_image_blob`] | Ready |
//!
//! ### 工作区与外部目录授权
//!
//! | 旧命令 | 新 API | 状态 |
//! |---|---|---|
//! | `search_workspace_paths` | [`workspace::search_workspace_paths`] | Ready |
//! | `read_workspace_file` | [`workspace::read_workspace_file`] | Ready |
//! | `path_kind` | [`workspace::path_kind`] | Ready |
//! | `check_external_path` | [`workspace::check_external_path`] | Ready |
//! | `allow_external_dir` | [`workspace::allow_external_dir`] | Ready |
//! | `revoke_external_dir` | [`workspace::revoke_external_dir`] | Ready |
//!
//! ### 运行（start_chat → 帧流）
//!
//! | 旧命令 | 新 API | 状态 |
//! |---|---|---|
//! | `start_chat` | [`run::start_chat`] | Ready（阶段 2.C-2a 平移 run 装配树） |
//! | `cancel_run` | [`run::cancel_run`] | Ready |
//! | `inject_run_message` | [`run::inject_run_message`] | Ready |
//! | `resume_followup_queue` | [`run::resume_followup_queue`] | Ready（阶段 2.C-2a 同切片平移） |
//! | `remove_followup` | [`run::remove_followup`] | Ready |
//! | `resolve_ask` | [`run::resolve_ask`] | Ready |
//! | `approve_plan` | [`run::approve_plan`] | Ready |
//! | `compact_session` | [`run::compact_session`] | Ready（阶段 2.C-2a 同切片平移） |
//! | `attach_run` | [`run::attach_run`] | Ready |
//! | `session_run_snapshot` | [`run::session_run_snapshot`] | Ready |
//! | `session_pending_ask` | [`run::session_pending_ask`] | Ready |
//!
//! ### 设置（config / provider / ui_state / 用量统计 / 配额）
//!
//! | 旧命令 | 新 API | 状态 |
//! |---|---|---|
//! | `get_config` | [`settings::get_config`] | Ready |
//! | `save_config` | [`settings::save_config`] | Ready |
//! | `save_provider` | [`settings::save_provider`] | Ready |
//! | `delete_provider` | [`settings::delete_provider`] | Ready |
//! | `test_provider` | [`settings::test_provider`] | Ready |
//! | `get_ui_state` | [`settings::get_ui_state`] | Ready |
//! | `set_ui_state` | [`settings::set_ui_state`] | Ready |
//! | `stats_token` | [`settings::stats_token`] | Ready |
//! | `session_usage` | [`settings::session_usage`] | Ready |
//! | `quota_snapshots` | [`settings::quota_snapshots`] | Ready |
//!
//! ### MCP 管理（engine mcp 域直转）
//!
//! | 旧命令 | 新 API | 状态 |
//! |---|---|---|
//! | `mcp_list_config` | [`mcp::mcp_list_config`] | Ready |
//! | `mcp_save_config` | [`mcp::mcp_save_config`] | Ready |
//! | `mcp_connect` | [`mcp::mcp_connect`] | Ready |
//! | `mcp_disconnect` | [`mcp::mcp_disconnect`] | Ready |
//! | `mcp_reconnect` | [`mcp::mcp_reconnect`] | Ready |
//! | `mcp_test` | [`mcp::mcp_test`] | Ready |
//! | `mcp_set_launch_trust` | [`mcp::mcp_set_launch_trust`] | Ready |
//! | `mcp_snapshot` | [`mcp::mcp_snapshot`] | Ready |
//!
//! ### 技能（knowledge 域直转）
//!
//! | 旧命令 | 新 API | 状态 |
//! |---|---|---|
//! | `skill_list` | [`knowledge::skill_list`] | Ready |
//! | `skill_toggle` | [`knowledge::skill_toggle`] | Ready |
//! | `skill_reload` | [`knowledge::skill_reload`] | Ready |
//! | `skill_inspect` | [`knowledge::skill_inspect`] | Ready |
//! | `skill_revoke_trust` | [`knowledge::skill_revoke_trust`] | Ready |
//!
//! ### 子代理（stop/历史平移；spawn 编排随 start_chat 树走）
//!
//! | 旧命令 | 新 API | 状态 |
//! |---|---|---|
//! | `stop_subagent` | [`misc::stop_subagent`] | Ready |
//! | `load_subagent_history` | [`misc::load_subagent_history`] | Ready |
//!
//! ### 面板与杂项（搜索连通 / 预览 / 产物 / 日志 / 更新门 / git）
//!
//! | 旧命令 | 新 API | 状态 |
//! |---|---|---|
//! | `search_test` | [`misc::search_test`] | Ready |
//! | `preview_register` | [`misc::preview_register`] | Ready |
//! | `list_artifacts` | [`misc::list_artifacts`] | Ready |
//! | `list_editors` | [`misc::list_editors`] | Ready |
//! | `open_in_editor` | [`misc::open_in_editor`] | Ready |
//! | `read_session_log` | [`misc::read_session_log`] | Ready |
//! | `list_log_files` | [`misc::list_log_files`] | Ready |
//! | `update_precheck` | [`misc::update_precheck`] | Ready |
//! | `pause_followup_queue` | [`misc::pause_followup_queue`] | Ready |
//! | `git_status` | [`misc::git_status`] | Ready |
//! | `git_diff` | [`misc::git_diff`] | Ready |
//! | `git_recent_log` | [`misc::git_recent_log`] | Ready |
//!
//! ## 排除清单（不进 API 面的纯壳命令及原因）
//!
//! 全部推迟到阶段 4「桌面集成」用 GPUI 原生/独立 crate 方案实现：
//!
//! | 旧命令 | 排除原因与去向 |
//! |---|---|
//! | `activate_custom_titlebar` | Tauri decoration 插件专属（自绘标题栏激活）；GPUI 壳自绘 TitleBar（gpui-kit）天然无此命令 |
//! | `restore_native_titlebar` | 同上（自绘失败回退系统标题栏的看门狗路径，GPUI 壳不存在该双态） |
//! | `select_dir` | 原生目录选择（rfd）；阶段 4 GPUI 侧用 rfd 或 GPUI 文件对话框，属壳装配 |
//! | `resolve_exit_request` | 退出拦截回执（tauri `AppHandle::exit`）；状态机 `ExitConfirm` 已在 engine（`AppState.exit_confirm`），GPUI 壳按阶段 4.6 Wake `should_close` 模式直接操作状态机并自管退出 |
//! | `notification_permission_status` | 系统通知授权态（macOS UNUserNotificationCenter 实查）；阶段 4.3 gpui-component `SystemNotification` 时一并落壳侧 |
//! | `open_notification_settings` | 系统设置深链（OS 命令）；同上属壳能力 |
//! | `open_dir` | 系统文件管理器打开目录（`open` crate）；白名单校验逻辑（旧壳 `system.rs::resolve_open_dir`：PathGuard 数据目录+日志目录+项目数据目录）在阶段 4 实现时平移，**校验红线不得省略** |
//! | `open_logs_dir` | 同 `open_dir`（固定打开日志目录） |
//! | `update_status` | 更新器禁用门（读 tauri.conf.json 的 updater 插件装配）；阶段 4.5 先查 gpui-kit Auto Update 覆盖面再定（spec 阶段 4） |
//! | `mock_chat_run` | dev-only 假帧源（旧壳 S6 遗留）；GPUI 壳的测试帧源由阶段 2.12 UI 测试装置自备假数据 |
//!
//! ## 与帧流的接缝
//!
//! run 产生的帧经 `FrameHub` → [`SequencedSink`] 下发（旧壳的
//! `ChannelSink` 在 GPUI 侧换成把帧推入 Entity 的实现）；UI 侧消费帧的
//! 64ms 聚合与快照形态见 `crate::snapshot`（接口三件套之二）。

pub mod knowledge;
pub mod mcp;
pub mod misc;
pub mod run;
pub mod session;
pub mod settings;
pub mod workspace;

/// API 函数的实现状态（[`api_manifest`] 的标注维度；逐条含义见模块文档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiStatus {
    /// 本模块内已有完整实现，可直接调用。
    Ready,
    /// 签名与契约已定，实现待平移旧壳 chat.rs 的 run 装配树（目标切片见函数文档）。
    Planned,
    /// 纯壳命令，不进 API 面（阶段 4 桌面集成用 GPUI 原生方案）。
    ShellOnly,
}

/// 单条 API 的机器可读清单行（旧命令名 + 状态；镜像旧壳
/// `ipc_manifest` 的对账形态，供测试与阶段 5 功能覆盖审计消费）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiSpec {
    /// 旧 IPC 命令名（snake_case，与旧壳 `ipc_commands!` 清单一字不差）。
    pub name: &'static str,
    /// 实现状态。
    pub status: ApiStatus,
}

/// 全部旧命令 → 新 API 的机器可读清单（与上方映射表同源手写；
/// `api_test` 断言总数与名单完整，防文档表与清单漂移）。
pub fn api_manifest() -> Vec<ApiSpec> {
    use ApiStatus::{Ready, ShellOnly};
    vec![
        // 会话树持久化
        ApiSpec { name: "save_session_tree", status: Ready },
        ApiSpec { name: "load_session_tree", status: Ready },
        ApiSpec { name: "list_sessions", status: Ready },
        ApiSpec { name: "load_tool_outcomes", status: Ready },
        // 项目
        ApiSpec { name: "list_projects", status: Ready },
        ApiSpec { name: "save_project", status: Ready },
        ApiSpec { name: "delete_project", status: Ready },
        // 会话生命周期与偏好
        ApiSpec { name: "create_session", status: Ready },
        ApiSpec { name: "delete_session", status: Ready },
        ApiSpec { name: "rename_session", status: Ready },
        ApiSpec { name: "get_session_prefs", status: Ready },
        ApiSpec { name: "set_session_prefs", status: Ready },
        ApiSpec { name: "attach_image", status: Ready },
        ApiSpec { name: "read_image_blob", status: Ready },
        // 工作区与外部目录
        ApiSpec { name: "search_workspace_paths", status: Ready },
        ApiSpec { name: "read_workspace_file", status: Ready },
        ApiSpec { name: "path_kind", status: Ready },
        ApiSpec { name: "check_external_path", status: Ready },
        ApiSpec { name: "allow_external_dir", status: Ready },
        ApiSpec { name: "revoke_external_dir", status: Ready },
        // 运行
        ApiSpec { name: "start_chat", status: Ready },
        ApiSpec { name: "cancel_run", status: Ready },
        ApiSpec { name: "inject_run_message", status: Ready },
        ApiSpec { name: "resume_followup_queue", status: Ready },
        ApiSpec { name: "remove_followup", status: Ready },
        ApiSpec { name: "resolve_ask", status: Ready },
        ApiSpec { name: "approve_plan", status: Ready },
        ApiSpec { name: "compact_session", status: Ready },
        ApiSpec { name: "attach_run", status: Ready },
        ApiSpec { name: "session_run_snapshot", status: Ready },
        ApiSpec { name: "session_pending_ask", status: Ready },
        // 设置 / 用量 / 配额
        ApiSpec { name: "get_config", status: Ready },
        ApiSpec { name: "save_config", status: Ready },
        ApiSpec { name: "save_provider", status: Ready },
        ApiSpec { name: "delete_provider", status: Ready },
        ApiSpec { name: "test_provider", status: Ready },
        ApiSpec { name: "get_ui_state", status: Ready },
        ApiSpec { name: "set_ui_state", status: Ready },
        ApiSpec { name: "stats_token", status: Ready },
        ApiSpec { name: "session_usage", status: Ready },
        ApiSpec { name: "quota_snapshots", status: Ready },
        // MCP
        ApiSpec { name: "mcp_list_config", status: Ready },
        ApiSpec { name: "mcp_save_config", status: Ready },
        ApiSpec { name: "mcp_connect", status: Ready },
        ApiSpec { name: "mcp_disconnect", status: Ready },
        ApiSpec { name: "mcp_reconnect", status: Ready },
        ApiSpec { name: "mcp_test", status: Ready },
        ApiSpec { name: "mcp_set_launch_trust", status: Ready },
        ApiSpec { name: "mcp_snapshot", status: Ready },
        // 技能
        ApiSpec { name: "skill_list", status: Ready },
        ApiSpec { name: "skill_toggle", status: Ready },
        ApiSpec { name: "skill_reload", status: Ready },
        ApiSpec { name: "skill_inspect", status: Ready },
        ApiSpec { name: "skill_revoke_trust", status: Ready },
        // 子代理
        ApiSpec { name: "stop_subagent", status: Ready },
        ApiSpec { name: "load_subagent_history", status: Ready },
        // 面板与杂项
        ApiSpec { name: "search_test", status: Ready },
        ApiSpec { name: "preview_register", status: Ready },
        ApiSpec { name: "list_artifacts", status: Ready },
        ApiSpec { name: "list_editors", status: Ready },
        ApiSpec { name: "open_in_editor", status: Ready },
        ApiSpec { name: "read_session_log", status: Ready },
        ApiSpec { name: "list_log_files", status: Ready },
        ApiSpec { name: "update_precheck", status: Ready },
        ApiSpec { name: "pause_followup_queue", status: Ready },
        ApiSpec { name: "git_status", status: Ready },
        ApiSpec { name: "git_diff", status: Ready },
        ApiSpec { name: "git_recent_log", status: Ready },
        // 排除的纯壳命令（阶段 4 桌面集成）
        ApiSpec { name: "activate_custom_titlebar", status: ShellOnly },
        ApiSpec { name: "restore_native_titlebar", status: ShellOnly },
        ApiSpec { name: "select_dir", status: ShellOnly },
        ApiSpec { name: "resolve_exit_request", status: ShellOnly },
        ApiSpec { name: "notification_permission_status", status: ShellOnly },
        ApiSpec { name: "open_notification_settings", status: ShellOnly },
        ApiSpec { name: "open_dir", status: ShellOnly },
        ApiSpec { name: "open_logs_dir", status: ShellOnly },
        ApiSpec { name: "update_status", status: ShellOnly },
        ApiSpec { name: "mock_chat_run", status: ShellOnly },
    ]
}

// 测试挂载：清单对账（仅 cargo test 时编译）。
#[cfg(test)]
#[path = "api_test.rs"]
mod api_test;
