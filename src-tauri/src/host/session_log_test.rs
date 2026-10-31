//! [`super::SessionLogger`] 的单元测试（32 号票测试口径）：
//! 级别过滤（Debug 全记 / Info 不记 debug / Error 只记 error）、轮转触发
//! （写超阈值出 `.1` 且续写新文件）、tail 语义（缺省/空文件/坏行容错）、
//! 删除联动（主文件与轮转副本同删、失败报告面）。

use super::*;
use crate::providers::types::AppConfig;
use std::sync::Arc;

/// 在临时目录构造（日志器, 日志根, 配置缓存）：配置文件预写 `level` 级别。
fn setup(
    level: &str,
) -> (
    SessionLogger,
    tempfile::TempDir,
    Arc<ConfigStore<AppConfig>>,
) {
    let dir = tempfile::tempdir().expect("临时目录");
    let config_path = dir.path().join("config.json");
    std::fs::write(
        &config_path,
        format!(r#"{{ "log": {{ "level": "{level}" }} }}"#),
    )
    .expect("写配置");
    let config = Arc::new(ConfigStore::<AppConfig>::new(config_path));
    let root = dir.path().join("logs");
    let logger = SessionLogger::new(root.clone(), config.clone());
    (logger, dir, config)
}

/// 读回会话日志文件的行清单。
fn read_lines(dir: &tempfile::TempDir, name: &str) -> Vec<String> {
    std::fs::read_to_string(dir.path().join("logs").join(name))
        .expect("日志文件应存在")
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn level_filter_debug_logs_all() {
    let (logger, dir, _) = setup("debug");
    logger.session_line(
        "s1",
        LogLevel::Debug,
        LogKind::Llm,
        "debug 行".to_owned(),
        None,
        None,
        None,
        None,
    );
    logger.session_line(
        "s1",
        LogLevel::Info,
        LogKind::Run,
        "info 行".to_owned(),
        None,
        None,
        None,
        None,
    );
    logger.session_line(
        "s1",
        LogLevel::Error,
        LogKind::Tool,
        "error 行".to_owned(),
        None,
        None,
        None,
        None,
    );
    let lines = read_lines(&dir, "sessions/s1.jsonl");
    assert_eq!(lines.len(), 3, "Debug 级全记：{lines:?}");
    assert!(lines[0].contains(r#""level":"debug""#));
    assert!(lines[0].contains(r#""kind":"llm""#));
    assert!(lines[1].contains(r#""kind":"run""#));
    assert!(lines[2].contains(r#""kind":"tool""#));
}

#[test]
fn level_filter_info_drops_debug() {
    let (logger, dir, _) = setup("info");
    logger.session_line(
        "s1",
        LogLevel::Debug,
        LogKind::Llm,
        "请求原文".to_owned(),
        None,
        None,
        None,
        None,
    );
    logger.session_line(
        "s1",
        LogLevel::Info,
        LogKind::Run,
        "元数据".to_owned(),
        None,
        None,
        None,
        None,
    );
    let lines = read_lines(&dir, "sessions/s1.jsonl");
    assert_eq!(lines.len(), 1, "Info 级不记 debug 行：{lines:?}");
    assert!(lines[0].contains("元数据"));
    assert!(!logger.verbose(), "Info 级 verbose=false");
}

#[test]
fn level_filter_error_only_errors() {
    let (logger, dir, _) = setup("error");
    logger.session_line(
        "s1",
        LogLevel::Warn,
        LogKind::Tool,
        "警告".to_owned(),
        None,
        None,
        None,
        None,
    );
    logger.session_line(
        "s1",
        LogLevel::Error,
        LogKind::Run,
        "失败".to_owned(),
        None,
        None,
        None,
        None,
    );
    let lines = read_lines(&dir, "sessions/s1.jsonl");
    assert_eq!(lines.len(), 1, "Error 级只记 error：{lines:?}");
}

#[test]
fn level_change_takes_effect_without_restart() {
    let (logger, dir, config) = setup("info");
    logger.session_line(
        "s1",
        LogLevel::Debug,
        LogKind::Llm,
        "verbose 开前".to_owned(),
        None,
        None,
        None,
        None,
    );
    // 模拟 save_config 热替换（save 写盘 + 内存缓存热替换，级别切 Debug）
    let mut next = AppConfig::default();
    next.log.level = LogLevel::Debug;
    config.save(next).expect("保存配置");
    assert!(logger.verbose(), "热替换后 verbose=true");
    logger.session_line(
        "s1",
        LogLevel::Debug,
        LogKind::Llm,
        "verbose 开后".to_owned(),
        None,
        None,
        None,
        None,
    );
    let lines = read_lines(&dir, "sessions/s1.jsonl");
    assert_eq!(lines.len(), 1, "切级前的 debug 行已被过滤：{lines:?}");
    assert!(
        lines[0].contains("verbose 开后"),
        "切级后 debug 行立即可写（无需重启）"
    );
}

#[test]
fn rotate_replaces_single_copy() {
    // 轮转原语直验：旧文件进 `.1`（保留 1 份）；再次轮转覆盖旧副本。
    let (logger, dir, _) = setup("info");
    // 触发应用日志文件先落一个（目录就绪）
    logger.app_line(LogLevel::Info, "先落一行".to_owned(), None);
    let path = dir.path().join("logs").join("app.log");
    std::fs::write(&path, "旧内容").expect("预写");
    super::rotate_file(&path);
    assert!(!path.exists(), "当前文件被改名走");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("logs").join("app.log.1")).expect("副本"),
        "旧内容",
        "旧文件进 .1 副本"
    );
    // 再满覆盖：已有 .1 时再次轮转覆盖旧副本
    std::fs::write(&path, "第二批").expect("预写");
    super::rotate_file(&path);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("logs").join("app.log.1")).expect("副本"),
        "第二批",
        "再满覆盖旧副本（保留 1 份）"
    );
}

#[test]
fn session_rotate_via_write_path() {
    // 用会话通道 + 手工预置一个已超阈值的文件：写手打开时 seek 到文件尾
    // （字节计数 = 现有大小），下一行写入即触发轮转。
    let (logger, dir, _) = setup("debug");
    let sessions = dir.path().join("logs").join("sessions");
    std::fs::create_dir_all(&sessions).expect("建目录");
    let big = "y".repeat(1024);
    let mut prelude = String::new();
    for _ in 0..(10 * 1024 * 1024 / 1024) {
        prelude.push_str(&big);
        prelude.push('\n');
    }
    std::fs::write(sessions.join("s1.jsonl"), &prelude).expect("预置大文件");
    logger.session_line(
        "s1",
        LogLevel::Info,
        LogKind::Run,
        "触发轮转的一行".to_owned(),
        None,
        None,
        None,
        None,
    );
    logger.session_line(
        "s1",
        LogLevel::Info,
        LogKind::Run,
        "轮转后续写的一行".to_owned(),
        None,
        None,
        None,
        None,
    );
    assert!(
        sessions.join("s1.jsonl.1").exists(),
        "超阈值触发轮转出 .1 副本"
    );
    let current = std::fs::read_to_string(sessions.join("s1.jsonl")).expect("新文件");
    assert!(current.contains("触发轮转的一行"), "轮转后新文件续写");
    assert!(current.contains("轮转后续写的一行"));
    assert_eq!(current.lines().count(), 2, "新文件只含轮转后的行");
}

#[test]
fn read_tail_semantics() {
    let (logger, dir, _) = setup("info");
    // 文件不存在 → 空
    assert!(logger.read_tail("none", 500).is_empty());
    // 正常尾部截取
    for i in 0..10 {
        logger.session_line(
            "s1",
            LogLevel::Info,
            LogKind::Run,
            format!("行 {i}"),
            None,
            None,
            None,
            None,
        );
    }
    let tail3 = logger.read_tail("s1", 3);
    assert_eq!(tail3.len(), 3);
    assert!(tail3[0].contains("行 7"));
    assert!(tail3[2].contains("行 9"));
    // tail 大于总行数 → 全部
    assert_eq!(logger.read_tail("s1", 500).len(), 10);
    // 坏行容错：半行原样带出（诊断面不给读制造障碍）
    let path = dir.path().join("logs").join("sessions").join("s1.jsonl");
    let mut raw = std::fs::read_to_string(&path).expect("读回");
    raw.push_str("{\"ts\": \"半行坏数据");
    std::fs::write(&path, raw).expect("写回坏行");
    let tail = logger.read_tail("s1", 1);
    assert_eq!(tail.len(), 1);
    assert!(tail[0].starts_with("{\"ts\": \"半行坏数据"), "坏行原样输出");
}

#[test]
fn close_and_purge_removes_rotated_copy_and_reports_failures() {
    let (logger, dir, _) = setup("debug");
    logger.session_line(
        "s1",
        LogLevel::Info,
        LogKind::Run,
        "一行".to_owned(),
        None,
        None,
        None,
        None,
    );
    let sessions = dir.path().join("logs").join("sessions");
    std::fs::write(sessions.join("s1.jsonl.1"), "旧副本").expect("预置轮转副本");
    let failures = logger.close_and_purge("s1");
    assert!(failures.is_empty(), "两个文件都应删净：{failures:?}");
    assert!(!sessions.join("s1.jsonl").exists());
    assert!(!sessions.join("s1.jsonl.1").exists());
    // 关句柄后不再复活文件
    logger.session_line(
        "s1",
        LogLevel::Info,
        LogKind::Run,
        "不应复活".to_owned(),
        None,
        None,
        None,
        None,
    );
    assert!(
        !sessions.join("s1.jsonl").exists(),
        "删除后写手已摘除，不再写文件"
    );
    // 不存在的会话：空失败清单（幂等）
    assert!(logger.close_and_purge("ghost").is_empty());
    // 删除失败报告：把 jsonl 换成目录使 remove_file 失败
    std::fs::create_dir_all(sessions.join("s2.jsonl")).expect("造目录占位");
    logger.session_line(
        "s2",
        LogLevel::Info,
        LogKind::Run,
        "造文件".to_owned(),
        None,
        None,
        None,
        None,
    );
    let failures = logger.close_and_purge("s2");
    assert!(
        !failures.is_empty(),
        "删除失败必须报告，不能声称全部成功：{failures:?}"
    );
}

#[test]
fn list_files_and_app_log() {
    let (logger, dir, _) = setup("info");
    assert!(logger.list_files().is_empty(), "未写过日志 → 空清单");
    logger.app_line(LogLevel::Info, "启动序".to_owned(), None);
    logger.session_line(
        "s1",
        LogLevel::Info,
        LogKind::Run,
        "一行".to_owned(),
        None,
        None,
        None,
        None,
    );
    std::fs::write(dir.path().join("logs").join("app.log.1"), "旧副本").expect("副本");
    let files = logger.list_files();
    assert_eq!(
        files,
        vec!["app.log", "app.log.1", "sessions/s1.jsonl"],
        "相对名清单（app.log* + sessions/*）：{files:?}"
    );
    let app = read_lines(&dir, "app.log");
    assert_eq!(app.len(), 1);
    assert!(app[0].contains(r#""msg":"启动序""#));
    assert!(!app[0].contains("kind"), "应用行无 kind 字段");
}
