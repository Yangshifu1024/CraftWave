//! 产物面板与编辑器打开命令测试（独立测试文件，28 号票）：
//! list_artifacts（登记经 SessionStore 直驱 + 空清单兜底）、open_in_editor
//! 校验面（空路径 / 越界 / 白名单放行到编辑器探测）、编辑器探测表、
//! @ 补全搜索性能与忽略表核查。

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
/// 以及 open_in_editor 校验面表驱动：空路径拒绝 E_INVALID_ARG；白名单越界
/// 拒绝 E_PATH_OUTSIDE_ROOTS（§9.1——家目录下任意非白名单文件不可打开）；
/// 数据目录内（workspace 产物落点）通过白名单，编辑器缺失环境 → E_NOT_FOUND
/// 而非 panic。编辑器探测表：条目非空、候选表优先级序（CLI 在前）。
#[test]
fn list_artifacts_and_open_in_editor_semantics() {
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

    // open_in_editor 校验面表驱动。
    let dir = tempfile::tempdir().unwrap();
    let state = app(dir.path());

    // 空路径。
    let err = open_in_editor_core(&state, "  ", None).unwrap_err();
    assert_eq!(err.code, E_INVALID_ARG, "空路径应拒绝");

    // 白名单越界。
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("secret.txt");
    fs::write(&target, "x").unwrap();
    let err = open_in_editor_core(&state, target.to_string_lossy().as_ref(), None).unwrap_err();
    assert_eq!(err.code, E_PATH_OUTSIDE_ROOTS);

    // 白名单内：命令成功（行号参数生效形态不做 OS 层断言）或编辑器缺失
    // 环境 E_NOT_FOUND（探测不到可用编辑器）。
    let target = dir.path().join("workspace").join("out.txt");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(&target, "x").unwrap();
    match open_in_editor_core(&state, target.to_string_lossy().as_ref(), Some(12)) {
        Ok(()) => {}
        Err(err) => assert_eq!(err.code, E_NOT_FOUND, "无编辑器环境应 E_NOT_FOUND"),
    }

    // 编辑器探测：返回条目按候选表优先级序（CLI 在前），path 非空。
    let editors = probe_editors();
    for (candidate, path) in &editors {
        assert!(!candidate.display.is_empty());
        assert!(!path.as_os_str().is_empty());
    }
    let pos = |bin: &str| editors.iter().position(|(c, _)| c.bin == bin);
    if let (Some(code_pos), Some(zed_pos)) = (pos("code"), pos("zed")) {
        assert!(code_pos < zed_pos, "code 与 zed 同时在时 code 应在前");
    }
}

/// @ 补全搜索两验收（28 号票）：万级文件目录性能（~15k 条目命中前 20 条，
/// 断言 CI debug 容忍上限 600ms，实测数打印在案）；忽略表核查（.git/
/// node_modules/target/dist/build/.craftwave 下条目不进搜索结果）。
#[test]
fn search_paths_large_tree_budget_and_ignore_dirs() {
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

    // 忽略表核查（SEARCH_SKIP_DIRS 覆盖票面清单）。
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
    assert_eq!(hits.len(), 1, "忽略目录下的条目不应命中");
    assert!(hits[0].path.ends_with("needle_unique_root.rs"));
}
