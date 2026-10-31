//! host/usage.rs 的单元测试（20 号票）：三面落账同源与 run 计次、跨午夜
//! 拆日与历史不重算（含删除明细保留匿名聚合）、缓存命中/回填/降级与 90 天
//! 惰性清理联动。

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

/// 断言三面落账同源与 run 计次（Q40：子 run 与辅助调用不增 runs、用量照计、
/// 缓存写穿面按日/模型聚合），以及跨午夜拆日（Q39）与历史不重算——
/// 后续帧不重复计次、run 次数只在首个有计量日计一次、切时区不搬移历史日
/// 数字、Q43 删除会话明细后日/模型匿名数字保留。
#[test]
fn record_faces_run_counting_and_midnight_split() {
    let (_dir, root) = temp_root();
    let ledger = UsageLedger::new(&root);
    main_record(&ledger, "2026-10-30", "s1", "r1", "m1", delta(50, 5));
    // 子 run 两拍（首帧也不计次，Q40）与辅助调用（suggest）。
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
    // ① 聚合面：runs 只算主 run，用量含主/子/辅助。
    let day = ledger.stats.read_day("2026-10-30");
    assert_eq!(
        day.models["m1"].runs, 1,
        "子 run 与辅助调用不增 runs（Q40）"
    );
    assert_eq!(day.models["m1"].input, 63, "用量照计");
    // ② 明细面（Q37）：会话累计含主/子/辅助，run kind 归档正确。
    let detail = ledger.detail.load("s1");
    assert_eq!(detail.session_total.input, 63, "会话累计含主/子/辅助");
    assert_eq!(detail.runs["r1"].kind, "main");
    // ③ 缓存面（写穿）：行为日/模型聚合（同日多会话多 run 合并一行）。
    main_record(&ledger, "2026-10-30", "s2", "r2", "m1", delta(100, 10));
    let rows = ledger
        .cache
        .query_stats_range("2026-10-30", "2026-10-30")
        .expect("缓存可用");
    assert_eq!(rows.len(), 1, "缓存面按日/模型聚合：{rows:?}");
    assert_eq!(
        rows[0].usage.input, 163,
        "四笔（主+子+辅+主）合并 50+10+3+100"
    );
    assert_eq!(rows[0].runs, 2, "两次主 run 计次");

    // 跨午夜 run：前半程落 10-30、后半程落 10-31（Q39 拆日）
    let (_dir, root) = temp_root();
    let ledger = UsageLedger::new(&root);
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
    // Q43：删除会话明细 → 日/模型匿名数字保留（逐日 40 与 7 都在）
    ledger.detail.forget_session("s1");
    assert_eq!(
        ledger.detail.load("s1"),
        crate::storage::stats::SessionUsage::default()
    );
    let rows = ledger.query_range("2026-10-01", "2026-10-31");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].models["m1"].input, 40, "聚合数字保留（无会话关联）");
    assert_eq!(rows[1].models["m1"].input, 7);
}

#[test]
fn query_cache_backfill_degraded_and_retention_prune() {
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

    // 占位文件让 cache/ 目录不可重建 → 降级态直读 stats/*.json
    fs::remove_dir_all(root.join("cache")).expect("清缓存");
    fs::write(root.join("cache"), "not a dir").expect("占位");
    let degraded = UsageLedger::new(&root);
    assert!(degraded.cache.degraded(), "cache/ 被文件占位后应进降级态");
    let truth_only = degraded.query_range("2026-10-01", "2026-10-31");
    assert_eq!(truth_only.len(), 1, "降级直读 stats/*.json");
    assert_eq!(truth_only[0].models["m1"].input, 100);

    // 90 天惰性清理：手写一个 100 天前的统计文件（绕过 record 的今天日期），
    // 落账顺带清理过期日（91 天窗口外即删）。
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
        "落账顺带清理过期日"
    );
}
