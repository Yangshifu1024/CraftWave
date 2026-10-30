//! 工作区只读域命令（§3.3 #27 `search_workspace_paths` / #28
//! `read_workspace_file`，16 号票）+ 外部目录授权（§3.3 #33
//! `check_external_path` / #34 `allow_external_dir`，24 号票）。
//!
//! 路径校验复用 safety::PathGuard（§9.1 safe_join 单一实现；C-8 收窄后
//! roots = 项目主目录 + 数据目录下 `workspace/` 子目录，21 号票；24 号票起
//! 追加会话级授权目录——`authorized_external_dirs` 持久于 SessionMeta，
//! §9.9「授权持久于会话 roots 快照，不全局生效」）；解码复用
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
use crate::util::errors::{E_PATH_OUTSIDE_ROOTS, ToolError};

/// 会话 roots 解析（§6.2/§9.1，C-8 复判收窄，21 号票；24 号票扩授权目录）：
///
/// - 项目会话：两根 = 项目主目录 + `<data_root>/workspace/`；
/// - 临时会话（含未知会话兜底）：两根同目录 = `<data_root>/workspace/`；
/// - 第三集合 = SessionMeta 授权的外部目录（§9.9 会话 roots 快照：#34
///   「始终允许」并入后随 index.json 持久化，会话内跨 run 生效、跨会话
///   不延续；Q41 备份恢复由恢复流程清空）。
///
/// 数据目录本体（config.json/sessions/ 等存储内部文件）退出模型文件工具
/// 与 @ 补全的可见面。目录不在这里落盘创建，由使用方 create_dir_all
/// （chat.rs 既有习惯）或按不存在口径报错。
#[derive(Debug, Clone)]
pub(crate) struct SessionRoots {
    /// 项目主目录（相对路径解析基准与首个 root；临时会话 = workspace 根）。
    pub project_root: PathBuf,
    /// 数据目录下的 workspace 子目录（第二根）。
    pub workspace_root: PathBuf,
    /// 已授权外部目录（canonical 绝对路径；授权记录按字符串保留，此处
    /// canonicalize 失败的条目跳过——PathGuard 对不存在的根会报错）。
    pub extra_dirs: Vec<PathBuf>,
}

// SessionRoots 的根集合方法。
impl SessionRoots {
    /// 全部根（PathGuard 构造序：项目主目录在前——相对路径解析基准）。
    pub fn all(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.project_root.clone(), self.workspace_root.clone()];
        roots.extend(self.extra_dirs.iter().cloned());
        roots
    }

    /// 初始两根（不含授权目录；#33 insideRoots 判定口径）。
    pub fn core(&self) -> Vec<PathBuf> {
        vec![self.project_root.clone(), self.workspace_root.clone()]
    }
}

/// 会话 roots 解析（含授权目录；命令面与 run 装配共用）。
pub(crate) fn resolve_session_roots(app: &AppState, session_id: &str) -> SessionRoots {
    let workspace_root = app.store.root().join("workspace");
    let project_root = app
        .store
        .index()
        .get(session_id)
        .and_then(|meta| meta.project_id)
        .and_then(|pid| app.projects.home_of(&pid))
        .unwrap_or_else(|| workspace_root.clone());
    let extra_dirs = app
        .store
        .index()
        .get(session_id)
        .map(|meta| {
            meta.authorized_external_dirs
                .iter()
                .filter_map(|d| fs::canonicalize(d).ok())
                .collect()
        })
        .unwrap_or_default();
    SessionRoots {
        project_root,
        workspace_root,
        extra_dirs,
    }
}

/// ToolError → D14 错误码（safe_join 语义原样透传）。
fn to_ipc_error(err: ToolError) -> IpcError {
    IpcError::new(err.code, err.message)
}

/// `search_workspace_paths` 参数契约（§3.3 #27）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SearchWorkspacePathsArgs {
    pub session_id: String,
    pub query: String,
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub limit: Option<usize>,
}

/// 路径命中条目（§3.3 #27：`{ path, kind }`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
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
    // 性能埋点（28 号票验收：万级文件库 P95 < 300ms）：单次搜索全耗时，
    // 超过 100ms 才输出 warn（避免正常小目录的日志噪音）。
    let started = std::time::Instant::now();
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.unwrap_or(SEARCH_DEFAULT_LIMIT).clamp(1, 100);
    let roots = resolve_session_roots(app, session_id);

    let mut hits: Vec<WorkspacePathHit> = Vec::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let project_canon =
        fs::canonicalize(&roots.project_root).unwrap_or_else(|_| roots.project_root.clone());
    for root in roots.all() {
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
    let elapsed = started.elapsed();
    if elapsed.as_millis() > 100 {
        eprintln!(
            "[craftwave] @ 补全搜索耗时 {elapsed_ms}ms（session={session_id} query={query:?} 命中 {}）",
            hits.len(),
            elapsed_ms = elapsed.as_millis()
        );
    }
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
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ReadWorkspaceFileArgs {
    pub session_id: String,
    pub path: String,
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub range: Option<ReadRange>,
}

/// 行区间（offset=起始行 0 基，limit=行数）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ReadRange {
    pub offset: u64,
    pub limit: u64,
}

/// `read_workspace_file` 返回契约（§3.3 #28：`{ content, truncated }`）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
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
    let roots = resolve_session_roots(app, session_id);
    // roots 需真实存在（canonicalize）：项目目录按需补建；workspace 子目录
    // 首次使用即建（chat.rs 同款习惯，21 号票收窄后的第二根）
    let _ = fs::create_dir_all(&roots.project_root);
    let _ = fs::create_dir_all(&roots.workspace_root);
    let guard = PathGuard::new(roots.all()).map_err(to_ipc_error)?;
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

// ---- 拖拽路径探测（§3.3 #59j，17 号票）----

/// `path_kind` 参数契约（§3.3 #59j）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PathKindArgs {
    pub path: String,
}

/// `path_kind` 返回契约（§3.3 #59j）：kind 三态——dir = 目录（拖拽路由弹
/// 「添加为项目」确认）、file = 普通文件（转 @ 路径引用）、missing = 不存在
/// （忽略）；imageMime = 按扩展名识别出的图片 MIME（png/jpeg/gif/webp，
/// 拖拽路由据此走图片附件），非图片省略。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PathKindResult {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub image_mime: Option<&'static str>,
}

/// 按扩展名识别图片 MIME（拖拽入口无字节可嗅探，扩展名是唯一线索；
/// 内容层面的白名单校验仍由 attach_image 落盘时的魔数嗅探兜底）。
fn image_mime_by_extension(path: &str) -> Option<&'static str> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// 拖拽路径探测（§3.3 #59j，17 号票）：判定拖入路径是目录还是文件（symlink
/// 追随语义），并按扩展名识别图片 MIME。纯探测语义——不做 roots 校验、
/// 不泄露内容，前端拖拽路由（目录提议添加项目 / 图片转附件 / 普通文件转
/// 路径引用）据此分流；普通文件的模型侧可见性仍由 read 管线的授权链把关
/// （roots 外路径不因拖入自动放开，FR-13-16）。
pub fn path_kind_core(path: &str) -> PathKindResult {
    let mime = image_mime_by_extension(path);
    let kind = match fs::metadata(path) {
        Ok(meta) if meta.is_dir() => "dir",
        Ok(_) => "file",
        Err(_) => "missing",
    };
    PathKindResult {
        kind,
        image_mime: mime,
    }
}

/// `path_kind` 命令（§3.3 #59j）。
#[tauri::command]
pub(crate) async fn path_kind(path: String) -> Result<PathKindResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = PathKindArgs { path };
    if args.path.trim().is_empty() {
        return Err(IpcError::invalid_arg("path 不能为空"));
    }
    Ok(path_kind_core(&args.path))
}

// ---- 外部目录授权（§3.3 #33/#34，FR-05-12/§9.9，24 号票）----

/// `check_external_path` 参数契约（§3.3 #33）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CheckExternalPathArgs {
    pub session_id: String,
    pub path: String,
}

/// `check_external_path` 返回契约（§3.3 #33 逐字段）：insideRoots = 路径在
/// 初始两根（项目主目录 + workspace）内（无需授权）；allowed = 路径当前
/// 可访问（初始两根内，或已授权目录内）。外部未授权 → allowed=false,
/// insideRoots=false。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CheckExternalPathResult {
    pub allowed: bool,
    pub inside_roots: bool,
}

/// `check_external_path` 主体：按会话 roots 全集（含授权目录）做 safe_join
/// 同款解析；解析结果落在初始两根下 → insideRoots。会话未知按临时会话
/// 兜底（与 roots 解析一致，幂等查询语义）。
pub(crate) fn check_external_path_core(
    app: &AppState,
    session_id: &str,
    path: &str,
) -> Result<CheckExternalPathResult, IpcError> {
    let roots = resolve_session_roots(app, session_id);
    let core: Vec<PathBuf> = roots
        .core()
        .iter()
        .filter_map(|r| fs::canonicalize(r).ok())
        .collect();
    let guard = PathGuard::new(roots.all()).map_err(to_ipc_error)?;
    match guard.resolve(path) {
        Ok(resolved) => {
            let inside_roots = core
                .iter()
                .any(|r| resolved == *r || resolved.starts_with(r));
            Ok(CheckExternalPathResult {
                allowed: true,
                inside_roots,
            })
        }
        Err(err) if err.code == E_PATH_OUTSIDE_ROOTS => Ok(CheckExternalPathResult {
            allowed: false,
            inside_roots: false,
        }),
        Err(err) => Err(to_ipc_error(err)),
    }
}

/// `check_external_path` 命令（§3.3 #33）。
#[tauri::command]
pub(crate) async fn check_external_path(
    app: State<'_, AppState>,
    args: CheckExternalPathArgs,
) -> Result<CheckExternalPathResult, IpcError> {
    check_external_path_core(&app, &args.session_id, &args.path)
}

/// `allow_external_dir` 参数契约（§3.3 #34）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AllowExternalDirArgs {
    pub session_id: String,
    pub dir: String,
}

/// `allow_external_dir` 返回契约（§3.3 #34）：授权后的会话 roots 全集
/// （初始两根 + 全部已授权外部目录，canonical 字符串）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ExternalRootsResult {
    pub roots: Vec<String>,
}

/// 当前用户家目录（HOME / USERPROFILE；system.rs 展开同一口径）。
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

/// 授权目标是否属「整体放行」禁区（§9.9，命中即 E_INVALID_ARG 拒绝）：
/// 文件系统根、家目录本身、宿主托管目录（`~/.craftwave` = 数据目录）。
/// 从严口径：家目录的**祖先**（比家目录粒度更大）与数据目录的**祖先及
/// 后代**一并拒绝——授权托管目录的子目录会拆穿 C-8 收窄防线（数据目录
/// 本体退出 agent 可见面），家目录以上粒度违背「目录及子目录」的授权
/// 精神。项目主目录恰在数据目录内属于配置异常，同样被拒（防线优先）。
pub(crate) fn is_protected_grant_target(dir: &Path, data_dir: &Path) -> bool {
    // 文件系统根：除 RootDir/Prefix 外没有任何组件。
    let is_root = dir.components().all(|c| {
        matches!(
            c,
            std::path::Component::RootDir | std::path::Component::Prefix(_)
        )
    });
    if is_root {
        return true;
    }
    if let Some(home) = home_dir() {
        // 家目录本身，或目录是家目录的祖先（粒度 ≥ 家目录）。
        if dir == home || home.starts_with(dir) {
            return true;
        }
    }
    // 数据目录本身、其祖先（粒度 ≥ 托管目录）或其内部（拆穿 C-8 收窄）。
    dir == data_dir || data_dir.starts_with(dir) || dir.starts_with(data_dir)
}

/// 会话 roots 全集字符串（授权后的返回面；canonical 失败的根按原路径
/// 保留——展示面尽力而为）。
fn roots_as_strings(roots: &SessionRoots) -> Vec<String> {
    roots
        .all()
        .iter()
        .map(|r| {
            fs::canonicalize(r)
                .unwrap_or_else(|_| r.clone())
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// `allow_external_dir` 主体：校验禁授权目标 → canonical → 并入
/// SessionMeta 授权表（§9.9 会话 roots 快照持久化）→ 返回 roots 全集。
/// 幂等：已授权的目录重复授权直接返回现状。
pub(crate) fn allow_external_dir_core(
    app: &AppState,
    session_id: &str,
    dir: &str,
) -> Result<ExternalRootsResult, IpcError> {
    if dir.trim().is_empty() {
        return Err(IpcError::invalid_arg("dir 不能为空"));
    }
    // 会话必须存在：授权持久化在 SessionMeta 上，未知会话无处可写。
    if app.store.index().get(session_id).is_none() {
        return Err(IpcError::not_found(format!("会话不存在：{session_id}")));
    }
    let canonical = fs::canonicalize(dir).map_err(|e| {
        IpcError::invalid_arg(format!("授权目录无法解析（须真实存在）：{dir}：{e}"))
    })?;
    // 禁授权目标判定统一在 canonical 域（macOS /var ↔ /private/var symlink
    // 歧义：数据目录根与目标同域归一后才可比较，否则禁区判定被符号链接
    // 绕过）。
    let data_canonical =
        fs::canonicalize(app.store.root()).unwrap_or_else(|_| app.store.root().to_path_buf());
    if is_protected_grant_target(&canonical, &data_canonical) {
        return Err(IpcError::invalid_arg(
            "拒绝整体放行：不允许授权文件系统根、家目录本身或宿主托管目录（~/.craftwave）及其上/下范围",
        ));
    }
    let text = canonical.to_string_lossy().into_owned();
    let updated = app
        .store
        .index()
        .mutate(session_id, |meta| {
            if !meta.authorized_external_dirs.iter().any(|d| d == &text) {
                meta.authorized_external_dirs.push(text.clone());
            }
        })
        .map_err(|e| IpcError::internal(format!("会话索引写入失败：{e:?}")))?
        .ok_or_else(|| IpcError::not_found(format!("会话不存在：{session_id}")))?;
    let roots = SessionRoots {
        project_root: resolve_session_roots(app, session_id).project_root,
        workspace_root: app.store.root().join("workspace"),
        extra_dirs: updated
            .authorized_external_dirs
            .iter()
            .filter_map(|d| fs::canonicalize(d).ok())
            .collect(),
    };
    Ok(ExternalRootsResult {
        roots: roots_as_strings(&roots),
    })
}

/// `allow_external_dir` 命令（§3.3 #34）。
#[tauri::command]
pub(crate) async fn allow_external_dir(
    app: State<'_, AppState>,
    args: AllowExternalDirArgs,
) -> Result<ExternalRootsResult, IpcError> {
    allow_external_dir_core(&app, &args.session_id, &args.dir)
}

/// `revoke_external_dir` 参数契约（24 号票补充面，§9.9 撤销入口：Inspector
/// 已授权目录列表的移除按钮）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct RevokeExternalDirArgs {
    pub session_id: String,
    pub dir: String,
}

/// `revoke_external_dir` 主体：按 canonical 字符串移除一条授权（不存在不
/// 报错——幂等撤销）；返回剩余 roots 全集。移除后下次访问重新询问（§9.9）。
pub(crate) fn revoke_external_dir_core(
    app: &AppState,
    session_id: &str,
    dir: &str,
) -> Result<ExternalRootsResult, IpcError> {
    if dir.trim().is_empty() {
        return Err(IpcError::invalid_arg("dir 不能为空"));
    }
    if app.store.index().get(session_id).is_none() {
        return Err(IpcError::not_found(format!("会话不存在：{session_id}")));
    }
    // 输入允许非 canonical 形态：先 canonicalize 对齐存储口径；失败则按
    // 原串匹配（目录已被删的场景也能撤掉遗留授权）。
    let canonical = fs::canonicalize(dir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| dir.to_owned());
    let updated = app
        .store
        .index()
        .mutate(session_id, |meta| {
            meta.authorized_external_dirs.retain(|d| d != &canonical);
        })
        .map_err(|e| IpcError::internal(format!("会话索引写入失败：{e:?}")))?
        .ok_or_else(|| IpcError::not_found(format!("会话不存在：{session_id}")))?;
    let roots = SessionRoots {
        project_root: resolve_session_roots(app, session_id).project_root,
        workspace_root: app.store.root().join("workspace"),
        extra_dirs: updated
            .authorized_external_dirs
            .iter()
            .filter_map(|d| fs::canonicalize(d).ok())
            .collect(),
    };
    Ok(ExternalRootsResult {
        roots: roots_as_strings(&roots),
    })
}

/// `revoke_external_dir` 命令（24 号票补充面）。
#[tauri::command]
pub(crate) async fn revoke_external_dir(
    app: State<'_, AppState>,
    args: RevokeExternalDirArgs,
) -> Result<ExternalRootsResult, IpcError> {
    revoke_external_dir_core(&app, &args.session_id, &args.dir)
}

/// Q41 恢复授权接线面：清空**全部会话**的外部目录授权（33 号票备份恢复
/// 流程调用——「恢复不自动启用备份中的外部 roots」；普通重启不调用）。
/// 返回清理的会话条数。资源信任（trust/project.json）的同等清空面在
/// [`crate::storage::trust::TrustStore::clear_all`]。
/// 本票只出接口与测试（33 号票未动工），生产调用点随 33 号票接线。
#[allow(dead_code)]
pub(crate) fn clear_all_external_grants(app: &AppState) -> Result<usize, IpcError> {
    let mut cleared = 0usize;
    for meta in app.store.index().list() {
        if meta.authorized_external_dirs.is_empty() {
            continue;
        }
        app.store
            .index()
            .mutate(&meta.id, |m| m.authorized_external_dirs.clear())
            .map_err(|e| IpcError::internal(format!("会话索引写入失败：{e:?}")))?;
        cleared += 1;
    }
    Ok(cleared)
}
