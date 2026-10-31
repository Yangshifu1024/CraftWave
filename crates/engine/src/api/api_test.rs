//! API 清单对账测试：`api_manifest()` 与旧壳 `ipc_commands!` 清单的一致性。
//!
//! 旧壳清单的命令名是契约——这里静态钉死「总数 / 无重复 / 三状态各自的
//! 名单」，防止映射表文档、manifest 函数与实际 API 面三者漂移（接口先行
//! 原则 6：接口变更走显式提交）。阶段 5 功能覆盖审计时，另与旧壳落盘的
//! `target/ipc-manifest.json` 做双向对账。

use super::{ApiSpec, ApiStatus, api_manifest};

/// 旧壳 `ipc_commands!` 清单总条数（含 dev-only 的 mock_chat_run）。
const TOTAL_COMMANDS: usize = 78;

/// 排除的纯壳命令名单（模块文档「排除清单」的机器可读镜像）。
const SHELL_ONLY: &[&str] = &[
    "activate_custom_titlebar",
    "restore_native_titlebar",
    "select_dir",
    "resolve_exit_request",
    "notification_permission_status",
    "open_notification_settings",
    "open_dir",
    "open_logs_dir",
    "update_status",
    "mock_chat_run",
];

/// 实现待平移的 Planned 名单（随阶段 2.C 2.11 帧流切片消解）。
const PLANNED: &[&str] = &["start_chat", "resume_followup_queue", "compact_session"];

#[test]
fn manifest_总数与旧壳清单一致() {
    let manifest = api_manifest();
    assert_eq!(
        manifest.len(),
        TOTAL_COMMANDS,
        "api_manifest 条数应与旧壳 ipc_commands! 一致（{TOTAL_COMMANDS}）"
    );
}

#[test]
fn manifest_无重复命令名() {
    let mut names: Vec<&str> = api_manifest().iter().map(|s| s.name).collect();
    names.sort_unstable();
    let deduped = names.clone();
    names.dedup();
    assert_eq!(names, deduped, "manifest 存在重复命令名：清单必须与旧壳一对一");
}

#[test]
fn manifest_排除名单精确匹配() {
    let shell: Vec<&str> = api_manifest()
        .iter()
        .filter(|s| s.status == ApiStatus::ShellOnly)
        .map(|s| s.name)
        .collect();
    let mut expected = SHELL_ONLY.to_vec();
    expected.sort_unstable();
    let mut actual = shell;
    actual.sort_unstable();
    assert_eq!(actual, expected, "ShellOnly 名单与模块文档排除清单不一致");
}

#[test]
fn manifest_待平移名单精确匹配() {
    let planned: Vec<&str> = api_manifest()
        .iter()
        .filter(|s| s.status == ApiStatus::Planned)
        .map(|s| s.name)
        .collect();
    let mut expected = PLANNED.to_vec();
    expected.sort_unstable();
    let mut actual = planned;
    actual.sort_unstable();
    assert_eq!(actual, expected, "Planned 名单应恰为三个 run 装配树命令");
}

#[test]
fn manifest_状态覆盖完备() {
    // 每条必属三态之一（ApiStatus 是封闭枚举，本测试主要钉住
    // Ready+Planned+ShellOnly == 总数，防漏标）
    let manifest: Vec<ApiSpec> = api_manifest();
    let counted = manifest
        .iter()
        .filter(|s| {
            matches!(
                s.status,
                ApiStatus::Ready | ApiStatus::Planned | ApiStatus::ShellOnly
            )
        })
        .count();
    assert_eq!(counted, manifest.len());
    let ready = manifest.iter().filter(|s| s.status == ApiStatus::Ready).count();
    assert_eq!(
        ready,
        manifest.len() - SHELL_ONLY.len() - PLANNED.len(),
        "Ready 条数 = 总数 - 排除 - 待平移"
    );
}
