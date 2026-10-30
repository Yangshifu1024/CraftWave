//! workspace 域命令测试（独立测试文件）：会话 roots 解析（临时/项目双形态，
//! C-8 收窄后第二根 = 数据目录下 workspace/ 子目录，21 号票）、路径搜索
//! （跳过目录/大小写不敏感/相对路径口径）、预览读取（range 翻页/512KB 截断/
//! 二进制拒绝/越界拒绝/存储内部文件不可达）。

use std::fs;
use std::sync::Arc;

use super::workspace::{
    allow_external_dir_core, check_external_path_core, clear_all_external_grants,
    read_workspace_file_core, resolve_session_roots, revoke_external_dir_core,
    search_workspace_paths_core,
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
        authorized_external_dirs: Vec::new(),
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

    let roots = resolve_session_roots(&state, "s-proj");
    let (project_root, data_root) = (roots.project_root, roots.workspace_root);
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
    let roots = resolve_session_roots(&state, "s-temp");
    let (project_root, data_root) = (roots.project_root, roots.workspace_root);
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
    let roots = resolve_session_roots(&state, "幽灵会话");
    let (project_root, data_root) = (roots.project_root, roots.workspace_root);
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

// ---- 外部目录授权（§3.3 #33/#34，24 号票）----

/// #33 三态：初始两根内（insideRoots=true, allowed=true）/ 已授权目录
/// （insideRoots=false, allowed=true）/ 未授权外部（both=false）。
#[test]
fn check_external_path_three_states() {
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
        .upsert(meta("s-1", Some(&project.id)))
        .unwrap();
    fs::create_dir_all(home.path().join("src")).unwrap();
    fs::create_dir_all(dir.path().join("workspace")).unwrap();
    // 外部目录（数据目录外，供授权）
    let external = tempfile::tempdir().unwrap();

    // 初始两根内：allowed + insideRoots
    let r = check_external_path_core(&state, "s-1", "src/lib.rs").unwrap();
    assert!(r.allowed && r.inside_roots, "项目主目录内：{r:?}");
    // 未授权外部：both false
    let r = check_external_path_core(
        &state,
        "s-1",
        &external.path().join("docs").to_string_lossy(),
    )
    .unwrap();
    assert!(!r.allowed && !r.inside_roots, "未授权外部：{r:?}");
    // 授权后：allowed=true, insideRoots=false
    let ext_dir = external.path().join("docs");
    fs::create_dir_all(&ext_dir).unwrap();
    allow_external_dir_core(&state, "s-1", ext_dir.to_str().unwrap()).unwrap();
    let r =
        check_external_path_core(&state, "s-1", &ext_dir.join("a.md").to_string_lossy()).unwrap();
    assert!(r.allowed && !r.inside_roots, "授权目录内：{r:?}");
}

/// #34 行为：授权并入 SessionMeta（跨重启持久）、roots 全集返回、重复
/// 授权幂等；会话不存在 E_NOT_FOUND；目录不存在 E_INVALID_ARG。
#[test]
fn allow_external_dir_persists_and_validates() {
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
        .upsert(meta("s-1", Some(&project.id)))
        .unwrap();
    let external = tempfile::tempdir().unwrap();
    let ext_dir = external.path().join("docs");
    fs::create_dir_all(&ext_dir).unwrap();

    let r = allow_external_dir_core(&state, "s-1", ext_dir.to_str().unwrap()).unwrap();
    assert_eq!(r.roots.len(), 3, "两核心根 + 一个授权目录");
    let ext_canon = ext_dir
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        r.roots.contains(&ext_canon),
        "roots 含授权目录（canonical 形态）"
    );
    // 持久化在 SessionMeta（重启 = 新 AppState 同数据目录读回）
    assert_eq!(
        state
            .store
            .index()
            .get("s-1")
            .unwrap()
            .authorized_external_dirs
            .len(),
        1,
        "授权记录持久于索引"
    );
    // 重复授权幂等
    let r2 = allow_external_dir_core(&state, "s-1", ext_dir.to_str().unwrap()).unwrap();
    assert_eq!(r2.roots.len(), 3, "重复授权不重复追加");
    assert_eq!(
        state
            .store
            .index()
            .get("s-1")
            .unwrap()
            .authorized_external_dirs
            .len(),
        1
    );
    // 会话不存在
    let err = allow_external_dir_core(&state, "幽灵", ext_dir.to_str().unwrap()).unwrap_err();
    assert_eq!(err.code, "E_NOT_FOUND");
    // 目录不存在（canonicalize 失败）
    let err = allow_external_dir_core(
        &state,
        "s-1",
        external.path().join("不存在").to_str().unwrap(),
    )
    .unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
}

/// 拒绝整体放行（§9.9）：文件系统根、家目录本身及其祖先、宿主托管目录
/// （数据目录）本身/祖先/后代——全部 E_INVALID_ARG。
#[test]
fn protected_grant_targets_rejected() {
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
        .upsert(meta("s-1", Some(&project.id)))
        .unwrap();
    // 数据目录本身与后代（workspace 子目录虽在核心 roots 内，授权语义同样拒绝）
    for target in [dir.path().to_path_buf(), dir.path().join("workspace")] {
        let err = allow_external_dir_core(&state, "s-1", target.to_str().unwrap()).unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARG", "托管目录 {target:?}");
    }
    // 家目录本身与祖先（macOS HOME=/Users/x；/Users 是其祖先）
    if let Ok(home_real) = std::fs::canonicalize(std::path::Path::new(
        &std::env::var("HOME").unwrap_or_default(),
    )) {
        let err = allow_external_dir_core(&state, "s-1", home_real.to_str().unwrap()).unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARG", "家目录本身");
        if let Some(parent) = home_real.parent() {
            let err = allow_external_dir_core(&state, "s-1", parent.to_str().unwrap()).unwrap_err();
            assert_eq!(err.code, "E_INVALID_ARG", "家目录祖先 {parent:?}");
        }
    }
    // 文件系统根
    let err = allow_external_dir_core(&state, "s-1", "/").unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG", "文件系统根");
    assert!(
        state
            .store
            .index()
            .get("s-1")
            .unwrap()
            .authorized_external_dirs
            .is_empty(),
        "禁区目标全部未落授权"
    );
}

/// 授权后 PathGuard 放行（read_workspace_file 预览读取与 @ 补全同 roots
/// 面）：授权目录内文件可读；撤销后重新拒绝；授权跨会话不延续。
#[test]
fn grant_then_read_then_revoke() {
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
        .upsert(meta("s-1", Some(&project.id)))
        .unwrap();
    state
        .store
        .index()
        .upsert(meta("s-other", Some(&project.id)))
        .unwrap();
    let external = tempfile::tempdir().unwrap();
    let ext_dir = external.path().join("docs");
    fs::create_dir_all(&ext_dir).unwrap();
    fs::write(ext_dir.join("note.md"), "外部资料").unwrap();
    let abs = ext_dir.join("note.md").to_string_lossy().into_owned();

    // 未授权：E_PATH_OUTSIDE_ROOTS
    let err = read_workspace_file_core(&state, "s-1", &abs, None, None).unwrap_err();
    assert_eq!(err.code, "E_PATH_OUTSIDE_ROOTS", "未授权越界");
    // 授权：放行
    allow_external_dir_core(&state, "s-1", ext_dir.to_str().unwrap()).unwrap();
    let result = read_workspace_file_core(&state, "s-1", &abs, None, None).unwrap();
    assert_eq!(result.content, "外部资料", "授权后放行");
    // 授权不跨会话延续（s-other 同项目会话仍拒绝）
    let err = read_workspace_file_core(&state, "s-other", &abs, None, None).unwrap_err();
    assert_eq!(err.code, "E_PATH_OUTSIDE_ROOTS", "授权仅本会话有效");
    // 撤销：s-1 也重新拒绝
    let r = revoke_external_dir_core(&state, "s-1", ext_dir.to_str().unwrap()).unwrap();
    assert_eq!(r.roots.len(), 2, "撤销后仅剩两核心根");
    let err = read_workspace_file_core(&state, "s-1", &abs, None, None).unwrap_err();
    assert_eq!(err.code, "E_PATH_OUTSIDE_ROOTS", "撤销后下次访问重新询问");
    // 撤销不存在的授权幂等不报错
    revoke_external_dir_core(&state, "s-1", ext_dir.to_str().unwrap()).unwrap();
}

/// Q41 恢复授权接线面：clear_all_external_grants 清空全部会话授权
///（33 号票恢复流程调用；普通重启不调用——授权跨重启保留）。
#[test]
fn clear_all_grants_for_backup_restore() {
    let dir = tempfile::tempdir().unwrap();
    let state = app(dir.path());
    let external = tempfile::tempdir().unwrap();
    let ext_dir = external.path().join("docs");
    fs::create_dir_all(&ext_dir).unwrap();
    for sid in ["s-a", "s-b"] {
        state.store.index().upsert(meta(sid, None)).unwrap();
        allow_external_dir_core(&state, sid, ext_dir.to_str().unwrap()).unwrap();
    }
    // 正常重启口径：同数据目录新 AppState 授权仍在
    let restarted = app(dir.path());
    assert_eq!(
        restarted
            .store
            .index()
            .get("s-a")
            .unwrap()
            .authorized_external_dirs
            .len(),
        1,
        "普通重启保留有效授权"
    );
    // 恢复流程口径：清空全部
    let cleared = clear_all_external_grants(&state).unwrap();
    assert_eq!(cleared, 2, "清理两个会话");
    assert!(
        state
            .store
            .index()
            .get("s-a")
            .unwrap()
            .authorized_external_dirs
            .is_empty()
            && state
                .store
                .index()
                .get("s-b")
                .unwrap()
                .authorized_external_dirs
                .is_empty(),
        "恢复后授权失效（须重新确认）"
    );
}
