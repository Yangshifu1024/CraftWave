//! [`delete_session_core`] 的盘点联动测试：会话专属日志
//! 与轮转副本随删、删除失败必须报告、子代理历史按 meta 归属盘点清理、
//! 队列文件随删、用户工作成果不误伤。

use super::*;
use craftwave_engine::agent::RunManager;
use craftwave_engine::app_state::AppState;
use craftwave_engine::providers::types::LogLevel;
use craftwave_engine::util::events::RecordingEventSink;

fn app_in(dir: &std::path::Path) -> AppState {
    AppState::new_in(
        dir.to_path_buf(),
        std::sync::Arc::new(RecordingEventSink::new()),
    )
}

/// 建一个会话（index 条目——delete_session 要求存在）。
fn seed_session(app: &AppState, session_id: &str) {
    let ts = 0i64;
    app.store
        .index()
        .upsert(craftwave_engine::storage::SessionMeta {
            id: session_id.to_owned(),
            project_id: None,
            title: "测试会话".to_owned(),
            created_at: ts,
            updated_at: ts,
            revision: 0,
            message_count: 0,
            total_tokens: 0,
            authorized_external_dirs: Vec::new(),
        })
        .expect("建档");
}

/// 会话行写入简写（附加字段全空）。
fn line(app: &AppState, session: &str, msg: &str) {
    app.session_log.session_line(
        session,
        LogLevel::Info,
        craftwave_engine::session_log::LogKind::Run,
        msg.to_owned(),
        None,
        None,
        None,
        None,
    );
}

/// 盘点联动全链：会话日志与轮转副本随删、其他会话日志不误删；删除
/// 失败必须报告（E_IO 带残留清单），不得声称全部删除成功；归属边界——
/// 本会话的子代理历史随删（meta 匹配）、他会话的不误删（meta 不匹配）、
/// meta 缺失无法归属的保守保留、followUp 队列文件随删、用户
/// 工作成果（workspace 内产出）不随会话销毁（共享资源只清本会话
/// 引用，不删项目成果）。
#[test]
fn delete_session_purge_ownership_and_failure_reporting() {
    let dir = tempfile::tempdir().expect("临时目录");
    let app = app_in(dir.path());
    seed_session(&app, "s-del");
    // 写会话日志（产生主文件）+ 预置轮转副本；其他会话的日志不受影响
    line(&app, "s-del", "一行");
    let logs = dir.path().join("logs").join("sessions");
    std::fs::create_dir_all(&logs).expect("建目录");
    std::fs::write(logs.join("s-del.jsonl.1"), "旧副本").expect("预置副本");
    line(&app, "s-keep", "保留");

    delete_session_core(&app, &RunManager::new(), "s-del").expect("删除成功");

    assert!(!logs.join("s-del.jsonl").exists(), "会话日志应随删");
    assert!(!logs.join("s-del.jsonl.1").exists(), "轮转副本应随删");
    assert!(logs.join("s-keep.jsonl").exists(), "其他会话日志不误删");

    // 失败必须报告：写日志（产生文件）后把主文件换成目录，remove_file 必失败。
    let dir = tempfile::tempdir().expect("临时目录");
    let app = app_in(dir.path());
    seed_session(&app, "s-stuck");
    line(&app, "s-stuck", "一行");
    let log_file = dir
        .path()
        .join("logs")
        .join("sessions")
        .join("s-stuck.jsonl");
    std::fs::remove_file(&log_file).expect("先移除文件");
    std::fs::create_dir_all(&log_file).expect("目录占位使删除失败");

    let err = delete_session_core(&app, &RunManager::new(), "s-stuck")
        .expect_err("删除失败必须报告，不得声称全部删除成功");
    assert_eq!(err.code, "E_IO", "残留清单以 E_IO 带出：{err:?}");
    assert!(
        err.message.contains("残留"),
        "错误信息应指明残留：{}",
        err.message
    );

    // 归属边界：子代理历史按 meta 归属、队列随删、用户成果不误伤。
    let dir = tempfile::tempdir().expect("临时目录");
    let app = app_in(dir.path());
    seed_session(&app, "s-main");
    // 子代理历史：本会话的（meta 匹配）与他会的（meta 不匹配）
    let subs = dir.path().join("histories").join("subs");
    let mine = subs.join("run-1");
    let others = subs.join("run-2");
    std::fs::create_dir_all(&mine).expect("建目录");
    std::fs::create_dir_all(&others).expect("建目录");
    std::fs::write(
        mine.join("meta.json"),
        r#"{"version":1,"sessionId":"s-main","subId":"sub-1"}"#,
    )
    .expect("写 meta");
    std::fs::write(
        others.join("meta.json"),
        r#"{"version":1,"sessionId":"s-other","subId":"sub-2"}"#,
    )
    .expect("写 meta");
    // meta 缺失的目录（无法归属）：保守保留
    let orphan = subs.join("run-3");
    std::fs::create_dir_all(&orphan).expect("建目录");
    // 队列文件
    let queue = dir.path().join("sessions").join("s-main.queue.json");
    std::fs::write(&queue, r#"{"version":1,"paused":false,"entries":[]}"#).expect("写队列");
    // 用户工作成果
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("建目录");
    std::fs::write(workspace.join("产出.txt"), "AI 产出").expect("写成果");

    delete_session_core(&app, &RunManager::new(), "s-main").expect("删除成功");

    assert!(!mine.exists(), "本会话的子代理历史应随删");
    assert!(others.exists(), "他会话的子代理历史不误删");
    assert!(orphan.exists(), "无法归属的子历史目录保守保留");
    assert!(!queue.exists(), "followUp 队列文件应随删");
    assert_eq!(
        std::fs::read_to_string(workspace.join("产出.txt")).expect("成果仍在"),
        "AI 产出",
        "工作区文件不随会话删除（工作成果保留）"
    );
}
