//! 配额域单测：全本地、不发真实网络请求。
//!
//! 覆盖面（同型用例在单个 #[test] 内顺序/表驱动合并）：
//! - host 匹配表驱动（白名单/伪冒域）与 host_of 剥离、展示名回退；
//! - round1/clamp/sanitize 工具与六态失败归类判据；
//! - 每家适配器 `parse_*` 纯解析（正常 + 代表性错误态）；
//! - `fetch_json` 的 HTTP 面（axum mock server：200 / 401 / 解析失败 / 不可达）；
//! - 快照主流程（六态、排序、不发包纪律、失败隔离）；
//! - `quota.json` 原子写与行合并不变式（成功覆盖 / 失败继承 / 孤儿清理 /
//!   损坏隔离 / serde roundtrip）。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Json;
use axum::routing::get;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};

use super::deepseek;
use super::glm::{self, Flavor};
use super::kimi;
use super::minimax::{self, Endpoint};
use super::opencode;
use super::state::{self, FILE_NAME};
use super::{
    AdapterKind, FetchBox, FetchFailure, QuotaData, QuotaEntry, QuotaSnapshot, QuotaStatus,
    REASON_EMPTY_BASE_URL, REASON_NO_ADAPTER, clamp_percent, classify_failure, display_name,
    fetch_json, host_of, kind_for_base_url, round1, sanitize, snapshots_with,
};
use crate::providers::quota::AuthStyle;
use crate::providers::types::{ApiFormat, AppConfig, ProviderConfig, ProviderModel};

// ---------- 脚手架 ----------

/// 配置里的一个供应商（CraftWave v2.5 形态：单 apiKey 明文）。
fn provider(id: &str, name: &str, base_url: &str, api_key: Option<&str>) -> ProviderConfig {
    ProviderConfig {
        id: id.into(),
        name: name.into(),
        api_format: ApiFormat::OpenaiChat,
        base_url: (!base_url.is_empty()).then(|| base_url.to_string()),
        api_key: api_key.map(str::to_string),
        models: vec![ProviderModel {
            id: format!("{id}-model"),
            model: "mock-model".into(),
            max_tokens: 0,
            context_window: 0,
            reasoning_effort: None,
            vision: None,
        }],
        headers: None,
        local: false,
    }
}

fn config(providers: Vec<ProviderConfig>) -> AppConfig {
    AppConfig {
        providers,
        ..AppConfig::default()
    }
}

/// 固定「当前时刻」（重置窗口推算的注入点——单测无时钟）。
fn fixed_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 30, 8, 0, 0).unwrap()
}

/// 一个可展示的百分比行（重置时间固定，便于断言）。
fn percent_entry(key: &str, used: f64) -> QuotaEntry {
    QuotaEntry::percent(
        key,
        None,
        used,
        Some("2026-11-01T00:00:00Z".to_string()),
        None,
    )
}

/// 结构化取数失败（`status = None` 表示超时/连接失败一类无响应失败）。
fn failure(status: Option<u16>) -> FetchFailure {
    FetchFailure {
        status,
        message: format!("测试取数失败（status={status:?}）"),
    }
}

/// 假取数器：按「首密钥」回放预设结果，同时记录调用次数与实际收到的 key。
struct FakeFetcher {
    replies: HashMap<String, Result<Vec<QuotaEntry>, FetchFailure>>,
    calls: AtomicUsize,
    keys: Mutex<Vec<String>>,
}

impl FakeFetcher {
    fn new(replies: Vec<(&str, Result<Vec<QuotaEntry>, FetchFailure>)>) -> Self {
        Self {
            replies: replies
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
            calls: AtomicUsize::new(0),
            keys: Mutex::new(Vec::new()),
        }
    }

    fn fetcher(&self) -> impl Fn(AdapterKind, String) -> FetchBox + '_ {
        move |_kind, key: String| {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.keys.lock().unwrap().push(key.clone());
            let reply = match self.replies.get(&key) {
                Some(reply) => reply.clone(),
                None => Err(failure(None)),
            };
            Box::pin(async move { reply })
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

fn ids(rows: &[QuotaSnapshot]) -> Vec<String> {
    rows.iter().map(|row| row.provider_id.clone()).collect()
}

fn states(rows: &[QuotaSnapshot]) -> Vec<QuotaStatus> {
    rows.iter().map(|row| row.state).collect()
}

// ---------- 工具与判据（host 匹配表驱动收拢）----------

// 断言：辅助函数全语义——host 白名单逐家正向命中（子域/端口大小写/多级子域）、
// 自建中转与伪冒域（前缀粘连/后缀无关）一律不命中；host_of 剥端口/userinfo/
// 大小写/无协议前缀；display_name 按名字→host→id 三级回退；round1 一位小数
// 四舍五入、clamp_percent 夹取 0–100；sanitize 抹除长密钥、短于 8 不处理、
// 超长截断 160；六态失败归类（401/403/404 首次 rejected、曾成功 error，
// 超时/5xx/429/其余 4xx 恒 error）。
#[test]
fn helpers_host_display_round_clamp_sanitize_classify() {
    // host 白名单正向命中
    let cases: &[(&str, AdapterKind)] = &[
        ("https://api.deepseek.com/v1", AdapterKind::DeepSeek),
        ("https://api.deepseek.com", AdapterKind::DeepSeek),
        ("https://opencode.ai/zen/go/v1", AdapterKind::OpenCode),
        ("https://zen.opencode.ai/v1", AdapterKind::OpenCode),
        ("https://api.minimax.io/v1", AdapterKind::MiniMaxIntl),
        ("https://www.minimax.cn/v1", AdapterKind::MiniMaxCn),
        ("https://api.minimax.cn/anthropic", AdapterKind::MiniMaxCn),
        ("https://a.b.minimax.cn/v1", AdapterKind::MiniMaxCn),
        ("https://api.kimi.com/coding/v1", AdapterKind::Kimi),
        ("https://api.moonshot.cn/v1", AdapterKind::Kimi),
        ("https://bigmodel.cn/api/monitor", AdapterKind::Zhipu),
        ("https://api.z.ai/api/monitor", AdapterKind::Zai),
    ];
    for (url, kind) in cases {
        assert_eq!(kind_for_base_url(url), Some(*kind), "应命中：{url}");
    }
    let misses = [
        "https://my-gateway.example.com/v1",
        "https://evildeepseek.com/v1",
        "https://api.minimax.com/v1",
        "https://evilminimax.cn/v1",
        "https://deepseek.com.evil.io/v1",
        "",
        "   ",
    ];
    for url in misses {
        assert_eq!(kind_for_base_url(url), None, "不应命中：{url}");
    }

    // host_of：端口、userinfo、大小写、无协议前缀都剥干净
    assert_eq!(
        host_of("https://API.DeepSeek.COM:8443/v1").as_deref(),
        Some("api.deepseek.com")
    );
    assert_eq!(
        host_of("https://user@api.kimi.com/x").as_deref(),
        Some("api.kimi.com")
    );
    assert_eq!(
        host_of("api.minimax.io/v1").as_deref(),
        Some("api.minimax.io")
    );
    assert_eq!(
        host_of("HTTP://BIGMODEL.CN/api").as_deref(),
        Some("bigmodel.cn")
    );
    assert_eq!(host_of("").as_deref(), None);
    assert_eq!(host_of("https://").as_deref(), None);

    // display_name：名字 → host → id 三级回退
    assert_eq!(
        display_name("  My Kimi  ", "https://api.kimi.com/x", "p1"),
        "My Kimi"
    );
    assert_eq!(
        display_name("   ", "https://api.kimi.com/x", "p2"),
        "api.kimi.com"
    );
    assert_eq!(display_name("", "", "p3"), "p3");

    // round1 / clamp_percent
    assert_eq!(round1(12.34), 12.3);
    assert_eq!(round1(12.35), 12.4);
    assert_eq!(round1(-0.04), -0.0);
    assert_eq!(clamp_percent(101.0), 100.0);
    assert_eq!(clamp_percent(-3.0), 0.0);
    assert_eq!(clamp_percent(33.333_333), 33.3);

    // sanitize：抹密钥、短密钥不处理、超长截断
    let secret = "sk-very-secret-key-12345";
    let text = format!("连接失败：Bearer {secret} 被拒");
    let sanitized = sanitize(&text, secret);
    assert!(!sanitized.contains(secret), "密钥必须被抹除：{sanitized}");
    assert!(sanitized.contains("[redacted]"));
    assert_eq!(sanitize("包含 ab 的文本", "ab"), "包含 ab 的文本");
    let long = "x".repeat(300);
    assert_eq!(sanitize(&long, "").chars().count(), 160);

    // 六态失败归类：401/403/404 且从未成功过 → rejected；曾成功 → error；
    // 超时（无状态）、5xx、429、其余 4xx → 恒 error
    for status in [401, 403, 404] {
        assert_eq!(
            classify_failure(&failure(Some(status)), false),
            QuotaStatus::Rejected,
            "首次 {status} 应为 rejected"
        );
        assert_eq!(
            classify_failure(&failure(Some(status)), true),
            QuotaStatus::Error,
            "曾成功后的 {status} 应为 error"
        );
    }
    for status in [None, Some(500), Some(502), Some(429), Some(400)] {
        assert_eq!(
            classify_failure(&failure(status), false),
            QuotaStatus::Error,
            "{status:?} 应为 error"
        );
    }
}

// ---------- DeepSeek 解析 ----------

// 断言：DeepSeek 解析——多币种余额各成一行（key=balance_<币种>、数值行不带
// 百分比、字符串与数字余额都接受）；total 缺失回退 granted+topped 之和并标
// 不可用；无可视余额（空体/无 total/无余额字段）报错。
#[test]
fn deepseek_parser_normal_and_error() {
    // 正常：多币种
    let body = json!({
        "is_available": true,
        "balance_infos": [
            {"currency": "CNY", "total_balance": "110.25", "granted_balance": "10.00", "topped_up_balance": "100.25"},
            {"currency": "USD", "total_balance": 8.4}
        ]
    });
    let entries = deepseek::parse_balance(&body).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].key, "balance_cny");
    assert_eq!(entries[0].value_text.as_deref(), Some("CNY 110.25"));
    assert_eq!(entries[1].key, "balance_usd");
    assert_eq!(entries[1].value_text.as_deref(), Some("USD 8.40"));
    assert!(entries[0].remaining_percent.is_none(), "数值行不带百分比");

    // 变体：total 缺失 → granted + topped 之和 + 不可用标记
    let body = json!({
        "is_available": false,
        "balance_infos": [
            {"currency": "cny", "granted_balance": "10.50", "topped_up_balance": "20.00"}
        ]
    });
    let entries = deepseek::parse_balance(&body).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].value_text.as_deref(), Some("CNY 30.50 · 不可用"));

    // 错误：无可视余额
    assert!(deepseek::parse_balance(&json!({})).is_err());
    assert!(
        deepseek::parse_balance(
            &json!({"balance_infos": [{"currency": "EUR", "total_balance": "5"}]})
        )
        .is_err()
    );
    assert!(deepseek::parse_balance(&json!({"balance_infos": [{"currency": "CNY"}]})).is_err());
}

// ---------- Kimi 解析 ----------

// 断言：Kimi 解析——顶层 usage 行（已用/剩余百分比、RFC3339 归一时间戳）与
// limits 各窗（name 标签、数字字符串 limit、reset_in 秒数→now+窗、remaining
// 语义、窗口时长标签）；data 信封解包；有 limit 无 used/remaining 整行跳过
// （不伪造剩余 0%）；空 body 失败；纯时间戳 reset_time 与 window.duration
// 两种重置来源。
#[test]
fn kimi_parser_normal_and_error() {
    // 正常：顶层 usage + 两档 limits
    let body = json!({
        "usage": {"limit": 100, "used": 25.5, "reset_at": "2026-11-02T00:00:00+08:00"},
        "limits": [
            {"name": "5h 窗口", "detail": {"limit": "200", "used": 50, "reset_in": 3600}},
            {"window": {"duration": 5, "timeUnit": "HOUR"}, "detail": {"limit": 400, "remaining": 100}}
        ]
    });
    let entries = kimi::parse_usage(&body, fixed_now()).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].key, "usage");
    assert_eq!(entries[0].used_percent, Some(25.5));
    assert_eq!(entries[0].remaining_percent, Some(74.5));
    assert_eq!(
        entries[0].resets_at.as_deref(),
        Some("2026-11-01T16:00:00Z"),
        "字符串时间戳按 RFC3339 归一"
    );
    assert_eq!(entries[1].key, "limit_1");
    assert_eq!(entries[1].label.as_deref(), Some("5h 窗口"));
    assert_eq!(
        entries[1].used_percent,
        Some(25.0),
        "数字字符串 limit 也接受"
    );
    assert_eq!(
        entries[1].resets_at.as_deref(),
        Some("2026-10-30T09:00:00Z"),
        "reset_in 秒数 → now + 1h"
    );
    assert_eq!(entries[2].key, "limit_2");
    assert_eq!(
        entries[2].label.as_deref(),
        Some("5h"),
        "按窗口时长生成标签"
    );
    assert_eq!(
        entries[2].used_percent,
        Some(75.0),
        "remaining 语义（limit-remaining=已用）"
    );

    // data 信封解包 + 错误态
    let body = json!({
        "data": {
            "usage": {"limit": 10, "used": 1},
            "limits": [{"detail": {"limit": 10, "used": 2}}]
        }
    });
    let entries = kimi::parse_usage(&body, fixed_now()).unwrap();
    assert_eq!(entries.len(), 2, "data 信封解包");
    let only_limit = json!({"usage": {"limit": 10}, "limits": [{"detail": {"limit": 5}}]});
    assert!(
        kimi::parse_usage(&only_limit, fixed_now()).is_err(),
        "有 limit 无 used/remaining → 不伪造剩余 0%"
    );
    assert!(
        kimi::parse_usage(&json!({}), fixed_now()).is_err(),
        "空 body 失败"
    );

    // 重置时间两种来源：纯时间戳 / window.duration 秒数
    let by_timestamp = json!({"limit": 10, "used": 1, "reset_time": "2026-11-01 12:00:00"});
    let entries =
        kimi::parse_usage(&json!({"usage": by_timestamp, "limits": []}), fixed_now()).unwrap();
    assert_eq!(
        entries[0].resets_at.as_deref(),
        Some("2026-11-01T12:00:00Z")
    );
    let by_window = json!({"limit": 10, "used": 1, "window": {"duration": 60}});
    let entries = kimi::parse_usage(&json!({"usage": by_window}), fixed_now()).unwrap();
    assert_eq!(
        entries[0].resets_at.as_deref(),
        Some("2026-10-30T08:01:00Z"),
        "窗口时长秒数 → now + duration"
    );
}

// ---------- MiniMax 解析 ----------

/// MiniMax 单模型响应体（国际形态：count 为「剩余」语义；周窗固定剩 90%）。
fn minimax_model(name: &str, five_hour_remaining: i64, weekly_remaining: i64) -> Value {
    json!({
        "model_name": name,
        "current_interval_total_count": 100,
        "current_interval_usage_count": five_hour_remaining,
        "remains_time": 3_600_000i64,
        "current_weekly_total_count": 700,
        "current_weekly_usage_count": weekly_remaining,
        "weekly_remains_time": 86_400_000i64
    })
}

// 断言：MiniMax 解析——国际端点 count=剩余语义 + 通配 minimax-m* 择优、
// remains_time 毫秒→now+窗；中国端点 count=已用语义（剩余反转）+ 择优取剩余
// 最差者；非套餐模型被滤空即失败；base_resp 业务错误带上游消息；video 作为
// 中国站额外额度模型收纳。
#[test]
fn minimax_parser_intl_cn_and_error() {
    // 国际端点：count=剩余，通配择优
    let body = json!({
        "base_resp": {"status_code": 0},
        "model_remains": [
            minimax_model("minimax-m2", 25, 630),
            minimax_model("minimax-m*", 60, 630)
        ]
    });
    let entries = minimax::parse_usage(&body, Endpoint::International, fixed_now()).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].key, "five_hour");
    assert_eq!(entries[0].remaining_percent, Some(60.0), "通配 m* 择优命中");
    assert_eq!(entries[0].used_percent, Some(40.0));
    assert_eq!(
        entries[0].resets_at.as_deref(),
        Some("2026-10-30T09:00:00Z"),
        "remains_time 毫秒 → now + 1h"
    );
    assert_eq!(entries[1].key, "week");
    assert_eq!(entries[1].remaining_percent, Some(90.0));

    // 中国端点：count=已用，剩余语义反转，择优 = 剩余最差者
    let body = json!({
        "base_resp": {"status_code": 0},
        "model_remains": [
            minimax_model("minimax-m2", 25, 70),
            minimax_model("general", 80, 70)
        ]
    });
    let entries = minimax::parse_usage(&body, Endpoint::China, fixed_now()).unwrap();
    // m2 的 5 小时已用 25/100 → 剩 75%；general 已用 80 → 剩 20%
    // （两家的周窗同为已用 70/700 → 剩 90%，不改变最差窗口归属）
    assert_eq!(entries[0].key, "five_hour");
    assert_eq!(entries[0].remaining_percent, Some(20.0), "择优取剩余最差者");

    // 错误与过滤：非套餐模型全被滤掉 → 失败
    let filtered = json!({
        "base_resp": {"status_code": 0},
        "model_remains": [
            {"model_name": "abab-random", "current_interval_total_count": 100, "current_interval_usage_count": 10}
        ]
    });
    assert!(
        minimax::parse_usage(&filtered, Endpoint::International, fixed_now()).is_err(),
        "非套餐模型被滤空即失败"
    );
    // base_resp.status_code != 0 → 上游业务错误带消息
    let errored = json!({
        "base_resp": {"status_code": 1004, "status_msg": "invalid api key"},
        "model_remains": []
    });
    let err = minimax::parse_usage(&errored, Endpoint::China, fixed_now()).unwrap_err();
    assert!(err.contains("invalid api key"), "错误要带上游消息：{err}");
    // video 是中国站的额外额度模型，也要收纳
    let with_video = json!({
        "base_resp": {"status_code": 0},
        "model_remains": [minimax_model("video", 50, 50)]
    });
    assert!(minimax::parse_usage(&with_video, Endpoint::China, fixed_now()).is_ok());
}

// ---------- 智谱 / Z.ai 解析 ----------

fn glm_limit(kind: &str, unit: i64, percentage: f64) -> Value {
    json!({"type": kind, "unit": unit, "percentage": percentage, "nextResetTime": 1_798_228_800_000i64})
}

// 断言：智谱/Z.ai 双 flavor 解析——智谱 data.limits 按 unit 语义分档
//（3=5h 窗、6=周窗、TIME_LIMIT=MCP 时长）且未知类型跳过、nextResetTime 毫秒
// →RFC3339；Z.ai data.limits 缺失回顶层 limits；业务错误（success=false）带
// 消息失败且智谱不认 success 字段；CREDIT_LIMIT 只在 Z.ai 纳入。
#[test]
fn glm_parser_zhipu_zai_and_error() {
    // 智谱：unit 语义分档
    let body = json!({
        "data": {"limits": [
            glm_limit("TOKENS_LIMIT", 3, 10.0),
            glm_limit("TOKENS_LIMIT", 6, 20.0),
            glm_limit("TIME_LIMIT", 0, 30.0),
            glm_limit("UNKNOWN_TYPE", 3, 40.0)
        ]}
    });
    let entries = glm::parse_limits(&body, Flavor::Zhipu).unwrap();
    assert_eq!(entries.len(), 3, "未知类型跳过");
    assert_eq!(entries[0].key, "five_hour");
    assert_eq!(entries[0].used_percent, Some(10.0));
    assert_eq!(entries[0].remaining_percent, Some(90.0));
    assert_eq!(entries[1].key, "weekly");
    assert_eq!(entries[2].key, "mcp");
    assert_eq!(
        entries[0].resets_at.as_deref(),
        Some("2026-12-25T20:00:00Z"),
        "nextResetTime 毫秒 → RFC3339"
    );

    // Z.ai：顶层 limits 回退 + 业务错误 + flavor 差异
    let body = json!({"success": true, "limits": [glm_limit("TOKENS_LIMIT", 3, 55.5)]});
    let entries = glm::parse_limits(&body, Flavor::Zai).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].used_percent,
        Some(55.5),
        "data.limits 缺失回顶层"
    );
    let failed = json!({"success": false, "msg": "token 无效", "code": 401});
    let err = glm::parse_limits(&failed, Flavor::Zai).unwrap_err();
    assert_eq!(err, "token 无效", "业务错误带消息");
    assert!(
        glm::parse_limits(&failed, Flavor::Zhipu).is_err(),
        "智谱不认 Z.ai 的 success 字段"
    );
    let credit = json!({"success": true, "limits": [glm_limit("CREDIT_LIMIT", 3, 5.0)]});
    assert!(
        glm::parse_limits(&credit, Flavor::Zai).is_ok(),
        "CREDIT_LIMIT 只在 Z.ai"
    );
    assert!(glm::parse_limits(&credit, Flavor::Zhipu).is_err());
}

// ---------- OpenCode 解析 ----------

// 断言：OpenCode 解析——rolling/weekly/monthly 三窗各成一行（percent 数字
// 字符串也接受、resetsAt 直通、剩余=100-已用）；rate-limited 用满状态绝不
// 被过滤（最该被看到的一档）；缺 usage / 全窗无百分比失败。
#[test]
fn opencode_parser_normal_and_error() {
    let body = json!({
        "usage": {
            "rolling": {"status": "ok", "percent": 12.34, "resetsAt": "2026-11-02T00:00:00Z"},
            "weekly": {"status": "rate-limited", "percent": 100},
            "monthly": {"percent": "40"}
        }
    });
    let entries = opencode::parse_usage(&body).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].key, "rolling");
    assert_eq!(entries[0].used_percent, Some(12.3));
    assert_eq!(entries[0].remaining_percent, Some(87.7));
    assert_eq!(
        entries[0].resets_at.as_deref(),
        Some("2026-11-02T00:00:00Z")
    );
    assert_eq!(
        entries[1].status.as_deref(),
        Some("rate-limited"),
        "用满状态绝不能被过滤"
    );
    assert_eq!(entries[1].remaining_percent, Some(0.0));
    assert_eq!(
        entries[2].used_percent,
        Some(40.0),
        "数字字符串 percent 接受"
    );

    // 错误：缺 usage / 全窗无百分比
    assert!(opencode::parse_usage(&json!({})).is_err());
    assert!(opencode::parse_usage(&json!({"usage": {"rolling": {"status": "ok"}}})).is_err());
}

// ---------- fetch_json 的 HTTP 面（axum mock server）----------

/// axum mock：/balance 按场景回放状态码与响应体。
async fn status_mock(status: u16, body: Value) -> std::net::SocketAddr {
    let app = axum::Router::new().route(
        "/balance",
        get(move || async move {
            (
                axum::http::StatusCode::from_u16(status).unwrap(),
                Json(body),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

// 断言：fetch_json 的 HTTP 面——2xx 返回响应体；HTTP 错误带状态码（供
// rejected 判定）且错误消息脱敏不回显 Key；非 JSON 响应报解析失败（无 HTTP
// 状态）；不可达地址报无状态失败（status=None → error 态）。
#[tokio::test]
async fn fetch_json_http_face() {
    // 2xx：返回响应体
    let addr = status_mock(200, json!({"ok": true})).await;
    let client = crate::providers::http::build_proxy_client(None).unwrap();
    let body = fetch_json(
        &client,
        &format!("http://{addr}/balance"),
        "sk-test-key-12345678",
        AuthStyle::Bearer,
        "Mock",
    )
    .await
    .unwrap();
    assert_eq!(body, json!({"ok": true}));

    // HTTP 错误：带状态码 + 脱敏
    let secret = "sk-test-key-12345678";
    let addr = status_mock(401, json!({"error": "bad key sk-test-key-12345678"})).await;
    let client = crate::providers::http::build_proxy_client(None).unwrap();
    let err = fetch_json(
        &client,
        &format!("http://{addr}/balance"),
        secret,
        AuthStyle::Raw,
        "Mock",
    )
    .await
    .unwrap_err();
    assert_eq!(
        err.status,
        Some(401),
        "HTTP 错误必须带状态码供 rejected 判定"
    );
    assert!(
        !err.message.contains(secret),
        "错误消息必须脱敏：{}",
        err.message
    );
    assert!(err.message.contains("401"));

    // 解析失败：无 HTTP 状态、消息带 JSON
    let app = axum::Router::new().route("/balance", get(|| async { "这不是 JSON" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = crate::providers::http::build_proxy_client(None).unwrap();
    let err = fetch_json(
        &client,
        &format!("http://{addr}/balance"),
        "sk-test-key-12345678",
        AuthStyle::Bearer,
        "Mock",
    )
    .await
    .unwrap_err();
    assert_eq!(err.status, None, "解析失败无 HTTP 状态");
    assert!(err.message.contains("JSON"));

    // 不可达：端口 1（保留端口）连接必然失败 → 无状态失败（status=None → error 态）
    let client = crate::providers::http::build_proxy_client(None).unwrap();
    let err = fetch_json(
        &client,
        "http://127.0.0.1:1/balance",
        "sk-test-key-12345678",
        AuthStyle::Bearer,
        "Mock",
    )
    .await
    .unwrap_err();
    assert_eq!(err.status, None);
}

// ---------- 快照主流程：六态、排序与不发包纪律 ----------

// 断言：快照主流程——可查询类在前、unsupported 殿后（均按配置序），只有真正
// 要查的供应商发请求；无 Key（缺 Key 与空白 Key 归一后等价）不发请求且占位行
// 不落盘；401 首次 rejected 且 lastOkAt=None 但失败行落盘（供孤儿清理与
// 「曾成功」判定依据）；401 曾成功过 → error + 历史 lastOkAt + 离线兜底明细
//（不变式：lastOkAt 恒上次成功时刻、data.entries 恒最近一次成功明细）；
// 超时/5xx/解析错三家全败各得一行 error 且互不拦截。
#[tokio::test]
async fn snapshot_states_and_request_discipline() {
    // 排序与不发包：unsupported 殿后、只有可查的一家发包
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(vec![
        provider("p-empty", "空端点", "", Some("sk-empty")),
        provider(
            "p-ok",
            "DeepSeek",
            "https://api.deepseek.com/v1",
            Some("sk-ok"),
        ),
        provider(
            "p-gateway",
            "中转",
            "https://gateway.example.com/v1",
            Some("sk-gateway"),
        ),
    ]);
    let fake = FakeFetcher::new(vec![("sk-ok", Ok(vec![percent_entry("balance", 10.0)]))]);
    let out = snapshots_with(&cfg, dir.path(), &fake.fetcher()).await;
    assert_eq!(ids(&out), vec!["p-ok", "p-empty", "p-gateway"]);
    assert_eq!(out[0].state, QuotaStatus::Ok);
    assert_eq!(
        out[0].data.as_ref().unwrap().key_source.as_deref(),
        Some("config")
    );
    assert_eq!(out[1].state, QuotaStatus::Unsupported);
    assert_eq!(
        out[1].data.as_ref().unwrap().reason.as_deref(),
        Some(REASON_EMPTY_BASE_URL)
    );
    assert_eq!(out[1].last_ok_at, None);
    assert_eq!(out[2].state, QuotaStatus::Unsupported);
    assert_eq!(
        out[2].data.as_ref().unwrap().reason.as_deref(),
        Some(REASON_NO_ADAPTER)
    );
    assert_eq!(fake.calls(), 1, "只有真正要查的那一家发了请求");

    // 无 Key：不发请求、占位行不落盘
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(vec![provider(
        "p-nokey",
        "无 Key",
        "https://api.deepseek.com/v1",
        None,
    )]);
    let fake = FakeFetcher::new(vec![]);
    let out = snapshots_with(&cfg, dir.path(), &fake.fetcher()).await;
    assert_eq!(states(&out), vec![QuotaStatus::NoKey]);
    assert_eq!(fake.calls(), 0, "no_key 不发请求");
    assert!(!dir.path().join(FILE_NAME).exists(), "占位行不落盘");

    // 空白 Key 归一后同无 Key
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(vec![provider(
        "p-blank",
        "空白 Key",
        "https://api.deepseek.com/v1",
        Some("   "),
    )]);
    let fake = FakeFetcher::new(vec![]);
    let out = snapshots_with(&cfg, dir.path(), &fake.fetcher()).await;
    assert_eq!(states(&out), vec![QuotaStatus::NoKey]);
    assert_eq!(fake.calls(), 0);

    // 首次 401 → rejected；lastOkAt=None 但失败行落盘
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(vec![provider(
        "p-bad",
        "坏 Key",
        "https://api.deepseek.com/v1",
        Some("sk-bad"),
    )]);
    let fake = FakeFetcher::new(vec![("sk-bad", Err(failure(Some(401))))]);
    let out = snapshots_with(&cfg, dir.path(), &fake.fetcher()).await;
    assert_eq!(states(&out), vec![QuotaStatus::Rejected]);
    assert_eq!(out[0].last_ok_at, None, "从未成功恒 None");
    let persisted = state::load(dir.path());
    assert_eq!(persisted.len(), 1, "失败行落盘（孤儿清理依据）");
    assert_eq!(persisted[0].state, QuotaStatus::Rejected);

    // 曾成功后的 401 → error + 历史 lastOkAt + 离线兜底明细
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(vec![provider(
        "p-expired",
        "失效 Key",
        "https://api.deepseek.com/v1",
        Some("sk-expired"),
    )]);
    let ok = FakeFetcher::new(vec![(
        "sk-expired",
        Ok(vec![percent_entry("balance", 20.0)]),
    )]);
    let out = snapshots_with(&cfg, dir.path(), &ok.fetcher()).await;
    assert_eq!(states(&out), vec![QuotaStatus::Ok]);
    let last_ok = out[0].last_ok_at.clone().unwrap();
    let bad = FakeFetcher::new(vec![("sk-expired", Err(failure(Some(401))))]);
    let out = snapshots_with(&cfg, dir.path(), &bad.fetcher()).await;
    assert_eq!(states(&out), vec![QuotaStatus::Error]);
    assert_eq!(
        out[0].last_ok_at.as_deref(),
        Some(last_ok.as_str()),
        "不变式：lastOkAt 恒上次成功时刻"
    );
    let data = out[0].data.as_ref().unwrap();
    assert!(data.error.is_some());
    assert_eq!(
        data.entries.len(),
        1,
        "不变式：失败行带最近一次成功明细（离线兜底）"
    );
    assert_eq!(data.entries[0].key, "balance");

    // 无状态失败（超时/5xx/解析错）：三家全败各得一行 error，互不拦截
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(vec![
        provider(
            "p-timeout",
            "超时",
            "https://api.deepseek.com/v1",
            Some("sk-timeout"),
        ),
        provider(
            "p-5xx",
            "服务错",
            "https://api.kimi.com/coding/v1",
            Some("sk-5xx"),
        ),
        provider(
            "p-parse",
            "解析错",
            "https://api.z.ai/api",
            Some("sk-parse"),
        ),
    ]);
    let fake = FakeFetcher::new(vec![
        ("sk-timeout", Err(failure(None))),
        ("sk-5xx", Err(failure(Some(500)))),
        ("sk-parse", Err(failure(None))),
    ]);
    let out = snapshots_with(&cfg, dir.path(), &fake.fetcher()).await;
    assert_eq!(
        states(&out),
        vec![QuotaStatus::Error, QuotaStatus::Error, QuotaStatus::Error]
    );
    assert_eq!(fake.calls(), 3, "每家独立取数，一家失败不拦截其余");
}

// ---------- quota.json 行合并不变式、孤儿清理、损坏隔离与 wire 面 ----------

// 断言：quota.json 行合并全语义——成功行整行覆盖（entries/lastOkAt 取新值）；
// 失败行继承盘上旧值（不变式：lastOkAt 恒上次成功时刻、data.entries 恒最近
// 一次成功明细，离线兜底）；孤儿清理（ids 之外旧行删除）触发写盘，无变化不
// 写盘（返回 false）；损坏文件隔离为 .corrupt 备份并按空快照继续；行格式
// serde roundtrip（camelCase 键面 + snake_case 六态 wire 名）。
#[tokio::test]
async fn commit_merge_invariants_orphans_corrupt_serde() {
    // 行合并：第一批一家成功一家失败，第二批成功覆盖、失败继承
    let dir = tempfile::tempdir().unwrap();
    let ids_list = ["p-ok".to_string(), "p-fail".to_string()];
    let now_a = "2026-10-30T08:00:00Z";
    let ok_row = QuotaSnapshot {
        provider_id: "p-ok".into(),
        state: QuotaStatus::Ok,
        data: Some(QuotaData {
            display_name: "正常".into(),
            entries: vec![percent_entry("balance", 10.0)],
            reason: None,
            error: None,
            key_source: Some("config".into()),
        }),
        fetched_at: now_a.into(),
        last_ok_at: Some(now_a.into()),
    };
    let fail_row = QuotaSnapshot {
        provider_id: "p-fail".into(),
        state: QuotaStatus::Rejected,
        data: Some(QuotaData {
            display_name: "被拒".into(),
            entries: Vec::new(),
            reason: None,
            error: Some("401".into()),
            key_source: Some("config".into()),
        }),
        fetched_at: now_a.into(),
        last_ok_at: None,
    };
    assert!(state::commit(
        dir.path(),
        &ids_list,
        &[ok_row, fail_row],
        now_a
    ));
    assert_eq!(state::load(dir.path()).len(), 2);

    // 第二批：成功行覆盖、失败行继承（entries/lastOkAt 来自盘上旧值）
    let now_b = "2026-10-30T09:00:00Z";
    let ok_row2 = QuotaSnapshot {
        provider_id: "p-ok".into(),
        state: QuotaStatus::Ok,
        data: Some(QuotaData {
            display_name: "正常".into(),
            entries: vec![percent_entry("balance", 30.0)],
            reason: None,
            error: None,
            key_source: Some("config".into()),
        }),
        fetched_at: now_b.into(),
        last_ok_at: Some(now_b.into()),
    };
    let fail_row2 = QuotaSnapshot {
        provider_id: "p-fail".into(),
        state: QuotaStatus::Error,
        data: Some(QuotaData {
            display_name: "被拒".into(),
            entries: Vec::new(),
            reason: None,
            error: Some("超时".into()),
            key_source: Some("config".into()),
        }),
        fetched_at: now_b.into(),
        last_ok_at: None,
    };
    assert!(state::commit(
        dir.path(),
        &ids_list,
        &[ok_row2, fail_row2],
        now_b
    ));
    let disk = state::load(dir.path());
    let ok_line = disk.iter().find(|r| r.provider_id == "p-ok").unwrap();
    assert_eq!(ok_line.state, QuotaStatus::Ok);
    assert_eq!(
        ok_line.data.as_ref().unwrap().entries[0].used_percent,
        Some(30.0),
        "成功行整行覆盖"
    );
    assert_eq!(ok_line.last_ok_at.as_deref(), Some(now_b));
    let fail_line = disk.iter().find(|r| r.provider_id == "p-fail").unwrap();
    assert_eq!(fail_line.state, QuotaStatus::Error);
    assert_eq!(
        fail_line.last_ok_at, None,
        "失败行继承旧行 lastOkAt（此前从未成功）"
    );

    // 第三批：此前成功过的 p-ok 超时 → error 行继承旧明细与 lastOkAt
    let now_c = "2026-10-30T10:00:00Z";
    let timeout_row = QuotaSnapshot {
        provider_id: "p-ok".into(),
        state: QuotaStatus::Error,
        data: Some(QuotaData {
            display_name: "正常".into(),
            entries: Vec::new(),
            reason: None,
            error: Some("超时".into()),
            key_source: Some("config".into()),
        }),
        fetched_at: now_c.into(),
        last_ok_at: None,
    };
    assert!(state::commit(dir.path(), &ids_list, &[timeout_row], now_c));
    let disk = state::load(dir.path());
    let line = disk.iter().find(|r| r.provider_id == "p-ok").unwrap();
    assert_eq!(line.state, QuotaStatus::Error);
    assert_eq!(
        line.last_ok_at.as_deref(),
        Some(now_b),
        "不变式：继承上次成功时刻"
    );
    assert_eq!(
        line.data.as_ref().unwrap().entries.len(),
        1,
        "不变式：失败行继承最近一次成功明细（离线兜底）"
    );

    // 孤儿清理（ids 只剩 p-kept）→ 写盘；无请求结果且无孤儿 → 不写盘（false）
    let dir = tempfile::tempdir().unwrap();
    let kept_ids = ["p-kept".to_string()];
    let now = "2026-10-30T08:00:00Z";
    let row = QuotaSnapshot {
        provider_id: "p-kept".into(),
        state: QuotaStatus::Ok,
        data: Some(QuotaData {
            display_name: "保留".into(),
            entries: vec![percent_entry("balance", 5.0)],
            reason: None,
            error: None,
            key_source: None,
        }),
        fetched_at: now.into(),
        last_ok_at: Some(now.into()),
    };
    let mut orphan = row.clone();
    orphan.provider_id = "p-orphan".into();
    assert!(state::commit(
        dir.path(),
        &["p-kept".to_string(), "p-orphan".to_string()],
        &[row.clone(), orphan],
        now
    ));
    assert!(
        state::commit(dir.path(), &kept_ids, &[], now),
        "孤儿清理写盘"
    );
    let disk = state::load(dir.path());
    assert_eq!(ids(&disk), vec!["p-kept"]);
    assert!(
        !state::commit(dir.path(), &kept_ids, &[], now),
        "无变化不写盘"
    );

    // 损坏文件：隔离为 .corrupt、按空快照继续
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE_NAME), b"{broken json").unwrap();
    let rows = state::load(dir.path());
    assert!(rows.is_empty(), "损坏按空快照继续");
    assert!(
        dir.path().join(format!("{FILE_NAME}.corrupt")).exists(),
        "损坏文件备份留证"
    );
    assert!(!dir.path().join(FILE_NAME).exists(), "原文件已被移走");

    // wire 面：camelCase 键面 + snake_case 六态 wire 名 + roundtrip
    let row = QuotaSnapshot {
        provider_id: "p1".into(),
        state: QuotaStatus::Ok,
        data: Some(QuotaData {
            display_name: "展示名".into(),
            entries: vec![percent_entry("five_hour", 12.5)],
            reason: None,
            error: None,
            key_source: Some("config".into()),
        }),
        fetched_at: "2026-10-30T08:00:00Z".into(),
        last_ok_at: Some("2026-10-30T08:00:00Z".into()),
    };
    let text = serde_json::to_string(&row).unwrap();
    assert!(text.contains("\"providerId\""));
    assert!(text.contains("\"lastOkAt\""));
    assert!(text.contains("\"usedPercent\""));
    assert!(text.contains("\"state\":\"ok\""), "snake_case 六态 wire 名");
    let back: QuotaSnapshot = serde_json::from_str(&text).unwrap();
    assert_eq!(back, row, "serde roundtrip");
    let rejected = QuotaSnapshot {
        state: QuotaStatus::Rejected,
        ..QuotaSnapshot::default()
    };
    assert!(
        serde_json::to_string(&rejected)
            .unwrap()
            .contains("\"state\":\"rejected\"")
    );
}
