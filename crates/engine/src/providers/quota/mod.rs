//! 订阅/余额适配器：Inspector 配额卡的数据面。
//!
//! 分层（与 CodeWave 同构，凭据链按 CraftWave v2.5 口径改写）：
//! - 本模块承载**数据模型 + 主机路由 + 行态分类 + 快照主流程**；
//! - 各厂商协议差异在 `providers/quota/` 下一家一文件（取数 `fetch` 与
//!   纯解析 `parse_*` 两半——解析收 `serde_json::Value` + 注入的 `now`，
//!   单测无网络无时钟）；
//! - 密钥源 = config.json `ProviderConfig.apiKey`（v2.5 口径：Key 明文随
//!   0600 配置落盘，无 keyring——CodeWave 的 CredentialStore 链在移植时
//!   去除，`invalid` 态随之失去稳定触发路径，保留为枚举位）；
//! - 请求走调用方传入的 `reqwest::Client`（生产 = 代理工厂客户端，
//!   providers/http.rs，与模型请求同一条代理语义）。
//!
//! 设计约束（逐条）：
//! - **按行态返回**（六态 `ok/error/invalid/rejected/no_key/unsupported`）：
//!   每一家配置过的供应商都有一行，未适配显示占位不报错；
//! - 单家失败不影响其余：需发请求的全并发（`join_all`），每家独立 10s 超时；
//! - **失败静默降级**：错误只进行态与 warn 日志（脱敏），不 emit 事件、
//!   不弹提示，永不阻塞聊天；
//! - 「曾经成功过」记在全局数据目录 `quota.json`（快照，原子写）：
//!   失败行带历史明细与 `lastOkAt`（离线兜底 = 显示最近快照与获取时间）。

pub mod deepseek;
pub mod glm;
pub mod kimi;
pub mod minimax;
pub mod opencode;
pub mod state;

#[cfg(test)]
#[path = "quota_test.rs"]
mod quota_test;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

/// 单家额度请求超时（每家独立：一家慢不拖垮整批）。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// `unsupported` 的 reason：baseUrl 主机名未命中任何适配器白名单。
pub const REASON_NO_ADAPTER: &str = "no_adapter";
/// `unsupported` 的 reason：baseUrl 为空/空白。
pub const REASON_EMPTY_BASE_URL: &str = "empty_base_url";

// ---- 数据模型（ quota.json 行， camelCase）----

/// 一行额度事实：百分比行（`usedPercent`/`remainingPercent`）或数值行
/// （`valueText`，余额类），二者必居其一。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct QuotaEntry {
    /// 窗口键（five_hour/weekly/balance_cny/usage/limit_N/mcp/rolling…）。
    pub key: String,
    /// 厂商自带标签（Kimi/Zhipu 的 limits 有 name 时使用）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub label: Option<String>,
    /// 已用百分比（一位小数，0–100）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub used_percent: Option<f64>,
    /// 剩余百分比（一位小数，0–100）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub remaining_percent: Option<f64>,
    /// 数值行文本（余额/计数，前端直接展示）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub value_text: Option<String>,
    /// 重置时间（RFC3339；数值行恒为 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub resets_at: Option<String>,
    /// 窗口级上游状态字面量（目前只有 OpenCode 带，如 `ok`/`rate-limited`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub status: Option<String>,
}

impl QuotaEntry {
    /// 百分比行：入参为「已用百分比」，剩余取补（都收敛到 0–100 一位小数）。
    pub(crate) fn percent(
        key: &str,
        label: Option<String>,
        used_percent: f64,
        resets_at: Option<String>,
        status: Option<String>,
    ) -> Self {
        let used = clamp_percent(used_percent);
        Self {
            key: key.to_string(),
            label,
            used_percent: Some(used),
            remaining_percent: Some(clamp_percent(100.0 - used)),
            value_text: None,
            resets_at,
            status,
        }
    }

    /// 数值行（余额/计数）：前端直接展示文本；无窗口状态。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn value(key: &str, value_text: String) -> Self {
        Self {
            key: key.to_string(),
            label: None,
            used_percent: None,
            remaining_percent: None,
            value_text: Some(value_text),
            resets_at: None,
            status: None,
        }
    }
}

/// 一位小数取整（展示与测试断言都以此为准，CodeWave round1 口径）。
pub(crate) fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// 百分比收敛到 0–100。
pub(crate) fn clamp_percent(value: f64) -> f64 {
    round1(value.clamp(0.0, 100.0))
}

/// 一行快照的载荷（`data` 字段）：展示名 + 明细 + 失败/占位原因。
/// 失败行也带本结构（entries 为历史成功明细、error 有值），前端统一取数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct QuotaData {
    /// 展示名：`provider.name` 优先，空则回落 baseUrl 主机名，再无则回落 id。
    pub display_name: String,
    /// 额度明细（仅 ok 行非空；失败行空——离线兜底面见 `QuotaSnapshot`）。
    pub entries: Vec<QuotaEntry>,
    /// 仅 unsupported：`no_adapter` | `empty_base_url`。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub reason: Option<String>,
    /// error / rejected 的人读原因（已脱敏，可直接展示）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<String>,
    /// 密钥来源（当前只有 `config`——v2.5 口径 Key 存 config.json）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub key_source: Option<String>,
}

/// 供应商行态（六态；wire 名 snake_case，前端按此渲染）：
/// - `Ok` 取数成功；
/// - `Error` 其它失败（超时/连接失败/5xx/429/解析失败，或曾成功过的 401/403/404）；
/// - `Invalid` 密钥形态不可用（CodeWave 定义为 keyring 回读失败；CraftWave
///   无 keyring——**保留枚举位**，当前无稳定触发路径）；
/// - `Rejected` 401/403/404 且该家从未成功过（密钥/套餐被拒，重试无用）；
/// - `NoKey` 命中适配器但 apiKey 缺失；
/// - `Unsupported` 无法查询（主机未命中白名单 / baseUrl 为空）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum QuotaStatus {
    Ok,
    Error,
    Invalid,
    Rejected,
    NoKey,
    Unsupported,
}

impl QuotaStatus {
    /// 是否属于「可查询类」（排序时排在 `unsupported` 之前；CodeWave 口径）。
    pub fn is_queryable(self) -> bool {
        !matches!(self, Self::Unsupported)
    }
}

/// 一家供应商的额度快照行（元素 / `quota.json` 行，同构）。
///
/// `lastOkAt`（回写增补字段）：上次成功取数时刻——区分「刚才还好
/// 好的」与「从来没成功过」；本轮成功即本轮时刻，失败行带历史值，从未成功
/// 恒 None。失败行**保留历史 data**（最近一次成功的明细）：断网/失效后
/// 前端仍能显示最近快照，以 `lastOkAt` 标注数据时点（离线兜底）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase", default)]
pub struct QuotaSnapshot {
    /// 供应商 id（`config.providers[].id`）。
    pub provider_id: String,
    /// 行态（六态）。
    pub state: QuotaStatus,
    /// 行载荷（展示名/明细/原因；失败行的 entries 为空，但保留离线兜底面）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub data: Option<QuotaData>,
    /// 本行写入时刻（RFC3339 秒）——实时面 = 本次拉取时刻；离线兜底面 =
    /// 最近一次查询时刻。
    pub fetched_at: String,
    /// 上次成功取数时刻（RFC3339 秒；从未成功恒 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub last_ok_at: Option<String>,
}

// QuotaSnapshot 的 serde 缺省（旧文件缺字段透明回填，绝不因解析失败 panic）。
impl Default for QuotaSnapshot {
    fn default() -> Self {
        Self {
            provider_id: String::new(),
            state: QuotaStatus::Unsupported,
            data: None,
            fetched_at: String::new(),
            last_ok_at: None,
        }
    }
}

// ---- 适配器注册表与主机路由（CodeWave 口径）----

/// 本期支持的供应商适配器（注册序即主机匹配序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterKind {
    OpenCode,
    DeepSeek,
    MiniMaxIntl,
    MiniMaxCn,
    Kimi,
    Zhipu,
    Zai,
}

/// 适配器白名单（只有这些域的 baseUrl 才能查询额度）。
pub const ALL: &[AdapterKind] = &[
    AdapterKind::OpenCode,
    AdapterKind::DeepSeek,
    AdapterKind::MiniMaxIntl,
    AdapterKind::MiniMaxCn,
    AdapterKind::Kimi,
    AdapterKind::Zhipu,
    AdapterKind::Zai,
];

impl AdapterKind {
    /// 该供应商支持的 baseUrl 域名白名单（子域后缀也算命中）。
    pub fn base_url_hosts(self) -> &'static [&'static str] {
        match self {
            Self::OpenCode => &["opencode.ai"],
            Self::DeepSeek => &["api.deepseek.com", "deepseek.com"],
            Self::MiniMaxIntl => &["api.minimax.io", "minimax.io"],
            Self::MiniMaxCn => &["api.minimax.cn", "minimax.cn", "www.minimax.cn"],
            Self::Kimi => &["api.kimi.com", "kimi.com", "api.moonshot.cn", "moonshot.cn"],
            Self::Zhipu => &["bigmodel.cn"],
            Self::Zai => &["api.z.ai", "z.ai"],
        }
    }
}

/// 从 baseUrl 取小写主机名（去协议、去端口、去路径、去 @ 前的 userinfo）。
pub(crate) fn host_of(base_url: &str) -> Option<String> {
    let rest = base_url.split("://").nth(1).unwrap_or(base_url);
    let host = rest.split(['/', '?', '#']).next()?.split('@').next_back()?;
    let host = host.split(':').next()?.trim().to_lowercase();
    (!host.is_empty()).then_some(host)
}

/// 主机命中判定：相等或 `.{候选}` 子域后缀命中（`evildeepseek.com` 这类
/// 前缀伪冒与无后缀关系的域都不命中）。
fn host_matches(host: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|c| host == *c || host.ends_with(&format!(".{c}")))
}

/// baseUrl 命中的适配器（主机白名单判定；不用模型 id 前缀——OpenCode 等
/// 兼容端点上也能跑他家模型，前缀匹配会误判，CodeWave 同口径）。
pub(crate) fn kind_for_base_url(base_url: &str) -> Option<AdapterKind> {
    let host = host_of(base_url)?;
    ALL.iter()
        .copied()
        .find(|k| host_matches(&host, k.base_url_hosts()))
}

/// 展示名：`provider.name` 优先，空则回落 baseUrl 主机名，再无则回落 id。
pub(crate) fn display_name(name: &str, base_url: &str, id: &str) -> String {
    let trimmed = name.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    host_of(base_url).unwrap_or_else(|| id.to_string())
}

// ---- 失败模型与统一取数 ----

/// 一次取数失败：`status` 是有 HTTP 响应时的状态码，无响应（超时/连接失败/
/// 解析失败）为 None。`message` 已完成脱敏，可直接进行态与日志。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FetchFailure {
    pub(crate) status: Option<u16>,
    pub(crate) message: String,
}

impl FetchFailure {
    /// 无 HTTP 状态的失败（超时/连接失败/解析失败）。
    pub(crate) fn message(message: impl Into<String>) -> Self {
        Self {
            status: None,
            message: message.into(),
        }
    }
}

/// Authorization 头风格：多数厂商 `Bearer <key>`，智谱/Z.ai 用裸 key
/// （与上游一致，不加前缀）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthStyle {
    Bearer,
    Raw,
}

/// 数值取值（数字或数字字符串都接受）——适配器解析共用。
pub(crate) fn number(value: &serde_json::Value, key: &str) -> Option<f64> {
    let raw = value.get(key)?;
    raw.as_f64()
        .or_else(|| raw.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

/// 非空字符串取值（trim 后为空视同缺失）——适配器解析共用。
pub(crate) fn text(value: &serde_json::Value, key: &str) -> Option<String> {
    let raw = value.get(key)?.as_str()?.trim();
    (!raw.is_empty()).then(|| raw.to_string())
}

/// 抹除文本里的密钥（长度 >= 8 才替换，避免把普通词替换掉），并压缩空白 +
/// 截断 160 字符——错误消息绝不回显 Key（日志脱敏纪律）。
pub(crate) fn sanitize(text: &str, secret: &str) -> String {
    let redacted = if secret.len() >= 8 {
        text.replace(secret, "[redacted]")
    } else {
        text.to_string()
    };
    let squeezed = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    squeezed.chars().take(160).collect()
}

/// 统一的 GET + JSON 响应处理：非 2xx 与解析失败都返回脱敏后的用户文案。
/// 有 HTTP 响应时带上状态码（供上层判 401/403/404 → rejected）；
/// 无响应/读体失败时 status 为 None。
pub(crate) async fn fetch_json(
    client: &reqwest::Client,
    url: &str,
    key: &str,
    style: AuthStyle,
    label: &str,
) -> Result<Value, FetchFailure> {
    let auth = match style {
        AuthStyle::Bearer => format!("Bearer {key}"),
        AuthStyle::Raw => key.to_string(),
    };
    let response = client
        .get(url)
        .header(reqwest::header::AUTHORIZATION, auth)
        .header(reqwest::header::ACCEPT, "application/json")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            FetchFailure::message(format!("{label}：{}", sanitize(&e.to_string(), key)))
        })?;

    let status = response.status();
    let body = response.text().await.map_err(|e| {
        FetchFailure::message(format!("{label}：{}", sanitize(&e.to_string(), key)))
    })?;
    if !status.is_success() {
        return Err(FetchFailure {
            status: Some(status.as_u16()),
            message: format!("{label} {status}：{}", sanitize(body.trim(), key)),
        });
    }
    serde_json::from_str::<Value>(&body)
        .map_err(|_| FetchFailure::message(format!("{label}：响应不是合法 JSON")))
}

/// 按适配器分发取数（密钥已解析完成；协议差异不出 providers/ 各文件）。
pub(crate) async fn fetch(
    kind: AdapterKind,
    key: &str,
    client: &reqwest::Client,
) -> Result<Vec<QuotaEntry>, FetchFailure> {
    match kind {
        AdapterKind::OpenCode => opencode::fetch(key, client).await,
        AdapterKind::DeepSeek => deepseek::fetch(key, client).await,
        AdapterKind::MiniMaxIntl => {
            minimax::fetch(minimax::Endpoint::International, key, client).await
        }
        AdapterKind::MiniMaxCn => minimax::fetch(minimax::Endpoint::China, key, client).await,
        AdapterKind::Kimi => kimi::fetch(key, client).await,
        AdapterKind::Zhipu => glm::fetch(glm::Flavor::Zhipu, key, client).await,
        AdapterKind::Zai => glm::fetch(glm::Flavor::Zai, key, client).await,
    }
}

// ---- 快照主流程 ----

/// 取数 Future（统一装箱：生产走 [`fetch`]，单测注入假实现）。
pub(crate) type FetchBox =
    Pin<Box<dyn Future<Output = Result<Vec<QuotaEntry>, FetchFailure>> + Send>>;

/// 取数器：按「适配器种类 + 密钥」返回取数 Future（测试注入面）。
pub(crate) type Fetcher<'a> = &'a (dyn Fn(AdapterKind, String) -> FetchBox + Send + Sync);

/// 待发请求的一条（配置下标 + 适配器 + 密钥）。
struct Pending {
    index: usize,
    kind: AdapterKind,
    key: String,
}

/// 全部配置过的供应商的额度快照（生产入口， 数据面）：
/// 逐家按 baseUrl 主机路由适配器，需发请求的全并发实时拉取，一家失败不
/// 影响其余；批次末把「有请求结果的行」合并进 `quota.json`（离线兜底面）。
///
/// 结果顺序 = 排序规则（配置序、可查询类在前、unsupported 殿后）；前端
/// 「当前会话所用 provider 置顶」的展示编排由前端完成（契约无参数）。
pub async fn snapshots(
    config: &crate::providers::types::AppConfig,
    data_dir: &Path,
    client: &reqwest::Client,
) -> Vec<QuotaSnapshot> {
    // client 移进闭包（reqwest::Client 内部 Arc，clone 廉价），使返回的
    // Future 不借用局部变量，可与其它请求统一 join_all。
    let owned = client.clone();
    let fetcher = move |kind: AdapterKind, key: String| -> FetchBox {
        let client = owned.clone();
        Box::pin(async move { fetch(kind, &key, &client).await })
    };
    snapshots_with(config, data_dir, &fetcher).await
}

/// 快照主流程的可注入形态（单测传假取数器与临时目录，无网络无时钟依赖——
/// `now` 由 [`now_rfc3339`] 取真实时钟，断言不含时间戳内容）。
pub(crate) async fn snapshots_with(
    config: &crate::providers::types::AppConfig,
    data_dir: &Path,
    fetcher: Fetcher<'_>,
) -> Vec<QuotaSnapshot> {
    let fetched_at = now_rfc3339();
    let ids: Vec<String> = config.providers.iter().map(|p| p.id.clone()).collect();
    // 取数前读一份快照：只供本批行展示（失败行的历史 data / lastOkAt）与
    // 「曾成功过」判据。真正的「读—改—写」在批次末的临界区里重做
    //（`state::commit`）——那个才落盘，且并发批次互不覆盖。
    let history = state::load(data_dir);

    let mut rows: Vec<Option<QuotaSnapshot>> = vec![None; config.providers.len()];
    let mut pending: Vec<Pending> = Vec::new();

    for (index, provider) in config.providers.iter().enumerate() {
        let base_url = provider.base_url.as_deref().unwrap_or("");
        let display = display_name(&provider.name, base_url, &provider.id);
        // ① baseUrl 为空 / 未命中白名单 → unsupported（不发请求，不报错）
        if base_url.trim().is_empty() {
            rows[index] = Some(plain_row(
                provider.id.clone(),
                display,
                QuotaStatus::Unsupported,
                Some(REASON_EMPTY_BASE_URL.to_string()),
                None,
                &history,
                &fetched_at,
            ));
            continue;
        }
        let Some(kind) = kind_for_base_url(base_url) else {
            rows[index] = Some(plain_row(
                provider.id.clone(),
                display,
                QuotaStatus::Unsupported,
                Some(REASON_NO_ADAPTER.to_string()),
                None,
                &history,
                &fetched_at,
            ));
            continue;
        };
        // ② 密钥解析（v2.5 口径 = config.json 明文；空白归 None 已在
        //    normalize 完成，这里再防御一次 trim）：缺失 → no_key
        let Some(key) = provider
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
        else {
            rows[index] = Some(plain_row(
                provider.id.clone(),
                display,
                QuotaStatus::NoKey,
                None,
                None,
                &history,
                &fetched_at,
            ));
            continue;
        };
        // ③ 只有一枚 Key（Key 池 M1 单 Key 直用），入并发队列
        pending.push(Pending {
            index,
            kind,
            key: key.to_string(),
        });
    }

    // ④ 需发请求的全并发（每家独立超时在 fetch_json 内；一家失败不影响其余）
    let results = futures::future::join_all(
        pending
            .iter()
            .map(|item| (fetcher)(item.kind, item.key.clone())),
    )
    .await;

    // 本批「有请求结果的行」（ok/error/rejected）——批次末合并进 quota.json
    let mut result_rows: Vec<QuotaSnapshot> = Vec::new();
    for (item, result) in pending.iter().zip(results) {
        let provider = &config.providers[item.index];
        let base_url = provider.base_url.as_deref().unwrap_or("");
        let display = display_name(&provider.name, base_url, &provider.id);
        let row = match result {
            Ok(entries) => QuotaSnapshot {
                provider_id: provider.id.clone(),
                state: QuotaStatus::Ok,
                data: Some(QuotaData {
                    display_name: display,
                    entries,
                    reason: None,
                    error: None,
                    key_source: Some("config".to_string()),
                }),
                fetched_at: fetched_at.clone(),
                // 本轮成功：带本次时刻（commit 刚写入同值）
                last_ok_at: Some(fetched_at.clone()),
            },
            Err(failure) => {
                // 失败静默：日志一行（消息已脱敏，不含 Key），不 emit 事件
                // 不弹提示（「静默降级，不阻塞任何功能」）
                eprintln!(
                    "[craftwave] 配额查询失败（provider={}）：{}",
                    provider.id, failure.message
                );
                let status =
                    classify_failure(&failure, state::has_succeeded(&history, &provider.id));
                plain_row(
                    provider.id.clone(),
                    display,
                    status,
                    None,
                    Some(failure.message),
                    &history,
                    &fetched_at,
                )
            }
        };
        rows[item.index] = Some(row.clone());
        result_rows.push(row);
    }

    // ⑤ 组装返回面：结果行 + 占位行（no_key/unsupported）都已按配置位归在
    //    rows（两分支互斥），flatten 即配置序
    let all: Vec<QuotaSnapshot> = rows.into_iter().flatten().collect();

    // ⑥ 批次末的原子提交：临界区内「重新读盘 → 合并本批结果 → 清孤儿 →
    //    写盘一次」。取数（await）与落盘（同步文件 IO）彻底分离：持锁期不
    //    跨 await；重新读盘保证并发批次互相不覆盖记录。
    state::commit(data_dir, &ids, &result_rows, &fetched_at);

    order(all)
}

/// 排序（CodeWave 口径）：稳定排序保证同组内保持配置顺序——
/// 可查询类在前，`unsupported` 殿后。
fn order(mut rows: Vec<QuotaSnapshot>) -> Vec<QuotaSnapshot> {
    rows.sort_by_key(|row| u8::from(!row.state.is_queryable()));
    rows
}

/// 失败归类（CodeWave 口径）：401/403/404 且**从未成功过** → rejected
/// （密钥/套餐被拒，重试无用）；其余（超时/连接失败/5xx/429/解析失败/
/// 曾成功过的 401/403/404）→ error。
fn classify_failure(failure: &FetchFailure, succeeded_before: bool) -> QuotaStatus {
    let rejected_status = matches!(failure.status, Some(401 | 403 | 404));
    if rejected_status && !succeeded_before {
        QuotaStatus::Rejected
    } else {
        QuotaStatus::Error
    }
}

/// 无请求结果的占位行（unsupported / no_key）与失败行（error / rejected）
/// 共用的组装面。
///
/// - **失败行**（`error` 有值）：`entries` 带历史成功明细（最近一次成功写入
///   `quota.json` 的数据——离线兜底面，断网/失效后仍能看到最近快照），
///   `lastOkAt` 标注其时点；`key_source` 如实带出（失败也发生在带着这枚
///   Key 查询的路径上）。
/// - **占位行**：entries 为空，`reason`/占位语义由行态表达；历史成功记录
///   照实带 `lastOkAt`（行态无关的统一规则，不因 unsupported 特判抹掉）。
#[allow(clippy::too_many_arguments)]
fn plain_row(
    provider_id: String,
    display: String,
    status: QuotaStatus,
    reason: Option<String>,
    error: Option<String>,
    history: &[QuotaSnapshot],
    fetched_at: &str,
) -> QuotaSnapshot {
    let historical = history.iter().find(|row| row.provider_id == provider_id);
    // 失败行继承最近一次成功的明细（quota.json 行的 data 恒为「最近一次
    // 成功明细」——state.rs 的合并规则保证：失败行落盘时也继承旧 data）
    let entries = if error.is_some() {
        historical
            .and_then(|row| row.data.as_ref())
            .map(|data| data.entries.clone())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let last_ok_at = historical
        .and_then(|row| row.last_ok_at.clone())
        .filter(|at| !at.is_empty());
    let has_error = error.is_some();
    QuotaSnapshot {
        provider_id,
        state: status,
        data: Some(QuotaData {
            display_name: display,
            entries,
            reason,
            error,
            key_source: has_error.then(|| "config".to_string()),
        }),
        fetched_at: fetched_at.to_string(),
        last_ok_at,
    }
}

/// 当前时刻（RFC3339 秒，UTC）。
fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}
