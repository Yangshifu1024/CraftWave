//! 产物面板与编辑器打开（`list_artifacts` + `list_editors` +
//! `open_in_editor`）。
//!
//! 产物读取：`SessionStore::list_artifacts`（持会话锁读 `<sid>.artifacts.json`
//! + 存在探测）。
//!
//! 编辑器探测：PATH 上常见 CLI（VS Code 系 / Sublime / Zed）优先，macOS 补
//! `open -a` 候选（/Applications 下 .app 存在性）；打开拉起不经 open crate
//! （CLI 参数形态各异——`-g file:line` / `file:line` / 裸路径——逐家拼装，
//! open crate 只会丢失行号定位）。
//!
//! 路径校验：open_in_editor 走 safe_join 白名单 = 数据目录 + 全部注册项目
//! 主目录（会话 roots 的超集——产物只可能落在这里，open_dir 的
//! 白名单体系同款）；越界 E_PATH_OUTSIDE_ROOTS。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::host::AppState;
use crate::host::IpcError;
use crate::safety::safe_join::PathGuard;

/// `list_artifacts` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ListArtifactsArgs {
    pub session_id: String,
}

/// `list_artifacts` 命令：返回本会话产物清单
/// （存在探测就地完成；排序 = 存在优先 + lastSeenAt 倒序——已删除灰显
/// 置底由排序天然达成）。会话未知/无登记文件 → 空清单（幂等查询语义）。
#[tauri::command]
pub(crate) async fn list_artifacts(
    app: State<'_, AppState>,
    session_id: String,
) -> Result<crate::storage::ListArtifactsResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = ListArtifactsArgs { session_id };
    Ok(crate::storage::ListArtifactsResult {
        artifacts: app.store.list_artifacts(&args.session_id),
    })
}

/// 编辑器条目（`{ name, path }`）：name = 展示名；path = CLI
/// 可执行文件全路径（CLI 探测命中）或 macOS 应用名（open -a 候选）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct EditorEntry {
    pub name: String,
    pub path: String,
}

/// CLI 编辑器候选（探测顺序即优先级）：展示名 + 可执行名 + 行号定位风格。
#[derive(Debug, Clone, Copy)]
pub(crate) struct EditorCandidate {
    pub display: &'static str,
    pub bin: &'static str,
    pub style: LineStyle,
}

/// 行号定位参数风格（按编辑器拼 CLI 参数）：
/// - DashG：VS Code 系 `--goto file:line`（-g 短参等价）；
/// - ColonSuffix：Sublime `file:line` 直接路径语法；
/// - None：不支持行号定位（Zed v1 忽略行号，尽力打开）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineStyle {
    DashG,
    ColonSuffix,
    None,
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
/// 公开给测试（list_editors / open_in_editor 共用同一探测序）。
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

/// `list_editors` 命令：探测结果全量返回
/// （前端产物页「打开」按钮的可用性提示面；打开动作不依赖本清单——
/// open_in_editor 每次现探取首个可用）。
#[tauri::command]
pub(crate) async fn list_editors() -> Result<Vec<EditorEntry>, IpcError> {
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
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct OpenInEditorArgs {
    pub path: String,
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub line: Option<u32>,
}

/// open_in_editor 的路径白名单（safe_join；open_dir 体系同款）：
/// 数据目录整根（含 workspace/ 代理工作区——临时会话产物落点）+ 全部注册
/// 项目主目录（项目会话产物落点）。canonicalize 失败的条目跳过（PathGuard
/// 对不存在的根会整体报错）。
fn editor_roots(app: &AppState) -> Vec<PathBuf> {
    // 起始即持有所有权（后续 extend PathBuf）。
    let mut roots: Vec<PathBuf> = vec![app.store.root().to_path_buf()];
    if let Ok(projects) = app.projects.list() {
        roots.extend(projects.into_iter().map(|p| PathBuf::from(p.home_dir)));
    }
    roots
        .into_iter()
        .filter_map(|r| std::fs::canonicalize(r).ok())
        .collect()
}

/// ToolError → IPC 错误码（safe_join 语义原样透传，workspace.rs 同款）。
fn to_ipc_error(err: crate::util::errors::ToolError) -> IpcError {
    IpcError::new(err.code, err.message)
}

/// `open_in_editor` 主体（公开给测试）：白名单校验 → 探测编辑器 → 拼 CLI
/// 参数拉起（spawn 不等待——编辑器进程自管生命周期）。
pub(crate) fn open_in_editor_core(
    app: &AppState,
    path: &str,
    line: Option<u32>,
) -> Result<(), IpcError> {
    if path.trim().is_empty() {
        return Err(IpcError::invalid_arg("path 不能为空"));
    }
    let guard = PathGuard::new(editor_roots(app)).map_err(to_ipc_error)?;
    let resolved = guard.resolve(path).map_err(to_ipc_error)?;
    let resolved_str = resolved.to_string_lossy().into_owned();
    // 行号 0 视为未指定（省略定位段）。
    let line = line.filter(|l| *l > 0);

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

/// `open_in_editor` 命令：探测不到可用编辑器
/// E_NOT_FOUND（前端 Toast 提示）；路径越界 E_PATH_OUTSIDE_ROOTS；OS 拉起
/// 失败 E_IO。
#[tauri::command]
pub(crate) async fn open_in_editor(
    app: State<'_, AppState>,
    path: String,
    line: Option<u32>,
) -> Result<(), IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = OpenInEditorArgs { path, line };
    open_in_editor_core(&app, &args.path, args.line)
}
