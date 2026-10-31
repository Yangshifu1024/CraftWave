//! open_dir 白名单校验的表驱动测试（03 号票，FR-14-07，§3.3 #3）。
//!
//! 只测校验面 [`resolve_open_dir`]（headless、无系统副作用）：合法目录放行、
//! 白名单外拒绝（E_PATH_OUTSIDE_ROOTS）、非目录/不存在拒绝（E_NOT_FOUND）、
//! 空参拒绝（E_INVALID_ARG）、残缺注册与 "~" 展开安全语义。OS 打开动作
//! （open crate 一行调用）留在命令层 [`open_dir_core`]，不进测试面。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::system::{expand_home_prefix, resolve_open_dir};
use crate::host::AppState;
use crate::util::errors::{E_INVALID_ARG, E_NOT_FOUND, E_PATH_OUTSIDE_ROOTS};
use crate::util::events::RecordingEventSink;

/// 测试脚手架：在临时数据目录上建一份 AppState（事件汇用录制桩，无真实副作用）。
fn app(data: &std::path::Path) -> AppState {
    AppState::new_in(data.to_path_buf(), Arc::new(RecordingEventSink::new()))
}

/// 注册一个项目并返回其项目数据目录（save_project 注册即建 `.craftwave/`）。
fn register_project(state: &AppState, home: &Path) -> PathBuf {
    state
        .projects
        .save(None, "项目", home.to_string_lossy().as_ref())
        .expect("注册项目失败");
    home.join(".craftwave")
}

/// 断言 path 被放行且解析结果与 expect 一致（canonical 比较，消符号链接差异）。
fn assert_allowed(state: &AppState, path: &Path, expect: &Path) {
    let resolved = resolve_open_dir(state, &path.to_string_lossy()).expect("应放行");
    assert_eq!(
        fs::canonicalize(&resolved).unwrap(),
        fs::canonicalize(expect).unwrap()
    );
}

/// 断言 path 被拒绝且错误码匹配。
fn assert_rejected(state: &AppState, path: &Path, code: &str) {
    let err = resolve_open_dir(state, &path.to_string_lossy()).expect_err("应拒绝");
    assert_eq!(err.code, code, "拒绝路径 {path:?} 的错误码不符");
}

/// open_dir 白名单校验全集：三个合法白名单根 + 数据目录子路径全部放行；
/// 白名单外路径一律 E_PATH_OUTSIDE_ROOTS——系统其他目录、项目主目录本体
/// （白名单只收项目数据目录，不收工作区）、数据目录的上级、`..` 逃逸形态；
/// 边界形态与残缺注册：空/空白 path 报 E_INVALID_ARG；数据目录下的文件
/// （config.json）与不存在的子路径都是"授权范围内但目标不存在"，报
/// E_NOT_FOUND（与越界的 E_PATH_OUTSIDE_ROOTS 分层）；项目主目录被外部
/// 删除（残缺注册）时项目数据目录自动退出白名单，报越界不 panic；"~" 前缀
/// 展开（纯函数表驱动）：`~` 与 `~/…` 展开为 HOME 前缀；无前缀与 "~user"
/// 形态原样返回（后者交白名单拒绝）。展开后仍受白名单约束：测试进程的
/// HOME 展开路径（真实用户主目录）不在临时数据目录白名单内，必须拒绝——
/// 钉住"展开不绕过校验"的安全语义。
#[test]
fn whitelist_boundary_and_tilde_expansion_semantics() {
    let data = tempfile::tempdir().unwrap();
    let state = app(data.path());
    let home = tempfile::tempdir().unwrap();
    let project_data = register_project(&state, home.path());

    let allowed: Vec<(PathBuf, PathBuf)> = vec![
        // 数据目录本体
        (data.path().to_path_buf(), data.path().to_path_buf()),
        // 日志目录（open_dir_roots 幂等补建后放行）
        (data.path().join("logs"), data.path().join("logs")),
        // 项目数据目录本体
        (project_data.clone(), project_data.clone()),
        // 白名单根之下的子路径（projects/ 目录存有注册表）
        (data.path().join("projects"), data.path().join("projects")),
    ];
    for (path, expect) in allowed {
        assert_allowed(&state, &path, &expect);
    }

    let unrelated = tempfile::tempdir().unwrap();
    let outside: Vec<PathBuf> = vec![
        // 系统任意目录（不在白名单）
        unrelated.path().to_path_buf(),
        // 项目主目录本体（工作区不在白名单，只有其 .craftwave）
        home.path().to_path_buf(),
        // 数据目录的上级（临时根的父目录）
        data.path().parent().unwrap().to_path_buf(),
        // `..` 逃逸形态（词法上落在数据目录内、解析后越界）
        data.path().join("..").join("elsewhere"),
    ];
    for path in outside {
        assert_rejected(&state, &path, E_PATH_OUTSIDE_ROOTS);
    }

    // 边界形态与残缺注册。
    let data = tempfile::tempdir().unwrap();
    let state = app(data.path());
    // 造一个数据目录下的文件路径（非目录）
    fs::write(data.path().join("config.json"), "{}").unwrap();

    for empty in ["", "   "] {
        let err = resolve_open_dir(&state, empty).expect_err("空 path 应拒绝");
        assert_eq!(err.code, E_INVALID_ARG);
    }
    let err = resolve_open_dir(&state, &data.path().join("config.json").to_string_lossy())
        .expect_err("文件路径应拒绝");
    assert_eq!(err.code, E_NOT_FOUND);
    let err = resolve_open_dir(
        &state,
        &data.path().join("no-such-subdir").to_string_lossy(),
    )
    .expect_err("不存在的子路径应拒绝");
    assert_eq!(err.code, E_NOT_FOUND);

    // 模拟外部删除：整个项目主目录消失。
    let home = tempfile::tempdir().unwrap();
    let project_data = register_project(&state, home.path());
    fs::remove_dir_all(home.path()).unwrap();
    assert_rejected(&state, &project_data, E_PATH_OUTSIDE_ROOTS);

    // "~" 前缀展开（纯函数表驱动）。
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"));
    let cases: Vec<(&str, String)> = vec![
        ("~/x", format!("{}/x", home.as_deref().unwrap_or(""))),
        ("~", home.as_deref().unwrap_or("").to_owned()),
        ("/abs/path", "/abs/path".to_owned()),
        ("rel/path", "rel/path".to_owned()),
        ("~user/x", "~user/x".to_owned()),
    ];
    for (input, expect) in cases {
        assert_eq!(expand_home_prefix(input), expect, "输入 {input:?}");
    }

    let data = tempfile::tempdir().unwrap();
    let state = app(data.path());
    assert_rejected(&state, Path::new("~/.craftwave/logs"), E_PATH_OUTSIDE_ROOTS);
}
