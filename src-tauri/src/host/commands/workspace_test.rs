//! workspace 域命令测试（独立测试文件）：会话 roots 解析（临时/项目双形态，
//! C-8 收窄后第二根 = 数据目录下 workspace/ 子目录，21 号票）、路径搜索、
//! 预览读取（翻页/截断/拒绝面）、外部目录授权生命周期与恢复保护。

use std::fs;
use std::sync::Arc;

use super::workspace::{
    allow_external_dir_core, check_external_path_core, read_workspace_file_core,
    resolve_session_roots, revoke_external_dir_core, search_workspace_paths_core,
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

/// 项目会话夹具：项目主目录指向独立 tempdir（home），会话 id 固定 "s-1"。
/// 返回 (state, home, project_id)——目录句柄借 TempDir 析构清理。
fn project_session(data: &std::path::Path) -> (AppState, tempfile::TempDir, String) {
    let state = app(data);
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
    (state, home, project.id)
}

/// 断言 roots 的三种形态（项目会话 project_root=项目主目录、data_root=数据
/// 目录下 workspace/ 子目录——C-8 收窄 21 号票；临时会话两根同为 workspace
/// 子目录且数据目录本体不在 roots 内；未知会话按临时口径防御回退），以及
/// 搜索语义：基名大小写不敏感、跳过 .git/node_modules、项目主目录下相对
/// 路径、workspace 子目录命中绝对路径、数据目录本体文件不进命中（C-8 收窄
/// 验收）；目录也命中（kind=dir）、limit 生效、空 query 返回空。
#[test]
fn resolve_roots_and_search_semantics() {
    let data = tempfile::tempdir().unwrap();
    let (state, home, _ext) = project_session(data.path());
    fs::create_dir_all(data.path().join("workspace")).unwrap();
    let ws = fs::canonicalize(data.path().join("workspace")).unwrap();

    // 项目会话：项目主目录 + workspace 子目录。
    let roots = resolve_session_roots(&state, "s-1");
    assert_eq!(
        fs::canonicalize(&roots.project_root).unwrap(),
        fs::canonicalize(home.path()).unwrap()
    );
    assert_eq!(
        fs::canonicalize(&roots.workspace_root).unwrap(),
        ws,
        "第二根 = 数据目录下 workspace/（C-8 收窄）"
    );

    // 临时会话：两根同目录；数据目录本体（config.json 所在层）不在 roots 内。
    state.store.index().upsert(meta("s-temp", None)).unwrap();
    let roots = resolve_session_roots(&state, "s-temp");
    assert_eq!(
        roots.project_root, roots.workspace_root,
        "临时会话两根同目录"
    );
    assert_eq!(
        fs::canonicalize(roots.workspace_root).unwrap(),
        ws,
        "临时会话遍历基准即 workspace 子目录"
    );
    assert_ne!(
        fs::canonicalize(&roots.project_root).unwrap(),
        fs::canonicalize(data.path()).unwrap(),
        "数据目录根不再作为 agent 可见根（config.json 等退出可见面）"
    );

    // 未知会话：按临时口径（防御：index 无条目不 panic）。
    let roots = resolve_session_roots(&state, "幽灵会话");
    assert_eq!(roots.project_root, roots.workspace_root);
    assert_eq!(fs::canonicalize(roots.workspace_root).unwrap(), ws);

    // 搜索语义。
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

    // 目录命中、limit、空 query（临时会话遍历 workspace 子目录）。
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

/// 预览读取的翻页与截断：全文、range 按行翻页（未读到尾即 truncated）、
/// 512KB 字节上限按字符边界截断（文件落 workspace 子目录，临时会话相对
/// 路径基准即 workspace）；以及拒绝面与 workspace 第二根放行（C-8 收窄
/// 验收，21 号票）：前 8KB 含 NUL 判二进制拒绝、缺失文件 E_NOT_FOUND、
/// `..` 越界拒绝、存储内部文件（config.json）双口径都不可达；workspace
/// 子目录内文件可经绝对路径读取（第二根放行），数据目录本体绝对路径仍拒绝。
#[test]
fn read_preview_paging_truncation_and_rejections() {
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

    // 512KB 截断（600KB UTF-8）。
    let big = "汉".repeat(200_000);
    fs::write(data.path().join("workspace/big.txt"), &big).unwrap();
    let result = read_workspace_file_core(&state, "任意会话", "big.txt", None, None).unwrap();
    assert!(result.content.len() <= 512 * 1024);
    assert!(result.truncated);
    assert!(result.content.ends_with('汉'), "按字符边界截断");

    // 拒绝面与 workspace 第二根放行。
    let data = tempfile::tempdir().unwrap();
    fs::create_dir_all(data.path().join("workspace")).unwrap();
    fs::write(data.path().join("workspace/bin.dat"), [0u8, 1, 2, 3]).unwrap();
    fs::write(data.path().join("config.json"), b"{\"api_key\":\"secret\"}").unwrap();
    let state = app(data.path());

    let err = read_workspace_file_core(&state, "任意会话", "bin.dat", None, None).unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG", "二进制拒绝");
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

    // 项目会话下 workspace 子目录内文件可经绝对路径读取（读侧锚点）。
    let (state, _home, project_id) = project_session(data.path());
    state
        .store
        .index()
        .upsert(meta("s-ws", Some(&project_id)))
        .unwrap();
    fs::write(data.path().join("workspace/共享.md"), "工作区内容").unwrap();
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

/// 外部目录授权（§3.3 #33/#34，24 号票）全链：授权生命周期三段（#33 三态 +
/// #34 行为 + 读侧 PathGuard 联动）——初始两根内/未授权外部/授权目录内的
/// 判定；授权并入 SessionMeta 跨重启持久、roots 全集返回、重复授权幂等、
/// 会话不存在 E_NOT_FOUND、目录不存在 E_INVALID_ARG；授权后预览放行、撤销
/// 后重新拒绝、授权不跨会话延续（撤销幂等）；以及两道安全收口——禁区目标
/// （文件系统根、家目录本身及祖先、宿主托管目录——数据目录本身/祖先/后代）
/// 全部 E_INVALID_ARG 且不落授权（§9.9）；恢复保护序列（33 号票接线，Q41）：
/// 恢复标记存在时 apply_restore_protection 清空全部会话授权（普通重启不
/// 调用——授权跨重启保留）。
#[test]
fn external_grant_lifecycle_and_protection() {
    let dir = tempfile::tempdir().unwrap();
    let (state, _home, _ext) = project_session(dir.path());
    fs::create_dir_all(dir.path().join("workspace")).unwrap();
    let external = tempfile::tempdir().unwrap();
    let ext_dir = external.path().join("docs");
    fs::create_dir_all(&ext_dir).unwrap();
    fs::write(ext_dir.join("note.md"), "外部资料").unwrap();
    let abs = ext_dir.join("note.md").to_string_lossy().into_owned();

    // #33 三态：初始两根内 / 未授权外部。
    let r = check_external_path_core(&state, "s-1", "src/lib.rs").unwrap();
    assert!(r.allowed && r.inside_roots, "项目主目录内：{r:?}");
    let r = check_external_path_core(&state, "s-1", &ext_dir.to_string_lossy()).unwrap();
    assert!(!r.allowed && !r.inside_roots, "未授权外部：{r:?}");

    // #34 行为：授权持久于索引、roots 全集、幂等、参数校验。
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
    let r2 = allow_external_dir_core(&state, "s-1", ext_dir.to_str().unwrap()).unwrap();
    assert_eq!(r2.roots.len(), 3, "重复授权不重复追加");
    let err = allow_external_dir_core(&state, "幽灵", ext_dir.to_str().unwrap()).unwrap_err();
    assert_eq!(err.code, "E_NOT_FOUND");
    let err = allow_external_dir_core(
        &state,
        "s-1",
        external.path().join("不存在").to_str().unwrap(),
    )
    .unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");

    // #33 三态收尾：授权后 allowed=true, insideRoots=false。
    let r =
        check_external_path_core(&state, "s-1", &ext_dir.join("a.md").to_string_lossy()).unwrap();
    assert!(r.allowed && !r.inside_roots, "授权目录内：{r:?}");

    // 授权后 PathGuard 放行（read_workspace_file 预览读取与 @ 补全同 roots 面）。
    let result = read_workspace_file_core(&state, "s-1", &abs, None, None).unwrap();
    assert_eq!(result.content, "外部资料", "授权后放行");
    // 授权不跨会话延续（s-other 同项目会话仍拒绝）。
    let project_id = state
        .store
        .index()
        .get("s-1")
        .unwrap()
        .project_id
        .clone()
        .unwrap();
    state
        .store
        .index()
        .upsert(meta("s-other", Some(&project_id)))
        .unwrap();
    let err = read_workspace_file_core(&state, "s-other", &abs, None, None).unwrap_err();
    assert_eq!(err.code, "E_PATH_OUTSIDE_ROOTS", "授权仅本会话有效");
    // 撤销：s-1 也重新拒绝；撤销不存在的授权幂等不报错。
    let r = revoke_external_dir_core(&state, "s-1", ext_dir.to_str().unwrap()).unwrap();
    assert_eq!(r.roots.len(), 2, "撤销后仅剩两核心根");
    let err = read_workspace_file_core(&state, "s-1", &abs, None, None).unwrap_err();
    assert_eq!(err.code, "E_PATH_OUTSIDE_ROOTS", "撤销后下次访问重新询问");
    revoke_external_dir_core(&state, "s-1", ext_dir.to_str().unwrap()).unwrap();

    // 禁区目标收口：数据目录本身与后代（workspace 子目录虽在核心 roots 内，
    // 授权语义同样拒绝）。
    let dir = tempfile::tempdir().unwrap();
    let (state, _home, _ext) = project_session(dir.path());
    for target in [dir.path().to_path_buf(), dir.path().join("workspace")] {
        let err = allow_external_dir_core(&state, "s-1", target.to_str().unwrap()).unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARG", "托管目录 {target:?}");
    }
    // 家目录本身与祖先（macOS HOME=/Users/x；/Users 是其祖先）
    if let Ok(home_real) = fs::canonicalize(std::path::Path::new(
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

    // 恢复保护序列：先授权、验证普通重启保留，再放置恢复标记走真实保护路径。
    let external = tempfile::tempdir().unwrap();
    let ext_dir = external.path().join("docs");
    fs::create_dir_all(&ext_dir).unwrap();
    for sid in ["s-1", "s-b"] {
        if sid != "s-1" {
            state.store.index().upsert(meta(sid, None)).unwrap();
        }
        allow_external_dir_core(&state, sid, ext_dir.to_str().unwrap()).unwrap();
    }
    let restarted = app(dir.path());
    assert_eq!(
        restarted
            .store
            .index()
            .get("s-1")
            .unwrap()
            .authorized_external_dirs
            .len(),
        1,
        "普通重启保留有效授权"
    );
    assert!(
        !crate::storage::backup::restore_pending_path(dir.path()).exists(),
        "无恢复标记 = 普通重启"
    );
    fs::write(
        crate::storage::backup::restore_pending_path(dir.path()),
        b"{\"version\":1}",
    )
    .unwrap();
    let report = crate::storage::backup::apply_restore_protection(dir.path(), Some(&state.config));
    assert!(report.applied, "恢复标记存在时应执行保护");
    assert!(
        report.failures.is_empty(),
        "保护序列不应失败：{:?}",
        report.failures
    );
    // 信任清空 + 外部授权清空 + 审批白名单清空三动作齐备（mcp-tools 与
    // 队列文件在本测试场景不存在，不产生动作条目）
    assert_eq!(report.actions.len(), 3, "动作清单：{:?}", report.actions);
    assert!(
        state
            .store
            .index()
            .get("s-1")
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
    assert!(
        !crate::storage::backup::restore_pending_path(dir.path()).exists(),
        "保护完成后恢复标记应被删除（防重复触发）"
    );
}
