//! 产物面板与编辑器打开命令测试（独立测试文件，28 号票）：
//! list_artifacts（登记经 SessionStore 直驱 + 空清单兜底）、open_in_editor
//! （空路径拒绝 / 越界拒绝 E_PATH_OUTSIDE_ROOTS / 编辑器探测形态）。

use std::fs;
use std::sync::Arc;

use super::artifacts::{open_in_editor_core, probe_editors};
use super::workspace::search_workspace_paths_core;
use crate::host::AppState;
use crate::util::errors::{E_INVALID_ARG, E_NOT_FOUND, E_PATH_OUTSIDE_ROOTS};
use crate::util::events::RecordingEventSink;

/// 测试脚手架：临时数据目录上的 AppState（事件汇用录制桩）。
fn app(data: &std::path::Path) -> AppState {
    AppState::new_in(data.to_path_buf(), Arc::new(RecordingEventSink::new()))
}

/// list_artifacts 直驱：经 store 登记 → 查询带计数；无登记文件 → 空清单。
#[test]
fn list_artifacts_returns_rows_after_record() {
    let dir = tempfile::tempdir().unwrap();
    let state = app(dir.path());
    let file = dir.path().join("out.txt");
    fs::write(&file, "x").unwrap();
    state
        .store
        .record_artifact("s1", "run-a", file.to_string_lossy().as_ref())
        .unwrap();
    let result = crate::storage::ListArtifactsResult {
        artifacts: state.store.list_artifacts("s1"),
    };
    assert_eq!(result.artifacts.len(), 1);
    assert!(result.artifacts[0].exists);
    assert_eq!(result.artifacts[0].run_count, 1);
    // 无登记的会话 → 空清单（幂等查询）
    assert!(state.store.list_artifacts("s-none").is_empty());
}

/// open_in_editor：空路径拒绝 E_INVALID_ARG。
#[test]
fn open_in_editor_rejects_empty_path() {
    let dir = tempfile::tempdir().unwrap();
    let state = app(dir.path());
    let err = open_in_editor_core(&state, "  ", None).unwrap_err();
    assert_eq!(err.code, E_INVALID_ARG);
}

/// open_in_editor：白名单越界拒绝 E_PATH_OUTSIDE_ROOTS（§9.1——家目录下
/// 任意非白名单文件不可打开）。
#[test]
fn open_in_editor_rejects_path_outside_roots() {
    let dir = tempfile::tempdir().unwrap();
    let state = app(dir.path());
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("secret.txt");
    fs::write(&target, "x").unwrap();
    let err = open_in_editor_core(&state, target.to_string_lossy().as_ref(), None).unwrap_err();
    assert_eq!(err.code, E_PATH_OUTSIDE_ROOTS);
}

/// open_in_editor：数据目录内（workspace 产物落点）通过白名单；编辑器缺失
/// 环境（CI 无任何候选）→ E_NOT_FOUND 而非 panic。
#[test]
fn open_in_editor_whitelisted_path_reaches_editor_probe() {
    let dir = tempfile::tempdir().unwrap();
    let state = app(dir.path());
    let target = dir.path().join("workspace").join("out.txt");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "x").unwrap();
    let result = open_in_editor_core(&state, target.to_string_lossy().as_ref(), Some(12));
    match result {
        Ok(()) => {
            // 有可用编辑器的环境：命令成功（行号参数在 DashG/ColonSuffix
            // 形态下生效，None 形态忽略——不做 OS 层断言）
        }
        Err(err) => {
            // 无编辑器环境：明确 E_NOT_FOUND（探测不到可用编辑器）
            assert_eq!(err.code, E_NOT_FOUND);
        }
    }
}

/// 编辑器探测：返回条目按候选表优先级序（CLI 在前），path 非空。
/// 探测不到任何编辑器的环境返回空表（不报错）。
#[test]
fn probe_editors_returns_priority_ordered_entries() {
    let editors = probe_editors();
    for (candidate, path) in &editors {
        assert!(!candidate.display.is_empty());
        assert!(!path.as_os_str().is_empty());
    }
    // 优先级：若同时探测到 code 与 zed，code 在前（CLI_EDITORS 表序）
    let pos = |bin: &str| editors.iter().position(|(c, _)| c.bin == bin);
    if let (Some(code_pos), Some(zed_pos)) = (pos("code"), pos("zed")) {
        assert!(code_pos < zed_pos);
    }
}

/// 万级文件目录性能实测（28 号票验收：@ 补全命中前 20 条 P95 < 300ms）。
/// fixture：30 目录 × 350 文件 = 10500 条目 + 深一层 50×100 = 5000，共
/// ~15k 条目；查询命中前 20 条即止（limit=20 缺省）。实测数打印在案，
/// 断言 300ms 上限（debug 构建放宽到 600ms 上限保护 CI 抖动——票面记录
/// 以实测输出为准）。
#[test]
fn search_workspace_paths_on_large_tree_meets_budget() {
    let dir = tempfile::tempdir().unwrap();
    let state = app(dir.path());
    // 临时会话兜底 roots = <data>/workspace/（canonicalize 失败的根被跳过）
    let ws = dir.path().join("workspace");
    fs::create_dir_all(&ws).unwrap();
    // 造万级条目：10500 + 5000
    for d in 0..30 {
        let sub = ws.join(format!("mod{d}"));
        fs::create_dir_all(&sub).unwrap();
        for f in 0..350 {
            fs::write(sub.join(format!("file_{d}_{f}.rs")), "x").unwrap();
        }
    }
    for d in 0..50 {
        let sub = ws.join("deep").join(format!("pkg{d}"));
        fs::create_dir_all(&sub).unwrap();
        for f in 0..100 {
            fs::write(sub.join(format!("unit_{d}_{f}.ts")), "x").unwrap();
        }
    }
    // 预热一轮（首次含 canonicalize 与页缓存）
    let _ = search_workspace_paths_core(&state, "unknown-session", "file_1", Some(20)).unwrap();
    // 实测 5 轮取最大值（单机无并发扰动，最大值 ≈ P95 保守面）
    let mut worst_ms: u128 = 0;
    for round in 0..5 {
        let query = if round % 2 == 0 { "file_2" } else { "unit_3" };
        let started = std::time::Instant::now();
        let hits = search_workspace_paths_core(&state, "unknown-session", query, Some(20)).unwrap();
        let elapsed = started.elapsed();
        worst_ms = worst_ms.max(elapsed.as_millis());
        assert!(!hits.is_empty());
        assert!(hits.len() <= 20);
    }
    println!(
        "[28 号票性能实测] 万级目录（~15k 条目）@ 补全命中前 20 条最大耗时 {worst_ms}ms（验收线 300ms）"
    );
    // CI debug 构建抖动保护：600ms 硬上限；本机实测以 println 输出记录
    assert!(
        worst_ms < 600,
        "@ 补全搜索超预算：{worst_ms}ms（验收线 300ms，CI 容忍 600ms）"
    );
}

/// 忽略表核查（28 号票）：.git/node_modules/target/dist/build/.craftwave
/// 下的条目不进搜索结果（SEARCH_SKIP_DIRS 覆盖票面清单）。
#[test]
fn search_skips_default_ignore_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let state = app(dir.path());
    let ws = dir.path().join("workspace");
    fs::create_dir_all(&ws).unwrap();
    for name in [
        ".git",
        "node_modules",
        "target",
        "dist",
        "build",
        ".craftwave",
    ] {
        let sub = ws.join(name).join("nested");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("needle_unique_file.rs"), "x").unwrap();
    }
    fs::write(ws.join("needle_unique_root.rs"), "x").unwrap();
    let hits =
        search_workspace_paths_core(&state, "unknown-session", "needle_unique", Some(20)).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].path.ends_with("needle_unique_root.rs"));
}

/// 真实拉起验收（28 号票验收第 2 条；#[ignore] 不进常规 CI——会在用户
/// 桌面拉起 VS Code 窗口）：装有 `code` CLI 的机器上 `cargo test --ignored
/// open_in_editor_launches` 实测精确打开到文件行。断言面 = 命令 Ok（spawn
/// 成功）；窗口显示由用户肉眼确认。
#[test]
#[ignore = "真实拉起 VS Code（桌面弹窗），验收时手动 --ignored 运行"]
fn open_in_editor_launches_vscode_with_line() {
    let dir = tempfile::tempdir().unwrap();
    let state = app(dir.path());
    // 白名单 = 数据目录 + 注册项目主目录：注册一个临时项目，文件落其内
    let home = tempfile::tempdir().unwrap();
    let project = state
        .projects
        .save(None, "验收项目", home.path().to_string_lossy().as_ref())
        .unwrap();
    let target = home.path().join("验收.txt");
    fs::write(&target, "第一行\n第二行\n第三行\n").unwrap();
    let _ = project;
    open_in_editor_core(&state, target.to_string_lossy().as_ref(), Some(2))
        .expect("装有 code CLI 时应成功拉起 VS Code 并定位到第 2 行");
}
