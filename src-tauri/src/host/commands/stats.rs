//! 用量统计域命令（§3.3 #52 `stats_token` / #59k `session_usage`，20 号票）。
//!
//! - `stats_token`：按日/按模型聚合查询（FR-02-13/FR-15-02 面板数据源）；
//!   优先 `stats_cache`（§7.8，缺日回填），库降级直读 `stats/*.json` 真相面。
//! - `session_usage`：会话累计四口径查询（Q37 恢复读面）——明细文件缺失时
//!   回退 index.json 的 totalTokens 标量，不伪造旧记录的四口径拆分。

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::backend::frames::UsageTotal;
use crate::host::AppState;
use crate::host::IpcError;
use crate::util::errors::E_INVALID_ARG;

/// `stats_token` 的范围参数（§3.3 #52；闭区间本地日期，缺省 = 全部保留期）。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StatsRange {
    pub from: String,
    pub to: String,
}

/// `stats_token` 参数契约（§3.3 #52）。
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StatsTokenArgs {
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub range: Option<StatsRange>,
}

/// byDay 元素（§3.3 #52 字段级）：单日四口径合计（total = 四口径之和）。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DayUsage {
    pub day: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

/// byModel 元素（§3.3 #52 字段级）：模型维度聚合——modelName 为调用时
/// 显示名快照；runs 仅主 run（含队列自动发起）计入（Q40）。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub model_id: String,
    pub model_name: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
    pub runs: u64,
}

/// total 元素（§3.3 #52 字段级）：范围合计（含 runs 合计）。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TotalUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
    pub runs: u64,
}

/// `stats_token` 返回契约（§3.3 #52）。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StatsTokenResult {
    pub by_day: Vec<DayUsage>,
    pub by_model: Vec<ModelUsage>,
    pub total: TotalUsage,
}

/// 日期参数校验（`YYYY-MM-DD` 语义合法；否则 E_INVALID_ARG）。
fn validate_day(day: &str) -> Result<(), IpcError> {
    if crate::storage::stats::is_valid_day(day)
        && chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").is_ok()
    {
        Ok(())
    } else {
        Err(IpcError::new(
            E_INVALID_ARG,
            format!("非法日期（须为 YYYY-MM-DD）：{day}"),
        ))
    }
}

/// `stats_token` 主体（公开给集成测试；`now` 由调用方传本地今日——测试可
/// 注入固定日期，生产传 [`crate::host::usage::local_today`]）。
pub fn stats_token_core(
    app: &AppState,
    args: &StatsTokenArgs,
    today: &str,
) -> Result<StatsTokenResult, IpcError> {
    let (from, to) = match &args.range {
        Some(range) => {
            validate_day(&range.from)?;
            validate_day(&range.to)?;
            if range.from > range.to {
                return Err(IpcError::new(
                    E_INVALID_ARG,
                    format!(
                        "range.from 不能晚于 range.to：{} > {}",
                        range.from, range.to
                    ),
                ));
            }
            (range.from.clone(), range.to.clone())
        }
        // 缺省 = 全部保留期（最早统计日 ~ 今天；空统计返回空集）
        None => (
            crate::storage::stats::shift_day(
                today,
                -(i64::from(crate::storage::stats::STATS_RETENTION_DAYS)),
            )
            .unwrap_or_else(|| "1970-01-01".to_owned()),
            today.to_owned(),
        ),
    };
    let dailies = app.ledger.query_range(&from, &to);
    let mut by_day: Vec<DayUsage> = Vec::with_capacity(dailies.len());
    let mut models: std::collections::BTreeMap<String, ModelUsage> =
        std::collections::BTreeMap::new();
    let mut total = TotalUsage::default();
    for daily in dailies {
        let mut day_usage = DayUsage {
            day: daily.day.clone(),
            ..DayUsage::default()
        };
        for (model_id, entry) in &daily.models {
            let bucket_total = entry.input + entry.output + entry.cache_read + entry.cache_write;
            day_usage.input += entry.input;
            day_usage.output += entry.output;
            day_usage.cache_read += entry.cache_read;
            day_usage.cache_write += entry.cache_write;
            day_usage.total += bucket_total;
            let bucket = models
                .entry(model_id.clone())
                .or_insert_with(|| ModelUsage {
                    model_id: model_id.clone(),
                    model_name: entry.model_name.clone(),
                    ..ModelUsage::default()
                });
            // 显示名快照取「该模型任一日最近可见名」（BTreeMap 升序遍历中
            // 后写覆盖前写 = 最新日快照；模型改名后历史日仍保各自快照面）
            bucket.model_name = entry.model_name.clone();
            bucket.input += entry.input;
            bucket.output += entry.output;
            bucket.cache_read += entry.cache_read;
            bucket.cache_write += entry.cache_write;
            bucket.total += bucket_total;
            bucket.runs += entry.runs;
            total.input += entry.input;
            total.output += entry.output;
            total.cache_read += entry.cache_read;
            total.cache_write += entry.cache_write;
            total.total += bucket_total;
            total.runs += entry.runs;
        }
        by_day.push(day_usage);
    }
    Ok(StatsTokenResult {
        by_day,
        by_model: models.into_values().collect(),
        total,
    })
}

/// 用量统计查询（§3.3 #52，FR-02-13）：按日/按模型聚合（四口径 + 缓存
/// 命中率的前置数字 + 主 run 次数），范围缺省 = 全部保留期。
#[tauri::command]
pub(crate) async fn stats_token(
    app: State<'_, AppState>,
    range: Option<StatsRange>,
) -> Result<StatsTokenResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = StatsTokenArgs { range };
    stats_token_core(&app, &args, &crate::host::usage::local_today())
}

/// `session_usage` 返回契约（§3.3 #59k）：usage = 会话累计四口径（主/子/
/// 辅助总和）；legacyTotalTokens = 旧版标量回落（明细文件缺失且 index.json
/// 有历史总量时带出，前端合计行回落显示——不伪造四口径拆分）。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionUsageResult {
    pub session_id: String,
    pub usage: UsageTotal,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub legacy_total_tokens: Option<u64>,
}

/// `session_usage` 参数契约（§3.3 #59k）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionUsageArgs {
    pub session_id: String,
}

/// `session_usage` 主体（公开给集成测试）：内存表（活跃会话实时面）>
/// 明细文件 sessionTotal（Q37 权威面）> index.json totalTokens 标量回落
/// > 全零。已删除会话查不到任何面 → 全零（Q43：明细不可再查）。
pub fn session_usage_core(app: &AppState, session_id: &str) -> SessionUsageResult {
    // ① 内存表优先（活跃会话：帧面增量已实时推进，与明细文件同一落账双写）
    let in_memory = *app
        .session_usage
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(session_id)
        .unwrap_or(&UsageTotal::default());
    if in_memory != UsageTotal::default() {
        return SessionUsageResult {
            session_id: session_id.to_owned(),
            usage: in_memory,
            legacy_total_tokens: None,
        };
    }
    // ② 明细文件（Q37 权威恢复面）
    let detail = app.ledger.detail.load(session_id);
    if detail.session_total != UsageTotal::default() {
        return SessionUsageResult {
            session_id: session_id.to_owned(),
            usage: detail.session_total,
            legacy_total_tokens: None,
        };
    }
    // ③ 旧数据标量回落（index.json totalTokens；不伪造四口径拆分）
    let legacy = app
        .store
        .index()
        .get(session_id)
        .map(|meta| meta.total_tokens)
        .unwrap_or(0);
    SessionUsageResult {
        session_id: session_id.to_owned(),
        usage: UsageTotal::default(),
        legacy_total_tokens: (legacy > 0).then_some(legacy),
    }
}

/// 会话累计用量查询（§3.3 #59k，Q37 恢复读面）：前端会话打开时拉取回填
/// runMetrics.sessionUsage（tokens:update 同款 replace 语义），四口径与
/// 缓存命中率跨重启恢复。
#[tauri::command]
pub(crate) async fn session_usage(
    app: State<'_, AppState>,
    session_id: String,
) -> Result<SessionUsageResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = SessionUsageArgs { session_id };
    Ok(session_usage_core(&app, &args.session_id))
}

#[cfg(test)]
#[path = "stats_test.rs"]
mod stats_test;
