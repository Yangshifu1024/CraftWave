//! 设置域 API：配置读写、供应商管理与连通测试、UI 状态、用量统计与配额。
//!
//! 平移来源：`host/commands/config.rs`、`provider.rs`、`ui_state.rs`、
//! `stats.rs`、`quota.rs`（全部框架无关 core；业务核心在
//! `providers::service` / `providers::probe` / `usage` 账本 / `storage::stats`）。

use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::backend::frames::UsageTotal;
use crate::providers::probe;
use crate::providers::quota::{self, QuotaSnapshot};
use crate::providers::service;
use crate::providers::types::{AppConfig, ProviderConfig};
use crate::util::errors::{E_INVALID_ARG, E_NOT_FOUND, IpcError};

// ---- 配置（旧命令 get_config / save_config） ----

/// `save_config` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SaveConfigArgs {
    pub config: AppConfig,
}

/// 读取当前配置（协调器缓存优先）。
pub fn get_config(app: &AppState) -> Result<AppConfig, IpcError> {
    service::get_config(&app.config)
}

/// 整份配置经协调器归一化原子落盘，返回归一化结果。保存边界热生效两件：
/// 子代理全局并发上限（`apply_subagent_limit`）与代理客户端重建
/// （`apply_proxy_config`——进行中 run 不切换，下一次 start_chat 用新代理）。
/// （旧壳还同步重建托盘菜单语言——托盘属阶段 4 壳集成，此处无对应物。）
pub fn save_config(app: &AppState, args: SaveConfigArgs) -> Result<AppConfig, IpcError> {
    let normalized = service::save_config(&app.config, args.config)?;
    // 子代理全局并发上限热应用（已启动任务继续不取消，降低暂停补入，
    // 提高立即放行排队任务）
    app.apply_subagent_limit();
    // 代理热生效：重建 HTTP 客户端工厂缓存
    app.apply_proxy_config()?;
    Ok(normalized)
}

// ---- 供应商（旧命令 save/delete/test_provider） ----

/// `save_provider` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SaveProviderArgs {
    pub provider: ProviderConfig,
    #[cfg_attr(feature = "ts", ts(optional))]
    pub new_key: Option<String>,
}

/// `delete_provider` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DeleteProviderArgs {
    pub provider_id: String,
}

/// `test_provider` 参数契约：目标供应商 + 可选地址覆写
/// （草稿试连语义——表单里改了还没保存的 baseUrl 直接探测，不落库）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TestProviderArgs {
    pub provider_id: String,
    /// 草稿基地址覆写（缺省用已保存的 baseUrl / 官方缺省地址）。
    #[cfg_attr(feature = "ts", ts(optional))]
    pub base_url_override: Option<String>,
}

/// `test_provider` 返回契约：`{ ok, latencyMs?, models?, error? }`
/// （error = `{ code, message }` 脱敏，code 为供应商七类之一）。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TestProviderResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub models: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<probe::ProbeError>,
}

/// 保存供应商；newKey 有值时写入 apiKey（明文随 config.json 0600
/// 落盘）。返回归一化后的供应商。
pub fn save_provider(app: &AppState, args: SaveProviderArgs) -> Result<ProviderConfig, IpcError> {
    service::save_provider(&app.config, args.provider, args.new_key)
}

/// 删除供应商（Key 随配置一并消失）。
pub fn delete_provider(app: &AppState, args: DeleteProviderArgs) -> Result<(), IpcError> {
    service::delete_provider(&app.config, &args.provider_id)
}

/// 保存供应商前的连通性测试。探测请求走代理工厂
/// 当前客户端（与模型请求同一条代理语义）；按方言两段探测（接通
/// 三协议：openai_chat 与 openai_responses 共用 /models + Bearer，回退段
/// 各自打 chat/completions 或 responses；anthropic_messages 用 x-api-key +
/// anthropic-version 鉴权）。探测结果不落库。
pub async fn test_provider(app: &AppState, args: TestProviderArgs) -> Result<TestProviderResult, IpcError> {
    let config = app.config.load()?.as_ref().clone();
    let provider = config
        .providers
        .iter()
        .find(|p| p.id == args.provider_id)
        .ok_or_else(|| IpcError::new(E_NOT_FOUND, format!("供应商不存在：{}", args.provider_id)))?;
    // 地址优先级：草稿覆写 > 已存 baseUrl > 官方缺省
    let base_url = args
        .base_url_override
        .or_else(|| provider.base_url.clone())
        .unwrap_or_else(|| provider.api_format.official_base_url().to_owned());
    // Key：已存的 Key 参与探测（本地供应商按 local 标记免 Key——不携带）；
    // 草稿里改了但没保存的 Key 无法参与（契约面只有 baseUrlOverride，
    // 口径：草稿测试只覆写地址，Key 仍按已存值）。
    let api_key = if provider.local { None } else { provider.api_key.as_deref() };
    let client = app.http.client();
    let result = probe::probe(&client, provider.api_format, &base_url, api_key).await;
    Ok(TestProviderResult {
        ok: result.ok,
        latency_ms: result.latency_ms,
        models: result.models,
        error: result.error,
    })
}

// ---- UI 状态（旧命令 get/set_ui_state） ----

/// `get_ui_state` 参数契约（`{}`）。
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct GetUiStateArgs {}

/// `set_ui_state` 参数契约（`{ state: unknown }`）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SetUiStateArgs {
    pub state: serde_json::Value,
}

/// 读取 UI 状态（Json 原样；从未保存过 → null）。
/// ui-state.json 承载 Tab 布局等 UI 自管状态（载荷 Json 原样存取，
/// 结构由 UI 侧定义；写侧防抖由 UI 侧负责）。
pub fn get_ui_state(app: &AppState, _args: GetUiStateArgs) -> Result<serde_json::Value, IpcError> {
    Ok(app.ui_state.load()?.as_ref().clone())
}

/// 保存 UI 状态（整份替换）。顺带提取 `activeId` 更新活跃会话标识
/// （系统通知防打扰精判共享面）。
pub fn set_ui_state(app: &AppState, args: SetUiStateArgs) -> Result<(), IpcError> {
    let active = args.state.get("activeId").and_then(|v| v.as_str());
    *app.active_session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = active.map(str::to_owned);
    app.ui_state.save(args.state).map(|_| ())
}

// ---- 用量统计（旧命令 stats_token / session_usage） ----

/// `stats_token` 的范围参数（闭区间本地日期，缺省 = 全部保留期）。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StatsRange {
    pub from: String,
    pub to: String,
}

/// `stats_token` 参数契约。
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StatsTokenArgs {
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub range: Option<StatsRange>,
}

/// byDay 元素（字段级）：单日四口径合计（total = 四口径之和）。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DayUsage {
    pub day: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
}

/// byModel 元素（字段级）：模型维度聚合——modelName 为调用时
/// 显示名快照；runs 仅主 run（含队列自动发起）计入。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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

/// total 元素（字段级）：范围合计（含 runs 合计）。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TotalUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total: u64,
    pub runs: u64,
}

/// `stats_token` 返回契约。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
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

/// 用量统计查询：按日/按模型聚合（四口径 + 主 run 次数），范围缺省 =
/// 全部保留期。`today` 由调用方传本地今日（测试可注入固定日期，生产传
/// `crate::usage::local_today`）。
pub fn stats_token(
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
                    format!("range.from 不能晚于 range.to：{} > {}", range.from, range.to),
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

/// `session_usage` 返回契约：usage = 会话累计四口径（主/子/
/// 辅助总和）；legacyTotalTokens = 旧版标量回落（明细文件缺失且 index.json
/// 有历史总量时带出，合计行回落显示——不伪造四口径拆分）。
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionUsageResult {
    pub session_id: String,
    pub usage: UsageTotal,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub legacy_total_tokens: Option<u64>,
}

/// `session_usage` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SessionUsageArgs {
    pub session_id: String,
}

/// 会话累计用量查询（恢复读面）：内存表（活跃会话实时面）>
/// 明细文件 sessionTotal（权威面）> index.json totalTokens 标量回落
/// > 全零。已删除会话查不到任何面 → 全零（明细不可再查）。
pub fn session_usage(app: &AppState, args: SessionUsageArgs) -> Result<SessionUsageResult, IpcError> {
    // ① 内存表优先（活跃会话：帧面增量已实时推进，与明细文件同一落账双写）
    let in_memory = *app
        .session_usage
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&args.session_id)
        .unwrap_or(&UsageTotal::default());
    if in_memory != UsageTotal::default() {
        return Ok(SessionUsageResult {
            session_id: args.session_id,
            usage: in_memory,
            legacy_total_tokens: None,
        });
    }
    // ② 明细文件（权威恢复面）
    let detail = app.ledger.detail.load(&args.session_id);
    if detail.session_total != UsageTotal::default() {
        return Ok(SessionUsageResult {
            session_id: args.session_id,
            usage: detail.session_total,
            legacy_total_tokens: None,
        });
    }
    // ③ 旧数据标量回落（index.json totalTokens；不伪造四口径拆分）
    let legacy = app
        .store
        .index()
        .get(&args.session_id)
        .map(|meta| meta.total_tokens)
        .unwrap_or(0);
    Ok(SessionUsageResult {
        session_id: args.session_id,
        usage: UsageTotal::default(),
        legacy_total_tokens: (legacy > 0).then_some(legacy),
    })
}

// ---- 配额（旧命令 quota_snapshots） ----

/// 全部供应商的额度快照（六态行，排序 = 配置序、可查询类在前
/// unsupported 殿后）。调用时并发实时拉取（每家独立 10s 超时，一家失败不
/// 影响其余）；批次末合并进 `quota.json` 快照（离线兜底面）。周期刷新由
/// UI 配额页可见门控驱动，不建后端定时器。失败静默降级的纪律在领域面
/// 落实，本层不做二次错误包装——整函数只在配置读取失败时返回 Err。
pub async fn quota_snapshots(app: &AppState) -> Result<Vec<QuotaSnapshot>, IpcError> {
    let config = app.config.load()?.as_ref().clone();
    let client = app.http.client();
    Ok(quota::snapshots(&config, app.store.root(), &client).await)
}
