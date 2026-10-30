//! storage/index_db.rs 的单元测试（20 号票）：建库校验、损坏/版本不符
//! 重建、写穿 upsert、范围查询与全量回填、降级路径（§7.8 三铁律）。

use std::fs;

use super::*;

fn temp_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    (dir, root)
}

fn row(day: &str, model_id: &str, input: u64, runs: u64) -> StatsCacheRow {
    StatsCacheRow {
        day: day.to_owned(),
        model_id: model_id.to_owned(),
        usage: UsageTotal {
            input,
            output: 1,
            cache_read: 0,
            cache_write: 0,
        },
        runs,
    }
}

#[test]
fn open_creates_schema_and_version() {
    let (_dir, root) = temp_root();
    let db = open(&root);
    assert!(!db.degraded(), "正常建库不降级");
    assert!(root.join("cache/index.db").exists(), "库文件在 cache/ 下");
    // 直接开连接验证 DDL（逐字 §7.8 + runs 回写列）与版本号
    let conn = Connection::open(root.join("cache/index.db")).expect("二次打开");
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("版本");
    assert_eq!(version, INDEX_DB_VERSION);
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('sessions','stats_cache')",
            [],
            |r| r.get(0),
        )
        .expect("表存在");
    assert_eq!(count, 2, "sessions 与 stats_cache 两表俱建");
    // runs 列存在（20 号票回写）
    let runs_type: String = conn
        .query_row(
            "SELECT type FROM pragma_table_info('stats_cache') WHERE name='runs'",
            [],
            |r| r.get(0),
        )
        .expect("runs 列存在");
    assert_eq!(runs_type, "INTEGER");
}

#[test]
fn corrupt_db_is_discarded_and_rebuilt() {
    let (_dir, root) = temp_root();
    // 先建一个正常库
    drop(open(&root));
    let db_path = root.join("cache/index.db");
    // 写入垃圾字节模拟损坏（SQLite 文件头之后为非法页内容）
    fs::write(&db_path, vec![0xFFu8; 8192]).expect("写坏库文件");
    let db = open(&root);
    assert!(!db.degraded(), "损坏库被重建后可用");
    db.upsert_stats(&row("2026-10-30", "m1", 5, 1));
    let hit = db.query_stats_range("2026-10-30", "2026-10-30");
    assert!(hit.is_some(), "重建后查询可用");
    assert_eq!(hit.expect("rows").len(), 1);
}

#[test]
fn wrong_user_version_triggers_rebuild() {
    let (_dir, root) = temp_root();
    let db_path = root.join("cache/index.db");
    fs::create_dir_all(root.join("cache")).expect("建 cache 目录");
    {
        let conn = Connection::open(&db_path).expect("打开");
        conn.execute_batch(SCHEMA_SQL).expect("建表");
        conn.execute_batch("PRAGMA user_version = 99;")
            .expect("写错版本");
    }
    let db = open(&root);
    assert!(!db.degraded(), "版本不符重建成功");
    // 重建后版本归位（旧表数据清空）
    let conn = Connection::open(&db_path).expect("重开");
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("版本");
    assert_eq!(version, INDEX_DB_VERSION);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM stats_cache", [], |r| r.get(0))
        .expect("查询");
    assert_eq!(count, 0, "重建 = 空库（可从 stats/*.json 回填）");
}

#[test]
fn upsert_and_range_query_roundtrip() {
    let (_dir, root) = temp_root();
    let db = open(&root);
    db.upsert_stats(&row("2026-10-29", "m1", 10, 1));
    db.upsert_stats(&row("2026-10-30", "m1", 20, 1));
    db.upsert_stats(&row("2026-10-30", "m2", 30, 2));
    db.upsert_stats(&row("2026-10-31", "m1", 40, 1));
    // upsert 覆盖语义：同 (day, model) 再写为替换
    db.upsert_stats(&row("2026-10-30", "m1", 25, 2));
    let rows = db
        .query_stats_range("2026-10-29", "2026-10-31")
        .expect("命中");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0], row("2026-10-29", "m1", 10, 1));
    assert_eq!(rows[1], row("2026-10-30", "m1", 25, 2), "覆盖生效");
    assert_eq!(rows[2].model_id, "m2");
    assert_eq!(rows[3], row("2026-10-31", "m1", 40, 1), "闭区间含上界端点");
    let outside = db
        .query_stats_range("2026-11-01", "2026-12-01")
        .expect("命中");
    assert!(outside.is_empty(), "界外为空集");
}

#[test]
fn backfill_restores_full_table() {
    let (_dir, root) = temp_root();
    let db = open(&root);
    db.upsert_stats(&row("2026-10-01", "m1", 1, 1));
    // 模拟库重建（清空）
    db.backfill_stats(&[row("2026-10-02", "m1", 2, 1), row("2026-10-03", "m2", 3, 5)]);
    let rows = db
        .query_stats_range("2026-10-01", "2026-10-31")
        .expect("命中");
    assert_eq!(rows.len(), 2, "回填先清空旧表再写全量");
    assert_eq!(rows[0], row("2026-10-02", "m1", 2, 1));
}

#[test]
fn unavailable_cache_dir_degrades_gracefully() {
    let (_dir, root) = temp_root();
    // cache/ 被同名文件占位 → 建目录失败 → 降级不 panic
    fs::create_dir_all(&root).expect("根目录");
    fs::write(root.join("cache"), "not a dir").expect("占位文件");
    let db = open(&root);
    assert!(db.degraded(), "建目录失败降级");
    db.upsert_stats(&row("2026-10-30", "m1", 1, 1)); // 降级静默
    assert!(
        db.query_stats_range("2026-10-01", "2026-10-31").is_none(),
        "降级查询 None"
    );
    assert!(db.path().ends_with("index.db"));
}
