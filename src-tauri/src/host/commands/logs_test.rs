//! [`super::logs`] 命令主体的单元测试（32 号票）：tail 语义（缺省 500/
//! 上限 5000/文件不存在空清单）与 open_logs_dir 的目录创建面。

use super::*;
use crate::host::AppState;
use crate::util::events::RecordingEventSink;

fn app_in(dir: &std::path::Path) -> AppState {
    AppState::new_in(
        dir.to_path_buf(),
        std::sync::Arc::new(RecordingEventSink::new()),
    )
}

/// 断言 tail 语义（缺省 500/上限 5000/文件不存在空清单）与 open_logs_dir
/// 的目录创建面。
#[test]
fn tail_semantics_default_clamp_missing_and_open_dir() {
    let dir = tempfile::tempdir().expect("临时目录");
    let app = app_in(dir.path());
    // 文件不存在 → 空清单不报错（幂等查询）
    assert!(read_session_log_core(&app, "ghost", None).lines.is_empty());
    // 写 12 行，缺省 500 → 全部；tail=3 → 尾 3 行
    for i in 0..12 {
        app.session_log.session_line(
            "s1",
            crate::providers::types::LogLevel::Info,
            crate::host::session_log::LogKind::Run,
            format!("行 {i}"),
            None,
            None,
            None,
            None,
        );
    }
    assert_eq!(read_session_log_core(&app, "s1", None).lines.len(), 12);
    let tail3 = read_session_log_core(&app, "s1", Some(3)).lines;
    assert_eq!(tail3.len(), 3);
    assert!(tail3[0].contains("行 9"));
    // tail=0 视为缺省；超上限 clamp 到 5000
    assert_eq!(read_session_log_core(&app, "s1", Some(0)).lines.len(), 12);
    let maxed = read_session_log_core(&app, "s1", Some(u32::MAX));
    assert_eq!(maxed.lines.len(), 12); // 行数少于上限时全出

    // open_logs_dir：目录创建面。
    let dir = tempfile::tempdir().expect("临时目录");
    let app = app_in(dir.path());
    let logs = dir.path().join("logs");
    assert!(!logs.exists());
    // 系统打开在 headless 环境可能失败（无文件管理器），但目录创建先行
    // ——目录断言不依赖打开成功（打开失败属环境面）。
    let _ = crate::host::commands::logs::open_logs_dir_inner(&app);
    assert!(logs.is_dir(), "open_logs_dir 应先确保日志目录存在");
}
