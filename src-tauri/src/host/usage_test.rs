//! host/usage.rs 的单元测试（20 号票）：三面落账同源、跨午夜拆日、切时区
//! 不重算、缓存命中与降级直读一致、90 天惰性清理联动。

use std::fs;

use super::*;
use crate::backend::frames::UsageTotal;

fn temp_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    (dir, root)
}

fn delta(input: u64, output: u64) -> UsageTotal {
    UsageTotal {
        input,
        output,
        cache_read: 0,
        cache_write: 0,
    }
}

/// 主 run 首帧落账的便捷封装（counts_run=true）。
fn main_record(
    ledger: &UsageLedger,
    day: &str,
    session: &str,
    run: &str,
    model: &str,
    d: UsageTotal,
) {
    ledger.record_on_day(day, session, run, run_kind::MAIN, model, "显示名", d, true);
}

#[test]
fn record_writes_all_three_faces() {
    let (_dir, root) = temp_root();
    let ledger = UsageLedger::new(&root);
    main_record(&ledger, "2026-10-30", "s1", "r1", "m1", delta(100, 10));
    // ① 聚合面
    let day = ledger.stats.read_day("2026-10-30");
    assert_eq!(day.models["m1"].input, 100);
    assert_eq!(day.models["m1"].runs, 1);
    // ② 明细面（Q37）
    let detail = ledger.detail.load("s1");
    assert_eq!(detail.session_total.input, 100);
    assert_eq!(detail.runs["r1"].kind, "main");
    // ③ 缓存面（写穿）
    let rows = ledger
        .cache
        .query_stats_range("2026-10-30", "2026-10-30")
        .expect("缓存可用");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].usage.input, 100);
    assert_eq!(rows[0].runs, 1);
}

#[test]
fn sub_and_aux_never_count_runs_but_count_usage() {
    let (_dir, root) = temp_root();
    let ledger = UsageLedger::new(&root);
    main_record(&ledger, "2026-10-30", "s1", "r1", "m1", delta(50, 5));
    // 子 run 两拍（首帧也不计次，Q40）
    ledger.record_on_day(
        "2026-10-30",
        "s1",
        "sub-1",
        run_kind::SUB,
        "m1",
        "显示名",
        delta(10, 1),
        false,
    );
    // 辅助调用（suggest）
    ledger.record_on_day(
        "2026-10-30",
        "s1",
        "r1:suggest",
        run_kind::AUX,
        "m1",
        "显示名",
        delta(3, 1),
        false,
    );
    let day = ledger.stats.read_day("2026-10-30");
    assert_eq!(
        day.models["m1"].runs, 1,
        "子 run 与辅助调用不增 runs（Q40）"
    );
    assert_eq!(day.models["m1"].input, 63, "用量照计");
    let detail = ledger.detail.load("s1");
    assert_eq!(detail.session_total.input, 63, "会话累计含主/子/辅助");
}

#[test]
fn midnight_split_and_timezone_never_rewrites_history() {
    let (_dir, root) = temp_root();
    let ledger = UsageLedger::new(&root);
    // 跨午夜 run：前半程落 10-30、后半程落 10-31（Q39 拆日）
    main_record(&ledger, "2026-10-30", "s1", "r1", "m1", delta(40, 4));
    ledger.record_on_day(
        "2026-10-31",
        "s1",
        "r1",
        run_kind::MAIN,
        "m1",
        "显示名",
        delta(7, 1),
        false, // 后续帧不重复计次
    );
    let d30 = ledger.stats.read_day("2026-10-30");
    let d31 = ledger.stats.read_day("2026-10-31");
    assert_eq!(d30.models["m1"].input, 40);
    assert_eq!(d31.models["m1"].input, 7);
    assert_eq!(d30.models["m1"].runs, 1);
    assert_eq!(d31.models["m1"].runs, 0, "run 次数只在首个有计量日计一次");
    // 切时区不重划历史：历史日的数字不因后续「今天」变化而搬移——
    // 直接对历史日再查询/落账后，前日数据保持不变
    main_record(&ledger, "2026-11-15", "s2", "r2", "m1", delta(1, 0));
    assert_eq!(ledger.stats.read_day("2026-10-30").models["m1"].input, 40);
    assert_eq!(ledger.stats.read_day("2026-10-31").models["m1"].input, 7);
}

#[test]
fn query_range_prefers_cache_and_refills_when_missing() {
    let (_dir, root) = temp_root();
    let ledger = UsageLedger::new(&root);
    main_record(&ledger, "2026-10-30", "s1", "r1", "m1", delta(100, 10));
    // 正常路径：缓存命中（写穿）与真相面一致
    let via_cache = ledger.query_range("2026-10-01", "2026-10-31");
    assert_eq!(via_cache.len(), 1);
    assert_eq!(via_cache[0].models["m1"].input, 100);
    assert_eq!(via_cache[0].models["m1"].model_name, "显示名");
    // 模拟库重建（清空缓存表）：查询发现缺日 → 回填 → 数字不丢
    ledger.cache.backfill_stats(&[]);
    let after_backfill = ledger.query_range("2026-10-01", "2026-10-31");
    assert_eq!(after_backfill[0].models["m1"].input, 100, "回填后数字一致");
}

#[test]
fn degraded_cache_falls_back_to_truth_files() {
    let (_dir, root) = temp_root();
    let ledger = UsageLedger::new(&root);
    main_record(&ledger, "2026-10-30", "s1", "r1", "m1", delta(100, 10));
    // 占位文件让 cache/ 目录不可重建 → 降级态
    // （重新建一个 ledger 走降级路径：cache/ 变成文件后 open 降级）
    fs::remove_dir_all(root.join("cache")).expect("清缓存");
    fs::write(root.join("cache"), "not a dir").expect("占位");
    let degraded = UsageLedger::new(&root);
    assert!(degraded.cache.degraded());
    let truth_only = degraded.query_range("2026-10-01", "2026-10-31");
    assert_eq!(truth_only.len(), 1, "降级直读 stats/*.json");
    assert_eq!(truth_only[0].models["m1"].input, 100);
}

#[test]
fn retention_prune_runs_alongside_record() {
    let (_dir, root) = temp_root();
    let ledger = UsageLedger::new(&root);
    // 直接手写一个 100 天前的统计文件（绕过 record 的今天日期）
    let old_day = crate::storage::stats::shift_day("2026-10-30", -100).expect("合法日期");
    fs::create_dir_all(root.join("stats")).expect("目录");
    fs::write(
        root.join(format!("stats/{old_day}.json")),
        format!(r#"{{"version":1,"day":"{old_day}","models":{{}}}}"#),
    )
    .expect("写旧文件");
    main_record(&ledger, "2026-10-30", "s1", "r1", "m1", delta(1, 0));
    assert!(
        !root.join(format!("stats/{old_day}.json")).exists(),
        "落账顺带清理过期日（91 天窗口外即删）"
    );
}

#[test]
fn session_deletion_clears_detail_but_keeps_anonymous_stats() {
    let (_dir, root) = temp_root();
    let ledger = UsageLedger::new(&root);
    main_record(&ledger, "2026-10-30", "s1", "r1", "m1", delta(9, 1));
    // Q43：删除会话明细 → 日/模型匿名数字保留
    ledger.detail.forget_session("s1");
    assert_eq!(
        ledger.detail.load("s1"),
        crate::storage::stats::SessionUsage::default()
    );
    assert_eq!(
        ledger.query_range("2026-10-01", "2026-10-31")[0].models["m1"].input,
        9,
        "聚合数字保留（无会话关联）"
    );
}
