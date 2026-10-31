//! storage/stats.rs 的单元测试（20 号票）：日/模型聚合合并幂等、跨日翻
//! 文件、90 天惰性清理、损坏兜底、run/会话明细合并与删除（Q37/Q39/Q40/Q43
//! 存储面）。同型用例在单个 #[test] 内顺序合并，断言消息带用例标识。

use std::fs;

use super::*;

/// 测试数据根：TempDir 保活到测试结束（`_dir` 句柄在返回元组里，drop 即清理）。
fn temp_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    (dir, root)
}

fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> UsageTotal {
    UsageTotal {
        input,
        output,
        cache_read,
        cache_write,
    }
}

// ---- 日/模型聚合与守卫 ----

// 断言：日聚合全语义——同日同模型数字累加且 runs 不重复计次（Q40 幂等）、
// 同日两模型分桶、跨午夜同 run 落次日（Q39）、原子写文件无 tmp 残留、
// 损坏文件备份为 .corrupt 后重建、90 天惰性清理只删过期日、read_range
// 闭区间、非日期命名文件被忽略、shift_day 纯函数表驱动。
#[test]
fn daily_aggregate_and_guards() {
    // 同模型两笔：数字累加、runs 不再 +1（同 run 后续 usage 帧）
    let (_dir, root) = temp_root();
    let store = StatsStore::new(&root);
    let base = UsageRecord {
        day: "2026-10-30",
        model_id: "m1",
        model_name: "gpt-4o",
        delta: usage(100, 10, 40, 5),
        counts_run: true,
    };
    store.record(&base).expect("首次落账");
    store
        .record(&UsageRecord {
            counts_run: false,
            ..base.clone()
        })
        .expect("追加落账");
    let day = store.read_day("2026-10-30");
    let m = day.models.get("m1").expect("模型条目存在");
    assert_eq!(m.input, 200);
    assert_eq!(m.output, 20);
    assert_eq!(m.cache_read, 80);
    assert_eq!(m.cache_write, 10);
    assert_eq!(m.runs, 1, "同 run 后续帧不重复计次（Q40 幂等）");
    assert_eq!(m.model_name, "gpt-4o");

    // 同日两模型分桶 + 跨午夜落次日（独立 store，避免与前段同模型累加）
    let (_dir, root) = temp_root();
    let store = StatsStore::new(&root);
    for (id, name) in [("m1", "gpt-4o"), ("m2", "claude-x")] {
        store
            .record(&UsageRecord {
                day: "2026-10-30",
                model_id: id,
                model_name: name,
                delta: usage(10, 1, 0, 0),
                counts_run: true,
            })
            .expect("落账");
    }
    store
        .record(&UsageRecord {
            day: "2026-10-31",
            model_id: "m1",
            model_name: "gpt-4o",
            delta: usage(5, 0, 0, 0),
            counts_run: true,
        })
        .expect("次日落账");
    assert_eq!(store.list_days(), vec!["2026-10-30", "2026-10-31"]);
    let d30 = store.read_day("2026-10-30");
    assert_eq!(d30.models.len(), 2, "同日两模型分桶");
    assert_eq!(d30.models["m1"].input, 10);
    assert_eq!(d30.models["m1"].runs, 1);
    let d31 = store.read_day("2026-10-31");
    assert_eq!(d31.models["m1"].input, 5, "跨午夜增量落次日（Q39）");
    assert_eq!(d31.models["m1"].runs, 1, "run 属次日的部分也按主 run 计次");

    // 原子写：文件为完整 JSON、无 tmp 残留
    let raw = fs::read_to_string(root.join("stats/2026-10-30.json")).expect("读文件");
    let parsed: DailyStats = serde_json::from_str(&raw).expect("完整 JSON");
    assert_eq!(parsed.version, STATS_FORMAT_VERSION);
    assert_eq!(parsed.day, "2026-10-30");
    let tmp_count = fs::read_dir(root.join("stats"))
        .expect("目录")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(tmp_count, 0, "原子写不残留临时文件");

    // 损坏兜底：原文件备份为 .corrupt，统计从空重建后继续累计
    fs::create_dir_all(root.join("stats")).expect("建目录");
    fs::write(root.join("stats/2026-10-29.json"), "{ 这不是 JSON").expect("写坏文件");
    store
        .record(&UsageRecord {
            day: "2026-10-29",
            model_id: "m1",
            model_name: "gpt-4o",
            delta: usage(1, 0, 0, 0),
            counts_run: true,
        })
        .expect("损坏后重建落账");
    assert!(
        root.join("stats/2026-10-29.json.corrupt").exists(),
        "损坏文件应备份为 .corrupt"
    );
    assert_eq!(
        store.read_day("2026-10-29").models["m1"].input,
        1,
        "损坏文件按空重建后继续累计"
    );

    // 90 天惰性清理：91 天前（2026-07-30/31）被删，90 天整（2026-08-01）保留
    //（独立 store，避免与前段 10-29/10-31 等未过期日混入断言）
    let (_dir, root) = temp_root();
    let store = StatsStore::new(&root);
    for day in ["2026-07-30", "2026-07-31", "2026-08-01", "2026-08-02"] {
        store
            .record(&UsageRecord {
                day,
                model_id: "m1",
                model_name: "gpt-4o",
                delta: usage(1, 0, 0, 0),
                counts_run: true,
            })
            .expect("落账");
    }
    let today = "2026-10-30";
    let removed = store.prune_expired(today, STATS_RETENTION_DAYS);
    assert_eq!(removed, vec!["2026-07-30", "2026-07-31"], "只删过期日");
    assert_eq!(
        store.list_days(),
        vec!["2026-08-01", "2026-08-02"],
        "90 天整保留，更早的被删"
    );

    // read_range：闭区间含两端
    for day in ["2026-10-28", "2026-10-29", "2026-10-30", "2026-10-31"] {
        store
            .record(&UsageRecord {
                day,
                model_id: "m1",
                model_name: "gpt-4o",
                delta: usage(1, 0, 0, 0),
                counts_run: false,
            })
            .expect("落账");
    }
    let range = store.read_range("2026-10-29", "2026-10-30");
    let days: Vec<&str> = range.iter().map(|d| d.day.as_str()).collect();
    assert_eq!(days, vec!["2026-10-29", "2026-10-30"], "闭区间含两端");

    // 非日期命名（垃圾/穿越形态）被 list 忽略
    fs::write(root.join("stats/notes.json"), "{}").expect("垃圾文件");
    fs::write(root.join("stats/../../evil.json"), "{}").expect("穿越形态");
    let store2 = StatsStore::new(&root);
    assert_eq!(
        store2.list_days().len(),
        6,
        "非日期命名被忽略（其余日期照常列出）"
    );

    // shift_day 纯函数：负偏移跨月/正偏移/跨年/非法输入
    assert_eq!(shift_day("2026-10-30", -90).as_deref(), Some("2026-08-01"));
    assert_eq!(shift_day("2026-10-30", 2).as_deref(), Some("2026-11-01"));
    assert_eq!(shift_day("2026-01-01", -1).as_deref(), Some("2025-12-31"));
    assert_eq!(shift_day("垃圾", -1), None);
}

// ---- run/会话四口径明细 ----

// 断言：明细全语义——run 内切模型分桶保留并重算会话累计（Q37/Q40，主/子/
// 辅助全并入）、缺失/损坏文件按空兜底且可继续落账、mark_run_ended 补写
// endedAt 且幂等、forget_session 删除明细（Q43）且对不存在会话幂等。
#[test]
fn usage_detail_semantics() {
    let (_dir, root) = temp_root();
    let store = UsageDetailStore::new(&root);
    // 主 run 第一拍：模型 A
    let total = store
        .record(&RunUsageRecord {
            session_id: "s1",
            run_id: "r1",
            kind: "main",
            model_id: "mA",
            model_name: "GPT-4o",
            delta: usage(100, 20, 30, 0),
        })
        .expect("落账");
    assert_eq!(total, usage(100, 20, 30, 0));
    // 同 run 内切模型（Q37 真实归属）：第二拍模型 B
    let total = store
        .record(&RunUsageRecord {
            session_id: "s1",
            run_id: "r1",
            kind: "main",
            model_id: "mB",
            model_name: "Claude",
            delta: usage(50, 5, 0, 10),
        })
        .expect("落账");
    assert_eq!(total, usage(150, 25, 30, 10));
    // 子 run + 辅助调用并入会话累计（Q40）
    store
        .record(&RunUsageRecord {
            session_id: "s1",
            run_id: "sub-1",
            kind: "sub",
            model_id: "mA",
            model_name: "GPT-4o",
            delta: usage(10, 2, 0, 0),
        })
        .expect("落账");
    let total = store
        .record(&RunUsageRecord {
            session_id: "s1",
            run_id: "r1:suggest",
            kind: "aux",
            model_id: "mA",
            model_name: "GPT-4o",
            delta: usage(3, 1, 0, 0),
        })
        .expect("落账");
    assert_eq!(total, usage(163, 28, 30, 10), "sessionTotal 含主/子/辅助");
    // 读回验证结构
    let loaded = store.load("s1");
    assert_eq!(loaded.runs.len(), 3);
    assert_eq!(loaded.runs["r1"].by_model.len(), 2, "run 内切模型分桶保留");
    assert_eq!(loaded.runs["r1"].by_model["mA"].model_name, "GPT-4o");
    assert_eq!(loaded.runs["sub-1"].kind, "sub");
    assert_eq!(loaded.runs["r1:suggest"].kind, "aux");

    // 缺失/损坏兜底：损坏按空、缺失按空，且兜底后可继续落账
    fs::create_dir_all(root.join("sessions")).expect("建目录");
    fs::write(root.join("sessions/s-x.usage.json"), "broken").expect("写坏");
    assert_eq!(store.load("s-x"), SessionUsage::default(), "损坏按空兜底");
    assert_eq!(
        store.load("s-none"),
        SessionUsage::default(),
        "缺失按空兜底"
    );
    store
        .record(&RunUsageRecord {
            session_id: "s-x",
            run_id: "r1",
            kind: "main",
            model_id: "mA",
            model_name: "GPT-4o",
            delta: usage(1, 0, 0, 0),
        })
        .expect("兜底后落账");
    assert_eq!(
        store.load("s-x").session_total.input,
        1,
        "损坏文件被覆盖重建"
    );

    // 收口与删除：mark_run_ended 补写 endedAt、重复收口幂等、未知 run 静默成功；
    // forget_session 删除明细（Q43）且对不存在会话不报错
    store.mark_run_ended("s1", "r1").expect("收口");
    store.mark_run_ended("s1", "r1").expect("重复收口幂等");
    assert!(
        store.load("s1").runs["r1"].ended_at.is_some(),
        "endedAt 已补写"
    );
    store.mark_run_ended("s1", "r-none").expect("未知 run 收口");
    store.forget_session("s1");
    assert_eq!(store.load("s1"), SessionUsage::default(), "删除明细");
    store.forget_session("s-none"); // 幂等：不存在不报错
}
