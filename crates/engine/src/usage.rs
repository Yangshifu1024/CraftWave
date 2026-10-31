//! 用量落账与统计查询编排：把「每次模型响应
//! 返回 usage」的贡献增量落到三个面——
//!
//! 1. **日/模型聚合**（`stats/<YYYY-MM-DD>.json`，匿名、90 天保留）；
//! 2. **run/会话四口径明细**（`sessions/<sid>.usage.json`，权威恢复面）；
//! 3. **stats_cache 写穿**（`cache/index.db`，查询缓存；失败仅记日志）。
//!
//! 落账入口只有一个（[`UsageLedger::record`]），主 run 帧面（EventfulSink）、
//! 子代理帧面（SubRunSink）与辅助调用（suggest/压缩）共用——调用方各自
//! 保证「增量为本次调用的贡献」（累计 usage 帧先差分），本层不做二次折算
//! （聚合与明细只计一次；重放/重连不经本入口，天然幂等）。
//!
//! 记账日：[`local_today`] 取**当时**本地日期——每次落账独立取值，
//! 跨午夜的 run 前后两拍自然分属两天；历史已落日期永不重算（切时区不改
//! 历史文件）。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crate::backend::frames::UsageTotal;
use crate::storage::index_db::StatsCacheRow;
use crate::storage::stats::{DailyStats, STATS_RETENTION_DAYS, StatsStore, UsageDetailStore};

/// 当前本地日期（`YYYY-MM-DD`；时钟取不到时兜底 Unix 纪元日 1970-01-01——
/// 仅异常时钟下的防御面，正常环境不可达）。
pub fn local_today() -> String {
    use chrono::Datelike;
    let now = chrono::Local::now();
    format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day())
}

/// run 明细的类别值：主 run / 子代理 run / 辅助调用。
pub mod run_kind {
    pub const MAIN: &str = "main";
    pub const SUB: &str = "sub";
    pub const AUX: &str = "aux";
}

/// 用量账本：三个存储面的聚合句柄（AppState 持有；share_handles 克隆件）。
#[derive(Clone)]
pub struct UsageLedger {
    /// 日/模型聚合（真相面）。
    pub stats: Arc<StatsStore>,
    /// run/会话四口径明细（权威恢复面）。
    pub detail: Arc<UsageDetailStore>,
    /// 派生索引库（stats_cache 查询缓存，降级态自动直读真相面）。
    pub cache: Arc<crate::storage::index_db::IndexDb>,
}

impl UsageLedger {
    /// 数据根 → 账本（index.db 启动校验/重建在 [`crate::storage::index_db::
    /// open`] 内完成）。
    pub fn new(data_root: &Path) -> Self {
        Self {
            stats: Arc::new(StatsStore::new(data_root)),
            detail: Arc::new(UsageDetailStore::new(data_root)),
            cache: Arc::new(crate::storage::index_db::open(data_root)),
        }
    }

    /// 落一笔用量贡献（单一入口）：记账日 = [`local_today`]。
    /// 参数与语义见 [`Self::record_on_day`]。
    #[allow(clippy::too_many_arguments)]
    pub fn record(
        &self,
        session_id: &str,
        run_id: &str,
        kind: &str,
        model_id: &str,
        model_name: &str,
        delta: UsageTotal,
        counts_run: bool,
    ) {
        self.record_on_day(
            &local_today(),
            session_id,
            run_id,
            kind,
            model_id,
            model_name,
            delta,
            counts_run,
        );
    }

    /// 落账的可注入日期版本（「当时本地日期」的生产取值在 [`Self::
    /// record`]；测试注入固定日期验证跨午夜/切时区不重算）。任何一面失败
    /// 仅记日志不 panic（统计面不阻断 run 主链路）；落账成功后顺带做 90 天
    /// 惰性清理（无后台任务）。参数面为两个落账请求的并集
    /// （聚合身份 + 会话/run 身份），扁平传入保持单一调用出口的直读性。
    #[allow(clippy::too_many_arguments)]
    pub fn record_on_day(
        &self,
        day: &str,
        session_id: &str,
        run_id: &str,
        kind: &str,
        model_id: &str,
        model_name: &str,
        delta: UsageTotal,
        counts_run: bool,
    ) {
        // ① 日/模型聚合（匿名面）
        let record = crate::storage::stats::UsageRecord {
            day,
            model_id,
            model_name,
            delta,
            counts_run,
        };
        if let Err(e) = self.stats.record(&record) {
            #[cfg(debug_assertions)]
            eprintln!("[craftwave] 日统计落账失败（session={session_id} day={day}）：{e}");
        }
        // ② run/会话明细（权威面）
        let detail_record = crate::storage::stats::RunUsageRecord {
            session_id,
            run_id,
            kind,
            model_id,
            model_name,
            delta,
        };
        if let Err(e) = self.detail.record(&detail_record) {
            #[cfg(debug_assertions)]
            eprintln!("[craftwave] 会话用量明细落账失败（session={session_id}）：{e}");
        }
        // ③ stats_cache 写穿（失败仅记日志；read_day 重新加锁读一次，
        //    保证写进缓存的 = 真相面最新值）
        if !self.cache.degraded()
            && let Some(entry) = self.stats.read_day(day).models.get(model_id)
        {
            self.cache.upsert_stats(&StatsCacheRow {
                day: day.to_owned(),
                model_id: model_id.to_owned(),
                usage: UsageTotal {
                    input: entry.input,
                    output: entry.output,
                    cache_read: entry.cache_read,
                    cache_write: entry.cache_write,
                },
                runs: entry.runs,
            });
        }
        // ④ 90 天保留的惰性清理（文件名排序删除过期日；每次落账顺带）
        let removed = self.stats.prune_expired(day, STATS_RETENTION_DAYS);
        if !removed.is_empty() {
            #[cfg(debug_assertions)]
            eprintln!("[craftwave] 已清理过期日统计：{removed:?}");
        }
    }

    /// 统计查询（数据面）：`from`/`to` 为闭区间本地日期（None =
    /// 全部保留期）。优先 stats_cache（缺日回填），库降级直读
    /// `stats/*.json`（真相面）；两种路径的数字同源一致。
    pub fn query_range(&self, from: &str, to: &str) -> Vec<DailyStats> {
        let truth = self.stats.read_range(from, to);
        if self.cache.degraded() {
            return truth; // 降级直读
        }
        // 完整性校验：缓存行覆盖的日期集合与真相面不一致（重建后首次查询/
        // 写穿失败缺日）→ 全量回填再读缓存
        let cached = self.cache.query_stats_range(from, to);
        let need_backfill = match &cached {
            Some(rows) => {
                let mut cached_days: Vec<&str> = rows.iter().map(|r| r.day.as_str()).collect();
                cached_days.sort_unstable();
                cached_days.dedup();
                let truth_days: Vec<&str> = truth.iter().map(|d| d.day.as_str()).collect();
                cached_days != truth_days
            }
            None => true,
        };
        if need_backfill {
            let rows = self.cache_rows_from_truth();
            self.cache.backfill_stats(&rows);
            if let Some(refilled) = self.cache.query_stats_range(from, to) {
                return refill_from_rows(refilled, &truth);
            }
            return truth;
        }
        match cached {
            Some(rows) => refill_from_rows(rows, &truth),
            None => truth,
        }
    }

    /// 从真相面（stats/*.json）读出全部 (day, model) 行（回填数据源）。
    fn cache_rows_from_truth(&self) -> Vec<StatsCacheRow> {
        let mut rows = Vec::new();
        for day in self.stats.list_days() {
            let daily = self.stats.read_day(&day);
            for (model_id, entry) in &daily.models {
                rows.push(StatsCacheRow {
                    day: day.clone(),
                    model_id: model_id.clone(),
                    usage: UsageTotal {
                        input: entry.input,
                        output: entry.output,
                        cache_read: entry.cache_read,
                        cache_write: entry.cache_write,
                    },
                    runs: entry.runs,
                });
            }
        }
        rows
    }
}

/// 用缓存行组装逐日聚合（modelName 从真相面按 (day, modelId) 补齐——
/// 缓存表不存显示名，真相面永远可读）。
fn refill_from_rows(rows: Vec<StatsCacheRow>, truth: &[DailyStats]) -> Vec<DailyStats> {
    let mut names: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    for daily in truth {
        for (model_id, entry) in &daily.models {
            names.insert((daily.day.as_str(), model_id.as_str()), &entry.model_name);
        }
    }
    let mut by_day: BTreeMap<String, DailyStats> = BTreeMap::new();
    for row in rows {
        let daily = by_day.entry(row.day.clone()).or_insert_with(|| DailyStats {
            version: crate::storage::stats::STATS_FORMAT_VERSION,
            day: row.day.clone(),
            models: BTreeMap::new(),
        });
        let entry = daily.models.entry(row.model_id.clone()).or_default();
        entry.model_name = names
            .get(&(row.day.as_str(), row.model_id.as_str()))
            .copied()
            .unwrap_or("")
            .to_owned();
        entry.input = row.usage.input;
        entry.output = row.usage.output;
        entry.cache_read = row.usage.cache_read;
        entry.cache_write = row.usage.cache_write;
        entry.runs = row.runs;
    }
    by_day.into_values().collect()
}

#[cfg(test)]
#[path = "usage_test.rs"]
mod usage_test;
