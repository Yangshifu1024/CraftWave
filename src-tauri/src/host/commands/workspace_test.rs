//! workspace 域命令测试（独立测试文件）：会话 roots 解析（临时/项目双形态，
//! C-8 收窄后第二根 = 数据目录下 workspace/ 子目录，21 号票）、路径搜索
//! （跳过目录/大小写不敏感/相对路径口径）、预览读取（range 翻页/512KB 截断/
//! 二进制拒绝/越界拒绝/存储内部文件不可达）。

use std::fs;
use std::sync::Arc;

use super::workspace::{
    read_workspace_file_core, resolve_session_roots, search_workspace_paths_core,
};
use crate::host::AppState;
use crate::storage::SessionMeta;
use crate::util::events::RecordingEventSink;

/// 测试脚手架：在临时数据目录上建一份 AppState（事件汇用录制桩，无真实副作用）。
fn app(data: &std::path::Path) -> AppState {
    AppState::new_in(data.to_path_buf(), Arc::new(RecordingEventSink::new()))
}

/// 造一条会话索引（字段值本身无关紧要，只关心 id 与项目归属）。
fn meta(id: &str, project_id: Option<&str>) -> SessionMeta {
    SessionMeta {
        id: id.to_string(),
        project_id: project_id.map(str::to_string),
        title: "会话".to_string(),
        created_at: 0,
        updated_at: 0,
        revision: 0,
        message_count: 0,
        total_tokens: 0,
    }
}

/// 项目会话 roots：project_root=项目主目录，data_root=数据目录下
/// `workspace/` 子目录（C-8 收窄，21 号票）。
#[test]
fn resolve_roots_project_session_uses_home_dir() {
    let dir = tempfile::tempdir().unwrap();
    let state = app(dir.path());
    let home = tempfile::tempdir().unwrap();
    let project = state
        .projects
        .save(None, "项目", home.path().to_string_lossy().as_ref())
        .unwrap();
    state
        .store
        .index()
        .upsert(meta("s-proj", Some(&project.id)))
        .unwrap();

    let (project_root, data_root) = resolve_session_roots(&state, "s-proj");
    assert_eq!(
        fs::canonicalize(&project_root).unwrap(),
        fs::canonicalize(home.path()).unwrap()
    );
    // roots 不在此落盘创建（使用方 create_dir_all，chat.rs 既有习惯）——先建再比对
    fs::create_dir_all(dir.path().join("workspace")).unwrap();
    assert_eq!(
        fs::canonicalize(data_root).unwrap(),
        fs::canonicalize(dir.path().join("workspace")).unwrap()
    );
}

/// 临时会话 roots：两根同为数据目录下 `workspace/` 子目录（§6.2，C-8 收窄）。
#[test]
fn resolve_roots_temp_session_two_roots_at_workspace_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let state = app(dir.path());
    state.store.index().upsert(meta("s-temp", None)).unwrap();
    let (project_root, data_root) = resolve_session_roots(&state, "s-temp");
    assert_eq!(project_root, data_root, "临时会话两根同目录");
    fs::create_dir_all(dir.path().join("workspace")).unwrap();
    assert_eq!(
        fs::canonicalize(data_root).unwrap(),
        fs::canonicalize(dir.path().join("workspace")).unwrap()
    );
    // 数据目录本体（存储内部文件所在层）不在 roots 内
    assert_ne!(
        fs::canonicalize(&project_root).unwrap(),
        fs::canonicalize(dir.path()).unwrap(),
        "数据目录根不再作为 agent 可见根（config.json 等退出可见面）"
    );
}

/// 未知会话按临时口径（防御：index 无条目不 panic），两根落 workspace 子目录。
#[test]
fn resolve_roots_unknown_session_falls_back_to_workspace_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let state = app(dir.path());
    let (project_root, data_root) = resolve_session_roots(&state, "幽灵会话");
    assert_eq!(project_root, data_root);
    fs::create_dir_all(dir.path().join("workspace")).unwrap();
    assert_eq!(
        fs::canonicalize(data_root).unwrap(),
        fs::canonicalize(dir.path().join("workspace")).unwrap()
    );
}

/// 搜索：基名大小写不敏感、跳过 .git/node_modules、项目主目录下返回相对路径。
#[test]
fn search_paths_matches_basename_and_skips_noise() {
    let data = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join("src/components")).unwrap();
    fs::write(home.path().join("src/Button.tsx"), b"").unwrap();
    fs::write(home.path().join("src/components/IconButton.tsx"), b"").unwrap();
    fs::create_dir_all(home.path().join(".git/hooks")).unwrap();
    fs::write(home.path().join(".git/hooks/Button-hook"), b"").unwrap();
    fs::create_dir_all(home.path().join("node_modules/x")).unwrap();
    fs::write(home.path().join("node_modules/x/Button.js"), b"").unwrap();
    // 第二根（workspace 子目录）内条目命中且绝对路径返回；数据目录本体的
    // 文件（如 config.json）不在 roots 内，不进命中（C-8 收窄，21 号票）
    fs::create_dir_all(data.path().join("workspace")).unwrap();
    fs::write(data.path().join("workspace/外部的Button.md"), b"").unwrap();
    fs::write(data.path().join("config.json"), b"{}").unwrap();
    fs::write(data.path().join("数据目录根的Button.txt"), b"").unwrap();

    let state = app(data.path());
    let project = state
        .projects
        .save(None, "搜索项目", home.path().to_string_lossy().as_ref())
        .unwrap();
    state
        .store
        .index()
        .upsert(meta("s-search", Some(&project.id)))
        .unwrap();

    let hits = search_workspace_paths_core(&state, "s-search", "button", None).unwrap();
    let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
    // 项目主目录内 → 相对路径；noise 目录不进；workspace 子目录命中 → 绝对路径
    assert!(paths.contains(&"src/Button.tsx"), "实际 {paths:?}");
    assert!(paths.contains(&"src/components/IconButton.tsx"));
    assert!(
        paths
            .iter()
            .all(|p| !p.contains(".git") && !p.contains("node_modules"))
    );
    assert_eq!(
        paths.iter().filter(|p| p.ends_with(".md")).count(),
        1,
        "workspace 子目录条目绝对路径返回"
    );
    assert!(
        paths
            .iter()
            .all(|p| !p.contains("config.json") && !p.contains("数据目录根")),
        "数据目录本体文件不进 @ 补全命中（收窄语义）：{paths:?}"
    );
    assert!(hits.iter().all(|h| h.kind == "file"));
}

/// 搜索：目录也命中（kind=dir）、limit 生效、空 query 返回空（临时会话
/// 遍历的是 workspace 子目录，C-8 收窄）。
#[test]
fn search_paths_dirs_limit_and_empty_query() {
    let data = tempfile::tempdir().unwrap();
    let state = app(data.path());
    fs::create_dir_all(data.path().join("workspace/components/inner")).unwrap();

    let hits = search_workspace_paths_core(&state, "任意会话", "comp", None).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].kind, "dir");

    let capped = search_workspace_paths_core(&state, "任意会话", "comp", Some(1)).unwrap();
    assert_eq!(capped.len(), 1);

    assert!(
        search_workspace_paths_core(&state, "任意会话", "  ", None)
            .unwrap()
            .is_empty()
    );
}

/// 预览读取：全文、range 按行翻页、truncated 口径（文件落 workspace 子目录，
/// 临时会话相对路径基准即 workspace）。
#[test]
fn read_preview_full_and_paged() {
    let data = tempfile::tempdir().unwrap();
    fs::create_dir_all(data.path().join("workspace")).unwrap();
    fs::write(data.path().join("workspace/note.txt"), "一\n二\n三\n四\n五").unwrap();
    let state = app(data.path());

    let full = read_workspace_file_core(&state, "任意会话", "note.txt", None, None).unwrap();
    assert_eq!(full.content, "一\n二\n三\n四\n五");
    assert!(!full.truncated);

    let paged = read_workspace_file_core(&state, "任意会话", "note.txt", Some(1), Some(2)).unwrap();
    assert_eq!(paged.content, "二\n三");
    assert!(paged.truncated, "未读到尾即 truncated");
}

/// 预览读取：512KB 字节上限截断（按字符边界）。
#[test]
fn read_preview_truncates_beyond_512kb() {
    let data = tempfile::tempdir().unwrap();
    let big = "汉".repeat(200_000); // 600KB UTF-8
    fs::create_dir_all(data.path().join("workspace")).unwrap();
    fs::write(data.path().join("workspace/big.txt"), &big).unwrap();
    let state = app(data.path());

    let result = read_workspace_file_core(&state, "任意会话", "big.txt", None, None).unwrap();
    assert!(result.content.len() <= 512 * 1024);
    assert!(result.truncated);
    assert!(result.content.ends_with('汉'), "按字符边界截断");
}

/// 预览读取：前 8KB 含 NUL 判二进制拒绝；缺失文件 E_NOT_FOUND；越界路径拒绝；
/// 存储内部文件不可达（C-8 收窄验收，21 号票：临时会话对 config.json 双口径
/// 都拿不到内容——相对名解析进 workspace 报不存在，`..` 越界报路径拒绝）。
#[test]
fn read_preview_rejects_binary_missing_and_outside() {
    let data = tempfile::tempdir().unwrap();
    fs::create_dir_all(data.path().join("workspace")).unwrap();
    fs::write(data.path().join("workspace/bin.dat"), [0u8, 1, 2, 3]).unwrap();
    fs::write(data.path().join("config.json"), b"{\"api_key\":\"secret\"}").unwrap();
    let state = app(data.path());

    let err = read_workspace_file_core(&state, "任意会话", "bin.dat", None, None).unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");

    let err = read_workspace_file_core(&state, "任意会话", "missing.txt", None, None).unwrap_err();
    assert_eq!(err.code, "E_NOT_FOUND");

    let err =
        read_workspace_file_core(&state, "任意会话", "../escape.txt", None, None).unwrap_err();
    assert_eq!(err.code, "E_PATH_OUTSIDE_ROOTS");

    // 存储内部文件不可达：`../config.json` 越 workspace 出 roots；裸名
    // 「config.json」解析进 workspace（不存在）而非数据目录根
    let err =
        read_workspace_file_core(&state, "任意会话", "../config.json", None, None).unwrap_err();
    assert_eq!(err.code, "E_PATH_OUTSIDE_ROOTS", "数据目录本体越界拒绝");
    let err = read_workspace_file_core(&state, "任意会话", "config.json", None, None).unwrap_err();
    assert_eq!(
        err.code, "E_NOT_FOUND",
        "裸名不会命中数据目录根的 config.json"
    );
}

/// 项目会话下 workspace 子目录内文件可经绝对路径读取（第二根放行，
/// C-8 收窄验收「workspace 子目录内正常读写」的读侧锚点）。
#[test]
fn read_preview_allows_workspace_subdir_file_in_project_session() {
    let data = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(data.path().join("workspace")).unwrap();
    fs::write(data.path().join("workspace/共享.md"), "工作区内容").unwrap();
    let state = app(data.path());
    let project = state
        .projects
        .save(None, "项目", home.path().to_string_lossy().as_ref())
        .unwrap();
    state
        .store
        .index()
        .upsert(meta("s-ws", Some(&project.id)))
        .unwrap();

    let abs = data.path().join("workspace/共享.md");
    let result =
        read_workspace_file_core(&state, "s-ws", abs.to_str().unwrap(), None, None).unwrap();
    assert_eq!(
        result.content, "工作区内容",
        "workspace 第二根内绝对路径放行"
    );
    // 项目主目录外、workspace 外的绝对路径仍拒绝
    let err = read_workspace_file_core(
        &state,
        "s-ws",
        data.path().join("config.json").to_str().unwrap(),
        None,
        None,
    )
    .unwrap_err();
    assert_eq!(err.code, "E_PATH_OUTSIDE_ROOTS", "数据目录本体绝对路径拒绝");
}
