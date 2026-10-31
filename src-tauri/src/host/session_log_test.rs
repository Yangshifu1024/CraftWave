//! [`super::SessionLogger`] 的单元测试（32 号票测试口径）：
//! 级别过滤（Debug 全记 / Info 不记 debug / Error 只记 error / 热切换生效）、
//! 轮转语义（`.1` 副本保留 1 份 + 会话写路径触发）、tail 语义（缺省/坏行容错）、
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

/// 会话行写入的便捷封装（附加字段全空）。
fn line(logger: &SessionLogger, session: &str, level: LogLevel, kind: LogKind, msg: &str) {
    logger.session_line(session, level, kind, msg.to_owned(), None, None, None, None);
}

/// 断言级别过滤（Debug 全记 / Info 不记 debug / Error 只记 error / 热切换
/// 生效）与 tail 语义（缺省/坏行容错）。
#[test]
fn level_filter_and_read_tail_semantics() {
    // Debug 级全记：级别与 kind 的序列化形态都落在行里。
    let (logger, dir, _) = setup("debug");
    line(&logger, "s1", LogLevel::Debug, LogKind::Llm, "debug 行");
    line(&logger, "s1", LogLevel::Info, LogKind::Run, "info 行");
    line(&logger, "s1", LogLevel::Error, LogKind::Tool, "error 行");
    let lines = read_lines(&dir, "sessions/s1.jsonl");
    assert_eq!(lines.len(), 3, "Debug 级全记：{lines:?}");
    assert!(
        lines[0].contains(r#""level":"debug""#) && lines[0].contains(r#""kind":"llm""#),
        "行应含级别与 kind 字段：{}",
        lines[0]
    );
    assert!(
        lines[1].contains(r#""kind":"run""#),
        "kind 序列化应正确：{}",
        lines[1]
    );
    assert!(
        lines[2].contains(r#""kind":"tool""#),
        "kind 序列化应正确：{}",
        lines[2]
    );

    // Info 级不记 debug 行。
    let (logger, dir, _) = setup("info");
    line(&logger, "s1", LogLevel::Debug, LogKind::Llm, "请求原文");
    line(&logger, "s1", LogLevel::Info, LogKind::Run, "元数据");
    let lines = read_lines(&dir, "sessions/s1.jsonl");
    assert_eq!(lines.len(), 1, "Info 级不记 debug 行：{lines:?}");
    assert!(lines[0].contains("元数据"), "应只留 info 行：{}", lines[0]);
    assert!(!logger.verbose(), "Info 级 verbose=false");

    // Error 级只记 error（warn 也被滤）。
    let (logger, dir, _) = setup("error");
    line(&logger, "s1", LogLevel::Warn, LogKind::Tool, "警告");
    line(&logger, "s1", LogLevel::Error, LogKind::Run, "失败");
    let lines = read_lines(&dir, "sessions/s1.jsonl");
    assert_eq!(lines.len(), 1, "Error 级只记 error：{lines:?}");

    // 级别热切换：save_config 写盘 + 内存热替换后无需重启立即生效。
    let (logger, dir, config) = setup("info");
    line(&logger, "s1", LogLevel::Debug, LogKind::Llm, "verbose 开前");
    let mut next = AppConfig::default();
    next.log.level = LogLevel::Debug;
    config.save(next).expect("保存配置");
    assert!(logger.verbose(), "热替换后 verbose=true");
    line(&logger, "s1", LogLevel::Debug, LogKind::Llm, "verbose 开后");
    let lines = read_lines(&dir, "sessions/s1.jsonl");
    assert_eq!(lines.len(), 1, "切级前的 debug 行已被过滤：{lines:?}");
    assert!(
        lines[0].contains("verbose 开后"),
        "切级后 debug 行立即可写（无需重启）"
    );

    // tail 语义。
    let (logger, dir, _) = setup("info");
    // 文件不存在 → 空
    assert!(logger.read_tail("none", 500).is_empty());
    // 正常尾部截取
    for i in 0..10 {
        line(
            &logger,
            "s1",
            LogLevel::Info,
            LogKind::Run,
            &format!("行 {i}"),
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

/// 断言轮转与删除联动语义：轮转原语直验（旧文件进 `.1` 副本保留 1 份、
/// 再满覆盖旧副本）；会话写路径超阈值触发轮转后续写新文件；删除联动
/// （主文件与轮转副本同删、关句柄后不再复活文件、失败报告面不谎报成功）。
#[test]
fn rotate_and_close_purge_semantics() {
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

    // 会话写路径：写手打开时 seek 到文件尾（字节计数 = 现有大小），下一行
    // 写入即触发轮转，之后续写新文件。
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
    line(
        &logger,
        "s1",
        LogLevel::Info,
        LogKind::Run,
        "触发轮转的一行",
    );
    line(
        &logger,
        "s1",
        LogLevel::Info,
        LogKind::Run,
        "轮转后续写的一行",
    );
    assert!(
        sessions.join("s1.jsonl.1").exists(),
        "超阈值触发轮转出 .1 副本"
    );
    let current = std::fs::read_to_string(sessions.join("s1.jsonl")).expect("新文件");
    assert!(current.contains("触发轮转的一行"), "轮转后新文件续写");
    assert!(current.contains("轮转后续写的一行"));
    assert_eq!(current.lines().count(), 2, "新文件只含轮转后的行");

    // 删除联动：主文件与轮转副本同删。
    let (logger, dir, _) = setup("debug");
    line(&logger, "s1", LogLevel::Info, LogKind::Run, "一行");
    let sessions = dir.path().join("logs").join("sessions");
    std::fs::write(sessions.join("s1.jsonl.1"), "旧副本").expect("预置轮转副本");
    let failures = logger.close_and_purge("s1");
    assert!(failures.is_empty(), "两个文件都应删净：{failures:?}");
    assert!(!sessions.join("s1.jsonl").exists());
    assert!(!sessions.join("s1.jsonl.1").exists());
    // 关句柄后不再复活文件
    line(&logger, "s1", LogLevel::Info, LogKind::Run, "不应复活");
    assert!(
        !sessions.join("s1.jsonl").exists(),
        "删除后写手已摘除，不再写文件"
    );
    // 不存在的会话：空失败清单（幂等）
    assert!(logger.close_and_purge("ghost").is_empty());
    // 删除失败报告：把 jsonl 换成目录使 remove_file 失败
    std::fs::create_dir_all(sessions.join("s2.jsonl")).expect("造目录占位");
    line(&logger, "s2", LogLevel::Info, LogKind::Run, "造文件");
    let failures = logger.close_and_purge("s2");
    assert!(
        !failures.is_empty(),
        "删除失败必须报告，不能声称全部成功：{failures:?}"
    );
}
