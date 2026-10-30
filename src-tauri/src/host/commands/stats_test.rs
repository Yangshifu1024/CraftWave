//! host/commands/stats.rs 的单元测试（20 号票）：#52 range 校验与聚合形状、
//! #59k 恢复读面的三级回落与 Q43 删除语义。

use super::*;

/// 装配测试 AppState（临时目录 + 录音事件汇）。
fn app_fixture() -> (tempfile::TempDir, AppState) {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = AppState::new_in(
        dir.path().to_path_buf(),
        std::sync::Arc::new(crate::util::events::RecordingEventSink::new()),
    );
    (dir, app)
}

fn range(from: &str, to: &str) -> Option<StatsRange> {
    Some(StatsRange {
        from: from.to_owned(),
        to: to.to_owned(),
    })
}

#[test]
fn stats_token_rejects_bad_range() {
    let (_dir, app) = app_fixture();
    // 非法格式
    let err = stats_token_core(
        &app,
        &StatsTokenArgs {
            range: range("2026-1-30", "2026-10-31"),
        },
        "2026-10-31",
    )
    .expect_err("格式非法应拒");
    assert_eq!(err.code, E_INVALID_ARG);
    // from > to
    let err = stats_token_core(
        &app,
        &StatsTokenArgs {
            range: range("2026-10-31", "2026-10-01"),
        },
        "2026-10-31",
    )
    .expect_err("from>to 应拒");
    assert_eq!(err.code, E_INVALID_ARG);
}

#[test]
fn stats_token_aggregates_by_day_and_model() {
    let (_dir, app) = app_fixture();
    // 两日两模型三笔（主 run 计次/子 run/辅助）
    app.ledger.record_on_day(
        "2026-10-29",
        "s1",
        "r1",
        crate::host::usage::run_kind::MAIN,
        "m1",
        "GPT-4o",
        UsageTotal {
            input: 100,
            output: 10,
            cache_read: 20,
            cache_write: 5,
        },
        true,
    );
    app.ledger.record_on_day(
        "2026-10-30",
        "s1",
        "sub-1",
        crate::host::usage::run_kind::SUB,
        "m2",
        "Claude",
        UsageTotal {
            input: 50,
            output: 5,
            cache_read: 0,
            cache_write: 0,
        },
        false,
    );
    app.ledger.record_on_day(
        "2026-10-30",
        "s1",
        "r1:suggest",
        crate::host::usage::run_kind::AUX,
        "m1",
        "GPT-4o",
        UsageTotal {
            input: 3,
            output: 1,
            cache_read: 0,
            cache_write: 0,
        },
        false,
    );
    let result = stats_token_core(
        &app,
        &StatsTokenArgs {
            range: range("2026-10-01", "2026-10-31"),
        },
        "2026-10-30",
    )
    .expect("查询成功");
    // byDay 两行，逐日合计
    assert_eq!(result.by_day.len(), 2);
    assert_eq!(result.by_day[0].day, "2026-10-29");
    assert_eq!(result.by_day[0].total, 135);
    assert_eq!(result.by_day[1].day, "2026-10-30");
    assert_eq!(result.by_day[1].total, 59);
    // byModel 两桶：runs 只统计主 run（Q40）
    let m1 = result
        .by_model
        .iter()
        .find(|m| m.model_id == "m1")
        .expect("m1");
    assert_eq!(m1.model_name, "GPT-4o");
    assert_eq!(m1.input, 103);
    assert_eq!(m1.total, 139);
    assert_eq!(m1.runs, 1, "子 run 与辅助调用不增 runs");
    let m2 = result
        .by_model
        .iter()
        .find(|m| m.model_id == "m2")
        .expect("m2");
    assert_eq!(m2.runs, 0);
    // total 合计
    assert_eq!(result.total.total, 194);
    assert_eq!(result.total.runs, 1);
    // 缺省范围（全部保留期）数字一致
    let default_range =
        stats_token_core(&app, &StatsTokenArgs { range: None }, "2026-10-30").expect("查询");
    assert_eq!(default_range.total.total, result.total.total);
}

#[test]
fn session_usage_three_level_fallback() {
    let (dir, app) = app_fixture();
    // ① 无任何记录：全零
    let none = session_usage_core(&app, "s-none");
    assert_eq!(none.usage, UsageTotal::default());
    assert_eq!(none.legacy_total_tokens, None);
    // ② 旧数据仅标量（index.json totalTokens > 0，无明细文件）→ 标量回落
    app.store
        .index()
        .upsert(crate::storage::SessionMeta {
            id: "s-legacy".to_owned(),
            project_id: None,
            title: "旧会话".to_owned(),
            created_at: 1,
            updated_at: 1,
            revision: 1,
            message_count: 3,
            total_tokens: 777,
            authorized_external_dirs: Vec::new(),
        })
        .expect("建索引");
    let legacy = session_usage_core(&app, "s-legacy");
    assert_eq!(
        legacy.usage,
        UsageTotal::default(),
        "旧记录不伪造四口径拆分"
    );
    assert_eq!(legacy.legacy_total_tokens, Some(777));
    // ③ 明细文件权威（Q37）：恢复 sessionTotal
    app.ledger
        .detail
        .record(&crate::storage::stats::RunUsageRecord {
            session_id: "s-legacy",
            run_id: "r1",
            kind: "main",
            model_id: "m1",
            model_name: "GPT-4o",
            delta: UsageTotal {
                input: 40,
                output: 4,
                cache_read: 2,
                cache_write: 0,
            },
        })
        .expect("落明细");
    let restored = session_usage_core(&app, "s-legacy");
    assert_eq!(restored.usage.input, 40);
    assert_eq!(restored.legacy_total_tokens, None);
    // ④ 内存表优先（活跃实时面）：start_chat 恢复面把明细灌入内存表后，
    //    帧增量在其上累加——模拟恢复 + 累计，查询返回恢复+增量
    {
        let restored = app.ledger.detail.load("s-legacy").session_total;
        app.session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert("s-legacy".to_owned(), restored);
    }
    app.accumulate_usage(
        "s-legacy",
        &UsageTotal {
            input: 1,
            output: 0,
            cache_read: 0,
            cache_write: 0,
        },
    );
    let live = session_usage_core(&app, "s-legacy");
    assert_eq!(live.usage.input, 41);
    // tempdir 保活到测试结束
    drop(dir);
}

#[test]
fn session_usage_after_delete_is_zero() {
    let (_dir, app) = app_fixture();
    app.ledger.record_on_day(
        "2026-10-30",
        "s-del",
        "r1",
        crate::host::usage::run_kind::MAIN,
        "m1",
        "GPT-4o",
        UsageTotal {
            input: 9,
            output: 1,
            cache_read: 0,
            cache_write: 0,
        },
        true,
    );
    // Q43：删除会话 → 内存表摘除 + 明细文件清除 → 查询全零
    app.session_usage
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove("s-del");
    app.ledger.detail.forget_session("s-del");
    let after = session_usage_core(&app, "s-del");
    assert_eq!(after.usage, UsageTotal::default(), "删除后明细不可查询");
    assert_eq!(after.legacy_total_tokens, None);
    // 匿名日聚合保留（真实消耗）
    let stats = stats_token_core(
        &app,
        &StatsTokenArgs {
            range: range("2026-10-01", "2026-10-31"),
        },
        "2026-10-30",
    )
    .expect("查询");
    assert_eq!(stats.total.input, 9, "日模型数字保留（无会话关联）");
}
