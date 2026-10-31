//! storage/index_db.rs 的单元测试（20 号票）：建库校验、损坏/版本不符
//! 重建、写穿 upsert、范围查询与全量回填、降级路径（§7.8 三铁律）。
//! 全生命周期合并为单个 #[test]，断言消息带阶段标识。

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

// 断言：索引库全生命周期——正常建库不降级（库文件在 cache/ 下、DDL 两表
// 俱建、版本号与 runs 回写列齐备）；垃圾字节损坏库被丢弃重建后可用；版本
// 不符触发重建归位（空库可回填）；upsert 覆盖语义 + 范围闭区间查询；回填
// 先清空旧表再写全量；cache/ 建目录失败时降级不 panic（写静默、查询 None）。
#[test]
fn index_db_lifecycle() {
    // 建库：版本、DDL、runs 列
    let (_dir, root) = temp_root();
    let db = open(&root);
    assert!(!db.degraded(), "正常建库不降级");
    assert!(root.join("cache/index.db").exists(), "库文件在 cache/ 下");
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
    let runs_type: String = conn
        .query_row(
            "SELECT type FROM pragma_table_info('stats_cache') WHERE name='runs'",
            [],
            |r| r.get(0),
        )
        .expect("runs 列存在");
    assert_eq!(runs_type, "INTEGER", "runs 列存在（20 号票回写）");
    drop(db);

    // 损坏重建：垃圾字节库被丢弃，重建后可写可查
    let db_path = root.join("cache/index.db");
    fs::write(&db_path, vec![0xFFu8; 8192]).expect("写坏库文件");
    let db = open(&root);
    assert!(!db.degraded(), "损坏库被重建后可用");
    db.upsert_stats(&row("2026-10-30", "m1", 5, 1));
    let hit = db.query_stats_range("2026-10-30", "2026-10-30");
    assert!(hit.is_some(), "重建后查询可用");
    assert_eq!(hit.expect("rows").len(), 1);
    drop(db);

    // 版本不符：重建归位、旧表数据清空（可从 stats/*.json 回填）
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
    let conn = Connection::open(&db_path).expect("重开");
    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("版本");
    assert_eq!(version, INDEX_DB_VERSION, "重建后版本归位");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM stats_cache", [], |r| r.get(0))
        .expect("查询");
    assert_eq!(count, 0, "重建 = 空库（可从 stats/*.json 回填）");
    drop(conn);

    // upsert 覆盖 + 范围闭区间查询
    db.upsert_stats(&row("2026-10-29", "m1", 10, 1));
    db.upsert_stats(&row("2026-10-30", "m1", 20, 1));
    db.upsert_stats(&row("2026-10-30", "m2", 30, 2));
    db.upsert_stats(&row("2026-10-31", "m1", 40, 1));
    db.upsert_stats(&row("2026-10-30", "m1", 25, 2)); // 同 (day, model) 再写为替换
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

    // 全量回填：先清空旧表再写全量
    db.upsert_stats(&row("2026-10-01", "m1", 1, 1));
    db.backfill_stats(&[row("2026-10-02", "m1", 2, 1), row("2026-10-03", "m2", 3, 5)]);
    let rows = db
        .query_stats_range("2026-10-01", "2026-10-31")
        .expect("命中");
    assert_eq!(rows.len(), 2, "回填先清空旧表再写全量");
    assert_eq!(rows[0], row("2026-10-02", "m1", 2, 1));
    drop(db);

    // 降级路径：cache/ 被同名文件占位 → 建目录失败 → 降级不 panic
    let (_dir, root) = temp_root();
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
