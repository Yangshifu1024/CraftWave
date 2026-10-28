//! 工作区只读域命令（§3.3 #27 `search_workspace_paths` / #28
//! `read_workspace_file`，16 号票）：@ 引用补全与前端预览读取。
//!
//! 路径校验复用 safety::PathGuard（§9.1 safe_join 单一实现；C-8 收窄后
//! roots = 项目主目录 + 数据目录下 `workspace/` 子目录，21 号票）；解码复用
//! util::encoding（与 read 工具共用，C-10）；单次响应 ≤512KB（MAX_RESPONSE_BYTES）。
//!
//! @ 补全的返回路径约定（16 号票钉死）：项目主目录下的条目返回相对路径
//! （模型 read 工具的解析基准即项目主目录，可直接消费）；workspace 子目录
//! 专属条目返回绝对路径（数据目录本体退出可见面，21 号票）。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::host::AppState;
use crate::host::IpcError;
use crate::safety::safe_join::PathGuard;
use crate::util::encoding::{MAX_RESPONSE_BYTES, decode_text, is_probably_binary};
use crate::util::errors::ToolError;

/// 会话 roots 解析（§6.2/§9.1，C-8 复判收窄，21 号票）：
///
/// - 项目会话：两根 = 项目主目录 + `<data_root>/workspace/`；
/// - 临时会话（含未知会话兜底）：两根同目录 = `<data_root>/workspace/`。
///
/// 返回 (project_root, data_root)——第二根语义为「数据目录下的 agent
/// workspace 子目录」，PathGuard 构造（tools guard / read_workspace_file）
/// 与 @ 补全遍历都以它为 roots：数据目录本体（config.json/sessions/ 等存储
/// 内部文件）退出模型文件工具与 @ 补全的可见面。目录不在这里落盘创建，
/// 由使用方 create_dir_all（chat.rs 既有习惯）或按不存在口径报错。
pub(crate) fn resolve_session_roots(app: &AppState, session_id: &str) -> (PathBuf, PathBuf) {
    let workspace_root = app.store.root().join("workspace");
    let project_root = app
        .store
        .index()
        .get(session_id)
        .and_then(|meta| meta.project_id)
        .and_then(|pid| app.projects.home_of(&pid))
        .unwrap_or_else(|| workspace_root.clone());
    (project_root, workspace_root)
}

/// ToolError → D14 错误码（safe_join 语义原样透传）。
fn to_ipc_error(err: ToolError) -> IpcError {
    IpcError::new(err.code, err.message)
}

/// `search_workspace_paths` 参数契约（§3.3 #27）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchWorkspacePathsArgs {
    pub session_id: String,
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// 路径命中条目（§3.3 #27：`{ path, kind }`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspacePathHit {
    pub path: String,
    pub kind: String,
}

/// 遍历条目数上限（防大仓库卡死；到顶即截断返回已命中部分）。
const SEARCH_MAX_ENTRIES: usize = 20_000;
/// 默认返回条数（@ 菜单单屏容量）。
const SEARCH_DEFAULT_LIMIT: usize = 20;
/// 搜索跳过的目录名（依赖目录/构建产物/宿主状态——无索引的暴力遍历不进这些树）。
const SEARCH_SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    ".craftwave",
];

/// @ 引用补全主体（公开给测试；命令层只解 tauri State）。
pub(crate) fn search_workspace_paths_core(
    app: &AppState,
    session_id: &str,
    query: &str,
    limit: Option<usize>,
) -> Result<Vec<WorkspacePathHit>, IpcError> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.unwrap_or(SEARCH_DEFAULT_LIMIT).clamp(1, 100);
    let (project_root, data_root) = resolve_session_roots(app, session_id);

    let mut hits: Vec<WorkspacePathHit> = Vec::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let project_canon = fs::canonicalize(&project_root).unwrap_or_else(|_| project_root.clone());
    for root in [project_root, data_root] {
        let Ok(canon) = fs::canonicalize(&root) else {
            continue;
        };
        if !visited.insert(canon.clone()) {
            continue; // 临时会话两根同目录，只扫一遍
        }
        // 相对路径口径基于 canonical 根（macOS /var ↔ /private/var 符号链接）
        search_under(&canon, &project_canon, &needle, limit, &mut hits);
        if hits.len() >= limit {
            break;
        }
    }
    hits.truncate(limit);
    Ok(hits)
}

/// @ 引用补全（§3.3 #27）：BFS 遍历会话 roots，基名大小写不敏感子串匹配。
#[tauri::command]
pub(crate) async fn search_workspace_paths(
    app: State<'_, AppState>,
    session_id: String,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<WorkspacePathHit>, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = SearchWorkspacePathsArgs {
        session_id,
        query,
        limit,
    };
    search_workspace_paths_core(&app, &args.session_id, &args.query, args.limit)
}

/// 递归遍历（迭代栈防深目录爆栈）：基名子串命中即收，条目预算内截止。
fn search_under(
    dir: &Path,
    project_root: &Path,
    needle: &str,
    limit: usize,
    hits: &mut Vec<WorkspacePathHit>,
) {
    let mut budget = SEARCH_MAX_ENTRIES;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if hits.len() >= limit || budget == 0 {
            return;
        }
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            if budget == 0 || hits.len() >= limit {
                return;
            }
            budget -= 1;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
            if is_dir {
                if SEARCH_SKIP_DIRS.contains(&name) {
                    continue;
                }
                stack.push(path.clone());
            }
            if name.to_lowercase().contains(needle) {
                hits.push(WorkspacePathHit {
                    path: display_path(&path, project_root),
                    kind: if is_dir { "dir" } else { "file" }.to_owned(),
                });
            }
        }
    }
}

/// 展示路径：项目主目录下返回相对路径（read 工具解析基准），否则绝对路径。
/// 相对路径分隔符统一正斜杠（Windows 下 PathBuf 默认反斜杠；正斜杠两侧
/// 工具链均接受，且与前端 @ 补全插入形态一致——16 号票口径的平台补全）。
fn display_path(path: &Path, project_root: &Path) -> String {
    if let Ok(rel) = path.strip_prefix(project_root)
        && !rel.as_os_str().is_empty()
    {
        return rel.to_string_lossy().replace('\\', "/");
    }
    path.to_string_lossy().into_owned()
}

/// `read_workspace_file` 参数契约（§3.3 #28：range 按行翻页）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadWorkspaceFileArgs {
    pub session_id: String,
    pub path: String,
    #[serde(default)]
    pub range: Option<ReadRange>,
}

/// 行区间（offset=起始行 0 基，limit=行数）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadRange {
    pub offset: u64,
    pub limit: u64,
}

/// `read_workspace_file` 返回契约（§3.3 #28：`{ content, truncated }`）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadWorkspaceResult {
    pub content: String,
    pub truncated: bool,
}

/// 预览读取主体（公开给测试；命令层只解 tauri State）。
pub(crate) fn read_workspace_file_core(
    app: &AppState,
    session_id: &str,
    path: &str,
    offset: Option<u64>,
    limit: Option<u64>,
) -> Result<ReadWorkspaceResult, IpcError> {
    let range = match (offset, limit) {
        (Some(offset), Some(limit)) => Some(ReadRange { offset, limit }),
        _ => None,
    };
    let (project_root, data_root) = resolve_session_roots(app, session_id);
    // roots 需真实存在（canonicalize）：项目目录按需补建；workspace 子目录
    // 首次使用即建（chat.rs 同款习惯，21 号票收窄后的第二根）
    let _ = fs::create_dir_all(&project_root);
    let _ = fs::create_dir_all(&data_root);
    let guard = PathGuard::new(vec![project_root, data_root]).map_err(to_ipc_error)?;
    let abs = guard.resolve(path).map_err(to_ipc_error)?;

    let bytes = match fs::read(&abs) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(IpcError::not_found(format!("文件不存在：{path}")));
        }
        Err(e) => return Err(IpcError::internal(format!("文件读取失败：{e}"))),
    };
    if is_probably_binary(&bytes) {
        return Err(IpcError::invalid_arg(format!(
            "二进制文件不支持文本预览：{path}"
        )));
    }
    let decoded = decode_text(&bytes);
    let lines: Vec<&str> = decoded.text.lines().collect();
    let (start, end) = match &range {
        Some(range) => {
            let start = (range.offset as usize).min(lines.len());
            let end = (start + range.limit as usize).min(lines.len());
            (start, end)
        }
        None => (0, lines.len()),
    };
    let mut content = lines[start..end].join("\n");
    let mut truncated = start > 0 || end < lines.len();
    if content.len() > MAX_RESPONSE_BYTES {
        // 按字符边界截断到上限（避免撕裂多字节 UTF-8）
        let mut cut = MAX_RESPONSE_BYTES;
        while !content.is_char_boundary(cut) {
            cut -= 1;
        }
        content.truncate(cut);
        truncated = true;
    }
    Ok(ReadWorkspaceResult { content, truncated })
}

/// 预览读取（§3.3 #28）：roots 内校验 → 编码自动判断（与 read 工具共用）→
/// 前 8KB NUL 判二进制 → 行区间切片 → 512KB 截断。
#[tauri::command]
pub(crate) async fn read_workspace_file(
    app: State<'_, AppState>,
    session_id: String,
    path: String,
    range: Option<ReadRange>,
) -> Result<ReadWorkspaceResult, IpcError> {
    let args = ReadWorkspaceFileArgs {
        session_id,
        path,
        range,
    };
    read_workspace_file_core(
        &app,
        &args.session_id,
        &args.path,
        args.range.as_ref().map(|r| r.offset),
        args.range.as_ref().map(|r| r.limit),
    )
}
