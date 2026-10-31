//! 面板与杂项域 API：搜索连通测试、HTML 沙箱预览、产物面板与编辑器、
//! 日志读取、更新安装门、子代理停止与历史回看、git 只读面板。
//!
//! 平移来源：`host/commands/search.rs`、`preview.rs`、`artifacts.rs`、
//! `logs.rs`、`updater.rs`（判定主体）、`subagent.rs`（stop/历史两命令）、
//! `git_cmds.rs`（git 三命令）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::session_roots::resolve_session_roots;
use crate::safety::safe_join::PathGuard;
use crate::storage::MessageEntry;
use crate::util::errors::{E_IO, IpcError, ToolError};

// ---- 网络搜索连通测试（旧命令 search_test） ----

/// `search_test` 参数契约：目标通道 + 草稿配置（不经保存）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SearchTestArgs {
    /// 目标通道："searxng" | "tavily"（其余值按参数错误拒绝）。
    pub target: String,
    /// SearXNG 草稿基地址（target=searxng 时必填；空/缺失按失败返回引导）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub searxng_base_url: Option<String>,
    /// Tavily 草稿 Key（target=tavily 时必填；空/缺失按失败返回引导）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub tavily_api_key: Option<String>,
}

/// `search_test` 返回契约：ok + 时延或错误原因。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SearchTestResult {
    /// 是否连通（探测请求 2xx 且响应结构可解析）。
    pub ok: bool,
    /// 时延毫秒（ok=true 时）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub latency_ms: Option<u64>,
    /// 失败原因（ok=false 时；含配置引导，如 SearXNG 403 → settings.yml
    /// 的 search.formats 加 json）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<String>,
}

/// 连通测试总预算（秒）：SearXNG 通道自身 15s 超时、Tavily 走公网兜底
/// 60s，命令面统一 30s 收口（超时按失败返回，不挂设置页）。
const SEARCH_TEST_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

/// 网络搜索通道连通测试（草稿试连，不写配置、不落盘；
/// Tavily 测试会消耗 1 credit——设置页指引文案已注明）。SearXNG 测试 =
/// GET `{baseUrl}/search?q=…&format=json`（本地回环直连，不经代理）；
/// Tavily 测试 = POST `/search`（Bearer Key，走应用代理设置）——二者都
// search 方法在 SearchProvider trait 上（engine tools 域；旧壳同款引入）。
use crate::tools::web_search::SearchProvider;

/// 复用 tools/web_search 的通道实现（测试与工具执行同一条请求路径，
/// 测通即用通）。
pub async fn search_test(app: &AppState, args: SearchTestArgs) -> Result<SearchTestResult, IpcError> {
    // 目标通道白名单（非法值按参数错误拒绝，区别于「测了但不通」）
    if args.target != "searxng" && args.target != "tavily" {
        return Err(IpcError::invalid_arg(format!(
            "target 仅支持 searxng/tavily（实际 {}）",
            args.target
        )));
    }
    // 代理/SSRF 策略按当前配置装配（与工具执行同源；测试通道 = 真实通道）
    let config = app.config.load()?.as_ref().clone();
    let env = crate::tools::web_search::SearchEnv::from_config(&config);
    let token = tokio_util::sync::CancellationToken::new();
    // 探测 future：两通道统一为 Result<(), String>（通道错误取 message），
    // 总预算 30s 收口（tokio::time::timeout——超时按失败返回）
    let probe = async {
        match args.target.as_str() {
            // SearXNG：本地回环直连（不经代理），一次 format=json 搜索探测
            "searxng" => {
                let base_url = args.searxng_base_url.as_deref().unwrap_or("").trim();
                if base_url.is_empty() {
                    return Err("请先填写 SearXNG 服务地址".to_owned());
                }
                let searxng = crate::providers::types::SearxngConfig {
                    enabled: true,
                    base_url: base_url.trim_end_matches('/').to_owned(),
                    max_results: 1,
                };
                let provider = crate::tools::web_search::SearxngProvider::production(
                    searxng,
                    env.throttle.clone(),
                );
                provider
                    .search("craftwave connectivity test", 1, &token)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.message.to_string())
            }
            // Tavily：Bearer Key 试连（走应用代理设置；消耗 1 credit）
            _ => {
                let api_key = args.tavily_api_key.as_deref().unwrap_or("").trim();
                if api_key.is_empty() {
                    return Err("请先填写 Tavily API Key（tavily.com 免费注册）".to_owned());
                }
                let provider = crate::tools::web_search::TavilyProvider {
                    api_key: api_key.to_owned(),
                    policy: env.policy,
                    proxy: env.proxy.clone(),
                    throttle: env.throttle.clone(),
                    endpoint: crate::tools::web_search::TAVILY_ENDPOINT.to_string(),
                };
                provider
                    .search("craftwave connectivity test", 1, &token)
                    .await
                    .map(|_| ())
                    .map_err(|e| e.message.to_string())
            }
        }
    };
    let started = std::time::Instant::now();
    match tokio::time::timeout(SEARCH_TEST_BUDGET, probe).await {
        // 2xx 且结构可解析：连通（带时延）
        Ok(Ok(())) => Ok(SearchTestResult {
            ok: true,
            latency_ms: Some(started.elapsed().as_millis() as u64),
            error: None,
        }),
        // 探测失败：带通道给出的原因（含配置引导）
        Ok(Err(message)) => Ok(SearchTestResult {
            ok: false,
            latency_ms: None,
            error: Some(message),
        }),
        // 总预算超时
        Err(_) => Ok(SearchTestResult {
            ok: false,
            latency_ms: None,
            error: Some(format!(
                "连通测试超时（超过 {} 秒未收到响应）",
                SEARCH_TEST_BUDGET.as_secs()
            )),
        }),
    }
}

// ---- HTML 沙箱预览（旧命令 preview_register） ----

/// `preview_register` 参数：待预览的 HTML 全文（≤50,000 字符，与
/// render_html 工具同款上限口径）。
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PreviewRegisterArgs {
    /// 自包含 HTML 全文。
    pub html: String,
}

/// `preview_register` 返回：token 与按平台拼好的加载地址（GPUI WebView
/// / iframe 直接作 src，不自行判断平台）。
#[derive(Debug, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PreviewRegisterResult {
    /// 内容寻址 token（sha256 前 16 hex）。
    pub token: String,
    /// 预览协议 URL。
    pub url: String,
}

/// 注册预览内容：字符数校验（与 render_html 同款）→ 注册表 upsert →
/// 返回 token + URL。幂等：同内容重复注册返回同 token。注册表是进程内存
/// （不落盘），应用退出清空；预览专用 CSP 由壳的 WebView 协议处理承载
/// （GPUI 侧注册自定义协议时复用 `tools::render_html::PREVIEW_CSP`）。
pub fn preview_register(args: PreviewRegisterArgs) -> Result<PreviewRegisterResult, IpcError> {
    crate::tools::render_html::validate_html_length(&args.html)
        .map_err(|e| IpcError::new(e.code, e.message))?;
    let registry = crate::tools::render_html::preview_registry();
    let token = registry.register(&args.html);
    Ok(PreviewRegisterResult {
        url: crate::tools::render_html::preview_url(&token),
        token,
    })
}

// ---- 产物面板（旧命令 list_artifacts / list_editors / open_in_editor） ----

/// `list_artifacts` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ListArtifactsArgs {
    pub session_id: String,
}

/// 产物清单（本会话产物登记 + 存在探测就地完成；排序 = 存在优先 +
/// lastSeenAt 倒序——已删除灰显置底由排序天然达成）。会话未知/无登记
/// 文件 → 空清单（幂等查询语义）。
pub fn list_artifacts(app: &AppState, args: ListArtifactsArgs) -> Result<crate::storage::ListArtifactsResult, IpcError> {
    Ok(crate::storage::ListArtifactsResult {
        artifacts: app.store.list_artifacts(&args.session_id),
    })
}

/// 编辑器条目（`{ name, path }`）：name = 展示名；path = CLI
/// 可执行文件全路径（CLI 探测命中）或 macOS 应用名（open -a 候选）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct EditorEntry {
    pub name: String,
    pub path: String,
}

/// 行号定位参数风格（按编辑器拼 CLI 参数）：
/// - DashG：VS Code 系 `--goto file:line`；
/// - ColonSuffix：Sublime `file:line` 直接路径语法；
/// - None：不支持行号定位（Zed v1 忽略行号，尽力打开）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineStyle {
    DashG,
    ColonSuffix,
    None,
}

/// CLI 编辑器候选（探测顺序即优先级）：展示名 + 可执行名 + 行号定位风格。
#[derive(Debug, Clone, Copy)]
pub(crate) struct EditorCandidate {
    pub display: &'static str,
    pub bin: &'static str,
    pub style: LineStyle,
}

const CLI_EDITORS: &[EditorCandidate] = &[
    EditorCandidate {
        display: "VS Code",
        bin: "code",
        style: LineStyle::DashG,
    },
    EditorCandidate {
        display: "Cursor",
        bin: "cursor",
        style: LineStyle::DashG,
    },
    EditorCandidate {
        display: "Windsurf",
        bin: "windsurf",
        style: LineStyle::DashG,
    },
    EditorCandidate {
        display: "VS Code Insiders",
        bin: "code-insiders",
        style: LineStyle::DashG,
    },
    EditorCandidate {
        display: "Sublime Text",
        bin: "subl",
        style: LineStyle::ColonSuffix,
    },
    EditorCandidate {
        display: "Zed",
        bin: "zed",
        style: LineStyle::None,
    },
];

/// macOS `open -a` 候选应用（CLI 探测全空时的兜底：/Applications 下 .app
/// 存在即可用；行号定位不可用，尽力打开）。
#[cfg(target_os = "macos")]
const MACOS_APP_EDITORS: &[&str] = &[
    "Visual Studio Code",
    "Cursor",
    "Windsurf",
    "Sublime Text",
    "Zed",
];

/// 可执行判定：Unix 看权限位任一 x；Windows 看扩展名（.exe/.cmd/.bat）。
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| matches!(e.to_ascii_lowercase().as_str(), "exe" | "cmd" | "bat"))
            .unwrap_or(false)
    }
}

/// PATH 上探测一个 CLI 名，返回首个命中目录下的全路径。
fn probe_cli(bin: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(bin);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// 探测全部可用编辑器（按优先级序）：CLI 在前，macOS `open -a` 候选殿后。
pub(crate) fn probe_editors() -> Vec<(EditorCandidate, PathBuf)> {
    let mut found = Vec::new();
    for candidate in CLI_EDITORS {
        if let Some(path) = probe_cli(candidate.bin) {
            found.push((*candidate, path));
        }
    }
    #[cfg(target_os = "macos")]
    for app in MACOS_APP_EDITORS {
        let app_path = PathBuf::from("/Applications").join(format!("{app}.app"));
        if app_path.is_dir() {
            found.push((
                EditorCandidate {
                    display: "open -a",
                    bin: app,
                    style: LineStyle::None,
                },
                app_path,
            ));
        }
    }
    found
}

/// 编辑器清单（旧命令 `list_editors`）：探测结果全量返回
/// （产物页「打开」按钮的可用性提示面；打开动作不依赖本清单——
/// open_in_editor 每次现探取首个可用）。
pub fn list_editors() -> Result<Vec<EditorEntry>, IpcError> {
    Ok(probe_editors()
        .into_iter()
        .map(|(candidate, path)| EditorEntry {
            name: candidate.display.to_owned(),
            path: path.to_string_lossy().into_owned(),
        })
        .collect())
}

/// `open_in_editor` 参数契约：line 缺省 / 0 = 不带行号定位。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct OpenInEditorArgs {
    pub path: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub line: Option<u32>,
}

/// open_in_editor 的路径白名单（safe_join；与 open_dir 体系同款）：
/// 数据目录整根（含 workspace/ 代理工作区——临时会话产物落点）+ 全部注册
/// 项目主目录（项目会话产物落点）。canonicalize 失败的条目跳过（PathGuard
/// 对不存在的根会整体报错）。
fn editor_roots(app: &AppState) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = vec![app.store.root().to_path_buf()];
    if let Ok(projects) = app.projects.list() {
        roots.extend(projects.into_iter().map(|p| PathBuf::from(p.home_dir)));
    }
    roots
        .into_iter()
        .filter_map(|r| std::fs::canonicalize(r).ok())
        .collect()
}

/// ToolError → IPC 错误码（safe_join 语义原样透传，workspace 同款）。
fn to_ipc_error(err: ToolError) -> IpcError {
    IpcError::new(err.code, err.message)
}

/// 用系统编辑器打开产物（旧命令 `open_in_editor`）：白名单校验 → 探测
/// 编辑器 → 拼 CLI 参数拉起（spawn 不等待——编辑器进程自管生命周期）。
/// 探测不到可用编辑器 E_NOT_FOUND；路径越界 E_PATH_OUTSIDE_ROOTS；OS 拉起
/// 失败 E_IO。
pub fn open_in_editor(app: &AppState, args: OpenInEditorArgs) -> Result<(), IpcError> {
    if args.path.trim().is_empty() {
        return Err(IpcError::invalid_arg("path 不能为空"));
    }
    let guard = PathGuard::new(editor_roots(app)).map_err(to_ipc_error)?;
    let resolved = guard.resolve(&args.path).map_err(to_ipc_error)?;
    let resolved_str = resolved.to_string_lossy().into_owned();
    // 行号 0 视为未指定（省略定位段）。
    let line = args.line.filter(|l| *l > 0);

    let editors = probe_editors();
    let Some((candidate, program)) = editors.first() else {
        return Err(IpcError::not_found(
            "未探测到可用编辑器（支持 VS Code / Cursor / Windsurf / Sublime Text / Zed 的 CLI 或 macOS 应用）",
        ));
    };

    // 按 LineStyle 拼参数（目标 + 可选行号定位段）。
    let target = match (candidate.style, line) {
        (LineStyle::DashG, Some(l)) => format!("--goto {resolved_str}:{l}"),
        (LineStyle::DashG, None) => resolved_str.clone(),
        (LineStyle::ColonSuffix, Some(l)) => format!("{resolved_str}:{l}"),
        (LineStyle::ColonSuffix, None) => resolved_str.clone(),
        (LineStyle::None, _) => resolved_str.clone(),
    };
    // macOS open -a 候选：program 是 .app 目录，走 `open -a <app> <path>`。
    let spawned = if candidate.display == "open -a" {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg("-a")
                .arg(candidate.bin)
                .arg(&resolved_str)
                .spawn()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(std::io::Error::other("open -a 仅 macOS 可用"))
        }
    } else {
        let mut cmd = std::process::Command::new(program);
        if target.starts_with("--goto ") {
            cmd.arg("--goto").arg(target.trim_start_matches("--goto "));
        } else {
            cmd.arg(&target);
        }
        cmd.spawn()
    };
    spawned.map_err(|e| IpcError::io(format!("编辑器拉起失败（{}）：{e}", candidate.display)))?;
    Ok(())
}

// ---- 日志读取（旧命令 read_session_log / list_log_files；open_logs_dir
// 属壳命令排除——阶段 4 系统打开通道） ----

/// `read_session_log` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ReadSessionLogArgs {
    pub session_id: String,
    /// 尾行数（缺省 500、上限 5000——越界 clamp 到边界）。
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub tail: Option<u32>,
}

/// `read_session_log` 返回契约：尾部行清单（JSONL 原文行，
/// 调用方解析渲染）。
#[derive(Debug, Clone, Default, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ReadSessionLogResult {
    pub lines: Vec<String>,
}

/// tail 缺省值与上限（缺省 500、上限 5000）。
const TAIL_DEFAULT: u32 = 500;
const TAIL_MAX: u32 = 5000;

/// 读取会话日志尾部：文件不存在 → 空清单不报错（幂等查询
/// 语义——verbose 从未开过的会话无日志文件属正常态）。纯运维诊断面
/// （拉取式、无实时流）。
pub fn read_session_log(app: &AppState, args: ReadSessionLogArgs) -> Result<ReadSessionLogResult, IpcError> {
    // tail clamp：缺省 500、上限 5000（0 视为缺省——拉 0 行无意义）
    let clamped = args
        .tail
        .filter(|t| *t > 0)
        .unwrap_or(TAIL_DEFAULT)
        .min(TAIL_MAX);
    Ok(ReadSessionLogResult {
        lines: app.session_log.read_tail(&args.session_id, clamped as usize),
    })
}

/// 日志文件清单（旧命令 `list_log_files`）：logs/ 下全部日志文件相对名
/// （app.log* + sessions/*，相对 `~/.craftwave/logs/`；spill 子目录不在
/// 清单——布局定义两项）。目录不存在 → 空清单不报错。
pub fn list_log_files(app: &AppState) -> Result<Vec<String>, IpcError> {
    Ok(app.session_log.list_files())
}

// ---- 更新安装门（旧命令 update_precheck / pause_followup_queue；
// update_status 属壳命令排除——阶段 4.5 更新器决策） ----

/// `update_precheck` 返回契约。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct UpdatePrecheckResult {
    /// 是否阻止安装与重启（任一阻止原因命中即 true）。
    pub blocked: bool,
    /// 人话阻止原因清单（直接逐行展示；空 = 放行）。
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

/// `update_precheck` 参数契约（空参）。
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct UpdatePrecheckArgs {}

/// 安装/重启的**运行门**：后端权威面实时判定（安装前与重启前
/// 均须重新调用——下载完成后新出现的活跃 run 仍能阻止安装）。判定口径：
/// 活跃 run（任一会话，审批挂起中算活跃）→ 阻止；应用级未决询问
/// → 阻止；未暂停的待发队列 → 阻止（已暂停队列不阻止——持久保留在
/// queue.json）。检查与下载不受本门限制。
pub fn update_precheck(app: &AppState, _args: UpdatePrecheckArgs) -> Result<UpdatePrecheckResult, IpcError> {
    let pending_ask_count = app
        .app_asks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .len();
    let queues = app.followups.all_snapshots();
    let running_queue_count = queues.iter().filter(|(_, q)| !q.paused && q.size > 0).count();
    let paused_queue_count = queues.iter().filter(|(_, q)| q.paused).count();
    let mut reasons = Vec::new();
    let running_count = app.runs.len();
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
    Ok(UpdatePrecheckResult {
        blocked: !reasons.is_empty(),
        reasons,
        running_count,
        pending_ask_count,
        running_queue_count,
        paused_queue_count,
    })
}

/// `pause_followup_queue` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PauseFollowupQueueArgs {
    pub session_id: String,
}

/// `pause_followup_queue` 返回契约：变更后队列快照（size =
/// 待发条数；paused 恒 true）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PauseFollowupQueueResult {
    pub size: usize,
    pub paused: bool,
}

/// 用户主动暂停待发队列（更新门配套——「有待发队列时，用户须先明确暂停
/// 并持久保留队列才能更新」；暂停写 queue.json 持久化，更新重启后仍等待
/// 用户明确继续）。空 sessionId 拒绝（E_INVALID_ARG）。
pub fn pause_followup_queue(
    app: &AppState,
    args: PauseFollowupQueueArgs,
) -> Result<PauseFollowupQueueResult, IpcError> {
    if args.session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    let snapshot = app.followups.set_paused(&args.session_id, true);
    Ok(PauseFollowupQueueResult {
        size: snapshot.size,
        paused: snapshot.paused,
    })
}

// ---- 子代理（旧命令 stop_subagent / load_subagent_history；
// spawn 编排随 start_chat 装配树走） ----

/// `stop_subagent` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StopSubagentArgs {
    pub sub_id: String,
}

/// 停止子代理（旧命令 `stop_subagent`）：级联取消（子代理令牌置位即返回；
/// 终态以该子代理的 subFrame 序列（runEnded + subDone）为权威）。
/// 子代理不存在或已结束 → E_NOT_FOUND。
pub fn stop_subagent(app: &AppState, args: StopSubagentArgs) -> Result<(), IpcError> {
    let handle = app
        .subs
        .get(&args.sub_id)
        .ok_or_else(|| IpcError::not_found(format!("子代理不存在或已结束：{}", args.sub_id)))?;
    handle.cancel.cancel();
    Ok(())
}

/// `load_subagent_history` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct LoadSubagentHistoryArgs {
    pub session_id: String,
    pub sub_id: String,
}

/// 子代理历史 meta（`histories/subs/<parentRunId>/<subId>/meta.json`，
/// 回看索引与 worktree 恢复指针）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SubMeta {
    pub version: u32,
    pub session_id: String,
    pub sub_id: String,
    pub parent_run_id: String,
    pub role: String,
    pub task: String,
    pub budget_steps: u32,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// worktree 保留信息（失败/取消的未汇回成果位置，恢复索引）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_root: Option<String>,
}

/// history.jsonl 的一行（serde tag=type）。Message/Tool 行按 turn 组织，
/// End 行收口（outcome + report）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SubHistoryLine {
    /// 一个模型轮次的 assistant 输出（正文 + 推理全文）。
    Message {
        turn: u32,
        text: String,
        #[serde(default)]
        reasoning: String,
    },
    /// 一次工具调用（参数全文 + 结果摘要）。
    Tool {
        turn: u32,
        call_id: String,
        name: String,
        args_json: String,
        ok: bool,
        summary: String,
    },
    /// 子代理收口行（四象限 + 汇报全文）。
    End {
        outcome: String,
        report: String,
    },
}

/// history.jsonl → 消息条目重建（旧壳 `load_sub_history_messages` 平移）：
/// 按 turn 分组，组内工具双 part（toolCall+toolResult）→ reasoning → text，
/// 条目按 turn 链成父子线；End 行的 report 成为末尾独立条目。
pub(crate) fn load_sub_history_messages(
    dir: &Path,
    sub_id: &str,
) -> Result<Vec<MessageEntry>, IpcError> {
    use crate::storage::{MessageRole, WirePart};
    use std::collections::BTreeMap;

    let path = dir.join("history.jsonl");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| IpcError::new(E_IO, format!("子代理历史读取失败：{e}")))?;
    let mut lines: Vec<SubHistoryLine> = Vec::new();
    for line in text.lines() {
        // 忽略解析失败行（崩溃残留的不完整尾行）
        if let Ok(parsed) = serde_json::from_str::<SubHistoryLine>(line) {
            lines.push(parsed);
        }
    }
    // 按 turn 分组：turn → (tools, text, reasoning)
    struct TurnGroup {
        tools: Vec<(String, String, String, bool, String)>,
        text: String,
        reasoning: String,
    }
    let mut groups: BTreeMap<u32, TurnGroup> = BTreeMap::new();
    let mut report_text: Option<String> = None;
    for line in lines {
        match line {
            SubHistoryLine::Message {
                turn,
                text,
                reasoning,
            } => {
                let group = groups.entry(turn).or_insert_with(|| TurnGroup {
                    tools: Vec::new(),
                    text: String::new(),
                    reasoning: String::new(),
                });
                group.text.push_str(&text);
                if !reasoning.is_empty() {
                    if !group.reasoning.is_empty() {
                        group.reasoning.push_str("\n\n");
                    }
                    group.reasoning.push_str(&reasoning);
                }
            }
            SubHistoryLine::Tool {
                turn,
                call_id,
                name,
                args_json,
                ok,
                summary,
            } => {
                let group = groups.entry(turn).or_insert_with(|| TurnGroup {
                    tools: Vec::new(),
                    text: String::new(),
                    reasoning: String::new(),
                });
                group.tools.push((call_id, name, args_json, ok, summary));
            }
            SubHistoryLine::End { report, .. } => {
                if !report.is_empty() {
                    report_text = Some(report);
                }
            }
        }
    }
    let mut out: Vec<MessageEntry> = Vec::new();
    let mut parent: Option<String> = None;
    for (turn, group) in groups {
        let mut parts: Vec<WirePart> = Vec::new();
        for (call_id, name, args_json, ok, summary) in &group.tools {
            parts.push(WirePart::ToolCall {
                call_id: call_id.clone(),
                name: name.clone(),
                args_json: args_json.clone(),
            });
            parts.push(WirePart::ToolResult {
                call_id: call_id.clone(),
                ok: *ok,
                summary: summary.clone(),
            });
        }
        if !group.reasoning.is_empty() {
            parts.push(WirePart::Reasoning {
                text: group.reasoning.clone(),
            });
        }
        if !group.text.is_empty() {
            parts.push(WirePart::Text {
                text: group.text.clone(),
            });
        }
        let id = format!("{sub_id}:a{turn}");
        out.push(MessageEntry {
            id: id.clone(),
            parent_id: parent.take(),
            role: MessageRole::Assistant,
            parts,
            created_at: 0,
            run_id: Some(sub_id.to_owned()),
            status: None,
        });
        parent = Some(id);
    }
    if let Some(report) = report_text {
        let id = format!("{sub_id}:report");
        out.push(MessageEntry {
            id: id.clone(),
            parent_id: parent.take(),
            role: MessageRole::Assistant,
            parts: vec![WirePart::Text { text: report }],
            created_at: 0,
            run_id: Some(sub_id.to_owned()),
            status: None,
        });
    }
    Ok(out)
}

/// 子代理历史回看（旧命令 `load_subagent_history`）：按 (sessionId, subId)
/// 在 `histories/subs/<parentRunId>/<subId>/` 下定位（parentRunId 不在参数
/// 面，扫描一层目录匹配 meta），未命中 → E_NOT_FOUND。
pub fn load_subagent_history(
    app: &AppState,
    args: LoadSubagentHistoryArgs,
) -> Result<Vec<MessageEntry>, IpcError> {
    let subs_root = app.store.root().join("histories").join("subs");
    let Ok(entries) = std::fs::read_dir(&subs_root) else {
        return Err(IpcError::not_found(format!(
            "子代理历史不存在：{}",
            args.sub_id
        )));
    };
    for entry in entries.flatten() {
        let candidate = entry.path().join(&args.sub_id);
        let meta_path = candidate.join("meta.json");
        if !meta_path.exists() {
            continue;
        }
        let hit = std::fs::read_to_string(&meta_path)
            .ok()
            .and_then(|text| serde_json::from_str::<SubMeta>(&text).ok())
            .is_some_and(|meta| {
                meta.session_id == args.session_id && meta.sub_id == args.sub_id
            });
        if hit {
            return load_sub_history_messages(&candidate, &args.sub_id);
        }
    }
    Err(IpcError::not_found(format!(
        "子代理历史不存在：{}",
        args.sub_id
    )))
}

// ---- git 只读面板（旧命令 git_status / git_diff / git_recent_log） ----

/// 仓库状态（旧命令 `git_status`）：仓库根 = sessionId → 会话 roots →
/// 项目主目录向上找 .git；临时会话的 workspace 根通常非仓库（返回
/// isRepo: false）。
pub fn git_status(app: &AppState, args: crate::git::GitStatusArgs) -> Result<crate::git::GitStatusResult, IpcError> {
    let root = resolve_session_roots(app, &args.session_id).project_root;
    Ok(crate::git::status::status_core(&root))
}

/// 工作区差异（旧命令 `git_diff`）：staged 参数忽略（聚合视图，见 Args 说明）。
pub fn git_diff(app: &AppState, args: crate::git::GitDiffArgs) -> Result<crate::git::GitDiffResult, IpcError> {
    let root = resolve_session_roots(app, &args.session_id).project_root;
    Ok(crate::git::status::diff_core(&root, args.staged))
}

/// 最近提交（旧命令 `git_recent_log`）。
pub fn git_recent_log(app: &AppState, args: crate::git::GitLogArgs) -> Result<crate::git::GitLogResult, IpcError> {
    let root = resolve_session_roots(app, &args.session_id).project_root;
    Ok(crate::git::status::log_core(&root, args.limit))
}
