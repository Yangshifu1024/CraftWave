//! 配额域单测（21 号票，FR-02-15）：全本地、不发真实网络请求。
//!
//! 覆盖面（票面「测试」节）：
//! - 每家适配器 `parse_*` 纯解析（正常 / 缺字段 / 数字字符串 / 超期窗口）；
//! - host 匹配表驱动（子域 / 端口 / @ userinfo / 大小写陷阱 / 伪冒域）；
//! - 六态分类（`FakeFetcher` 注入 + axum mock server 打 `fetch_json` 的
//!   HTTP 面：200 / 401 首次 / 401 曾成功 / 超时 / 解析失败 / 无 Key /
//!   未命中）；
//! - `quota.json` 原子写与行合并（成功覆盖 / 失败继承 / 孤儿清理 / 损坏
//!   备份）。

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

// ---------- host 匹配（表驱动）----------

#[test]
fn kind_for_base_url_matches_whitelisted_hosts_only() {
    // 白名单逐家正向命中
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
    // 自建/中转网关、伪冒域（前缀粘连、后缀无关系）一律不命中
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
}

#[test]
fn host_of_handles_ports_userinfo_case_and_scheme_less() {
    // 端口、userinfo（@ 前的凭据段）、大小写、无协议前缀都要剥干净
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
}

#[test]
fn display_name_falls_back_to_base_url_host_then_id() {
    assert_eq!(
        display_name("  My Kimi  ", "https://api.kimi.com/x", "p1"),
        "My Kimi"
    );
    assert_eq!(
        display_name("   ", "https://api.kimi.com/x", "p2"),
        "api.kimi.com"
    );
    assert_eq!(display_name("", "", "p3"), "p3");
}

// ---------- 百分比与脱敏 ----------

#[test]
fn round1_and_clamp_percent_keep_one_decimal_in_range() {
    assert_eq!(round1(12.34), 12.3);
    assert_eq!(round1(12.35), 12.4);
    assert_eq!(round1(-0.04), -0.0);
    assert_eq!(clamp_percent(101.0), 100.0);
    assert_eq!(clamp_percent(-3.0), 0.0);
    assert_eq!(clamp_percent(33.333_333), 33.3);
}

#[test]
fn sanitize_redacts_long_secrets_and_truncates() {
    let secret = "sk-very-secret-key-12345";
    let text = format!("连接失败：Bearer {secret} 被拒");
    let sanitized = sanitize(&text, secret);
    assert!(!sanitized.contains(secret), "密钥必须被抹除：{sanitized}");
    assert!(sanitized.contains("[redacted]"));
    // 短于 8 的「密钥」不处理（避免把普通词替换掉）
    assert_eq!(sanitize("包含 ab 的文本", "ab"), "包含 ab 的文本");
    // 超长截断到 160 字符
    let long = "x".repeat(300);
    assert_eq!(sanitize(&long, "").chars().count(), 160);
}

// ---------- 失败归类（六态判据）----------

#[test]
fn classify_failure_rejects_only_never_succeeded_auth_statuses() {
    // 401/403/404 且从未成功过 → rejected（密钥/套餐被拒）
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
    // 超时/连接失败（无状态）、5xx、429、其余 4xx → 恒 error
    for status in [None, Some(500), Some(502), Some(429), Some(400)] {
        assert_eq!(
            classify_failure(&failure(status), false),
            QuotaStatus::Error,
            "{status:?} 应为 error"
        );
    }
}

// ---------- DeepSeek 解析 ----------

#[test]
fn deepseek_parses_balance_entries_per_currency() {
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
    // 数值行不带百分比
    assert!(entries[0].remaining_percent.is_none());
}

#[test]
fn deepseek_falls_back_to_granted_plus_topped_and_marks_unavailable() {
    // total 缺失 → granted + topped 之和
    let body = json!({
        "is_available": false,
        "balance_infos": [
            {"currency": "cny", "granted_balance": "10.50", "topped_up_balance": "20.00"}
        ]
    });
    let entries = deepseek::parse_balance(&body).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].value_text.as_deref(), Some("CNY 30.50 · 不可用"));
}

#[test]
fn deepseek_rejects_body_without_visible_balance() {
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

#[test]
fn kimi_parses_usage_and_limits_with_labels_and_reset_times() {
    let body = json!({
        "usage": {"limit": 100, "used": 25.5, "reset_at": "2026-11-02T00:00:00+08:00"},
        "limits": [
            {"name": "5h 窗口", "detail": {"limit": "200", "used": 50, "reset_in": 3600}},
            {"window": {"duration": 5, "timeUnit": "HOUR"}, "detail": {"limit": 400, "remaining": 100}}
        ]
    });
    let entries = kimi::parse_usage(&body, fixed_now()).unwrap();
    assert_eq!(entries.len(), 3);
    // 顶层 usage 行：已用 25.5% → 剩余 74.5%；字符串时间戳按 RFC3339 归一
    assert_eq!(entries[0].key, "usage");
    assert_eq!(entries[0].used_percent, Some(25.5));
    assert_eq!(entries[0].remaining_percent, Some(74.5));
    assert_eq!(
        entries[0].resets_at.as_deref(),
        Some("2026-11-01T16:00:00Z")
    );
    // limit_1：数字字符串 limit 也接受；reset_in 秒数 → now + 1h
    assert_eq!(entries[1].key, "limit_1");
    assert_eq!(entries[1].label.as_deref(), Some("5h 窗口"));
    assert_eq!(entries[1].used_percent, Some(25.0));
    assert_eq!(
        entries[1].resets_at.as_deref(),
        Some("2026-10-30T09:00:00Z")
    );
    // limit_2：remaining 语义（limit-remaining=已用）；标签按窗口时长生成
    assert_eq!(entries[2].key, "limit_2");
    assert_eq!(entries[2].label.as_deref(), Some("5h"));
    assert_eq!(entries[2].used_percent, Some(75.0));
}

#[test]
fn kimi_unwraps_data_envelope_and_rejects_empty_results() {
    let body = json!({
        "data": {
            "usage": {"limit": 10, "used": 1},
            "limits": [{"detail": {"limit": 10, "used": 2}}]
        }
    });
    let entries = kimi::parse_usage(&body, fixed_now()).unwrap();
    assert_eq!(entries.len(), 2);
    // 有 limit 无 used/remaining → 不伪造「剩余 0%」，整行跳过
    let only_limit = json!({"usage": {"limit": 10}, "limits": [{"detail": {"limit": 5}}]});
    assert!(kimi::parse_usage(&only_limit, fixed_now()).is_err());
    // 空 body → 失败
    assert!(kimi::parse_usage(&json!({}), fixed_now()).is_err());
}

#[test]
fn kimi_reset_time_accepts_plain_timestamp_and_window_duration() {
    let by_timestamp = json!({"limit": 10, "used": 1, "reset_time": "2026-11-01 12:00:00"});
    let entries =
        kimi::parse_usage(&json!({"usage": by_timestamp, "limits": []}), fixed_now()).unwrap();
    assert_eq!(
        entries[0].resets_at.as_deref(),
        Some("2026-11-01T12:00:00Z")
    );
    // 窗口时长（秒）→ now + duration
    let by_window = json!({"limit": 10, "used": 1, "window": {"duration": 60}});
    let entries = kimi::parse_usage(&json!({"usage": by_window}), fixed_now()).unwrap();
    assert_eq!(
        entries[0].resets_at.as_deref(),
        Some("2026-10-30T08:01:00Z")
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

#[test]
fn minimax_intl_counts_mean_remaining_and_prefers_wildcard() {
    let body = json!({
        "base_resp": {"status_code": 0},
        "model_remains": [
            minimax_model("minimax-m2", 25, 630),
            minimax_model("minimax-m*", 60, 630)
        ]
    });
    let entries = minimax::parse_usage(&body, Endpoint::International, fixed_now()).unwrap();
    // 通配 minimax-m* 择优命中：5 小时剩余 60% → 已用 40%
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].key, "five_hour");
    assert_eq!(entries[0].remaining_percent, Some(60.0));
    assert_eq!(entries[0].used_percent, Some(40.0));
    // remains_time 毫秒 → now + 1h
    assert_eq!(
        entries[0].resets_at.as_deref(),
        Some("2026-10-30T09:00:00Z")
    );
    assert_eq!(entries[1].key, "week");
    assert_eq!(entries[1].remaining_percent, Some(90.0));
}

#[test]
fn minimax_cn_counts_mean_used_and_picks_worst_model() {
    // 中国端点：count 是「已用」——同字段值下剩余语义反转
    let body = json!({
        "base_resp": {"status_code": 0},
        "model_remains": [
            minimax_model("minimax-m2", 25, 70),
            minimax_model("general", 80, 70)
        ]
    });
    let entries = minimax::parse_usage(&body, Endpoint::China, fixed_now()).unwrap();
    // 择优 = 剩余最差者：m2 的 5 小时已用 25/100 → 剩 75%；general 已用 80 → 剩 20%
    // （两家的周窗同为已用 70/700 → 剩 90%，不改变最差窗口归属）
    assert_eq!(entries[0].key, "five_hour");
    assert_eq!(entries[0].remaining_percent, Some(20.0));
}

#[test]
fn minimax_filters_non_coding_models_and_reports_base_resp_errors() {
    // 非套餐模型（无重置提示 / 非 minimax-m* 且非 general|video）全被滤掉 → 失败
    let filtered = json!({
        "base_resp": {"status_code": 0},
        "model_remains": [
            {"model_name": "abab-random", "current_interval_total_count": 100, "current_interval_usage_count": 10}
        ]
    });
    assert!(minimax::parse_usage(&filtered, Endpoint::International, fixed_now()).is_err());
    // base_resp.status_code != 0 → 上游业务错误
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

#[test]
fn zhipu_parses_data_limits_with_unit_semantics() {
    let body = json!({
        "data": {"limits": [
            glm_limit("TOKENS_LIMIT", 3, 10.0),
            glm_limit("TOKENS_LIMIT", 6, 20.0),
            glm_limit("TIME_LIMIT", 0, 30.0),
            glm_limit("UNKNOWN_TYPE", 3, 40.0)
        ]}
    });
    let entries = glm::parse_limits(&body, Flavor::Zhipu).unwrap();
    // unit 3 = 5 小时窗、unit 6 = 每周窗、TIME_LIMIT = MCP 时长；未知类型跳过
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].key, "five_hour");
    assert_eq!(entries[0].used_percent, Some(10.0));
    assert_eq!(entries[0].remaining_percent, Some(90.0));
    assert_eq!(entries[1].key, "weekly");
    assert_eq!(entries[2].key, "mcp");
    // nextResetTime 毫秒时间戳 → RFC3339（1_798_228_800_000 ms = 2026-12-25T20:00:00Z）
    assert_eq!(
        entries[0].resets_at.as_deref(),
        Some("2026-12-25T20:00:00Z")
    );
}

#[test]
fn zai_takes_top_level_limits_fallback_and_reports_api_errors() {
    // Z.ai：data.limits 缺失时回顶层 limits
    let body = json!({"success": true, "limits": [glm_limit("TOKENS_LIMIT", 3, 55.5)]});
    let entries = glm::parse_limits(&body, Flavor::Zai).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].used_percent, Some(55.5));
    // Z.ai 业务错误（success=false / code>=400）→ 带消息失败
    let failed = json!({"success": false, "msg": "token 无效", "code": 401});
    let err = glm::parse_limits(&failed, Flavor::Zai).unwrap_err();
    assert_eq!(err, "token 无效");
    // 智谱不认 Z.ai 的 success 字段（同体在智谱下按 data.limits 判缺失）
    assert!(glm::parse_limits(&failed, Flavor::Zhipu).is_err());
    // CREDIT_LIMIT 只在 Z.ai 纳入
    let credit = json!({"success": true, "limits": [glm_limit("CREDIT_LIMIT", 3, 5.0)]});
    assert!(glm::parse_limits(&credit, Flavor::Zai).is_ok());
    assert!(glm::parse_limits(&credit, Flavor::Zhipu).is_err());
}

// ---------- OpenCode 解析 ----------

#[test]
fn opencode_parses_windows_and_keeps_rate_limited_status() {
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
    // 用满（rate-limited）绝不能被过滤——最该被看到的一档
    assert_eq!(entries[1].status.as_deref(), Some("rate-limited"));
    assert_eq!(entries[1].remaining_percent, Some(0.0));
    // 数字字符串 percent 也接受
    assert_eq!(entries[2].used_percent, Some(40.0));
}

#[test]
fn opencode_rejects_missing_usage_and_empty_windows() {
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

#[tokio::test]
async fn fetch_json_returns_body_on_2xx() {
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
}

#[tokio::test]
async fn fetch_json_carries_status_and_sanitized_body_on_http_error() {
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
}

#[tokio::test]
async fn fetch_json_reports_parse_failure_without_status() {
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
}

#[tokio::test]
async fn fetch_json_reports_unreachable_as_stateless_failure() {
    // 端口 1（保留端口）连接必然失败 → 无状态失败（status=None → error 态）
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

// ---------- 快照主流程：六态与排序 ----------

#[tokio::test]
async fn unsupported_rows_never_send_requests_and_sort_last() {
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

    // 可查询类在前，unsupported 殿后（均按配置序）
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
    // 只有真正要查的那一家发了请求
    assert_eq!(fake.calls(), 1);
}

#[tokio::test]
async fn missing_key_yields_no_key_row_without_request() {
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
    // 占位行不落盘（无请求结果 → quota.json 不出现）
    assert!(!dir.path().join(FILE_NAME).exists());
}

#[tokio::test]
async fn first_auth_failure_is_rejected_and_recorded_without_history() {
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
    // 失败行落盘（下次判「曾成功过」的依据 = lastOkAt 有值与否；rejected
    // 从未成功 → lastOkAt 仍为 None，但行本身入文件清孤儿用）
    let persisted = state::load(dir.path());
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0].state, QuotaStatus::Rejected);
}

#[tokio::test]
async fn auth_failure_after_success_is_error_with_last_ok_at_and_history_entries() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = config(vec![provider(
        "p-expired",
        "失效 Key",
        "https://api.deepseek.com/v1",
        Some("sk-expired"),
    )]);
    // 第一批：成功（写快照 + lastOkAt）
    let ok = FakeFetcher::new(vec![(
        "sk-expired",
        Ok(vec![percent_entry("balance", 20.0)]),
    )]);
    let out = snapshots_with(&cfg, dir.path(), &ok.fetcher()).await;
    assert_eq!(states(&out), vec![QuotaStatus::Ok]);
    let last_ok = out[0].last_ok_at.clone().unwrap();

    // 第二批：401（曾成功过）→ error + 历史 lastOkAt + 离线兜底明细
    let bad = FakeFetcher::new(vec![("sk-expired", Err(failure(Some(401))))]);
    let out = snapshots_with(&cfg, dir.path(), &bad.fetcher()).await;
    assert_eq!(states(&out), vec![QuotaStatus::Error]);
    assert_eq!(out[0].last_ok_at.as_deref(), Some(last_ok.as_str()));
    let data = out[0].data.as_ref().unwrap();
    assert!(data.error.is_some());
    assert_eq!(
        data.entries.len(),
        1,
        "失败行带最近一次成功明细（离线兜底）"
    );
    assert_eq!(data.entries[0].key, "balance");
}

#[tokio::test]
async fn stateless_failures_are_error_and_single_provider_failure_is_isolated() {
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
    // 三家全失败也各得一行 error，互不影响
    assert_eq!(
        states(&out),
        vec![QuotaStatus::Error, QuotaStatus::Error, QuotaStatus::Error]
    );
    assert_eq!(fake.calls(), 3, "每家独立取数，一家失败不拦截其余");
}

#[tokio::test]
async fn blank_api_key_after_normalize_is_treated_as_no_key() {
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
}

// ---------- quota.json 行合并与原子写 ----------

#[tokio::test]
async fn commit_overwrites_ok_rows_and_failure_rows_inherit_history() {
    let dir = tempfile::tempdir().unwrap();
    let ids = ["p-ok".to_string(), "p-fail".to_string()];
    let now_a = "2026-10-30T08:00:00Z";

    // 第一批：一家成功、一家失败
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
    assert!(state::commit(dir.path(), &ids, &[ok_row, fail_row], now_a));
    let disk = state::load(dir.path());
    assert_eq!(disk.len(), 2);

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
        &ids,
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

    // 第三批：此前成功过的 p-ok 现在超时 → error 行继承旧明细与 lastOkAt
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
    assert!(state::commit(dir.path(), &ids, &[timeout_row], now_c));
    let disk = state::load(dir.path());
    let line = disk.iter().find(|r| r.provider_id == "p-ok").unwrap();
    assert_eq!(line.state, QuotaStatus::Error);
    assert_eq!(line.last_ok_at.as_deref(), Some(now_b), "继承上次成功时刻");
    assert_eq!(
        line.data.as_ref().unwrap().entries.len(),
        1,
        "离线兜底：失败行继承最近一次成功明细"
    );
}

#[tokio::test]
async fn commit_cleans_orphans_and_skips_write_when_nothing_changed() {
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
    // 先写入两行（p-orphan 与 p-kept）
    let mut orphan = row.clone();
    orphan.provider_id = "p-orphan".into();
    assert!(state::commit(
        dir.path(),
        &["p-kept".to_string(), "p-orphan".to_string()],
        &[row.clone(), orphan],
        now
    ));
    // 孤儿清理（ids 只剩 p-kept）→ 写盘
    assert!(state::commit(dir.path(), &kept_ids, &[], now));
    let disk = state::load(dir.path());
    assert_eq!(ids(&disk), vec!["p-kept"]);
    // 无请求结果且无孤儿 → 不写盘（返回 false）
    assert!(!state::commit(dir.path(), &kept_ids, &[], now));
    // 纯占位批次（no_key/unsupported）不产生 result_rows → 文件不被触碰
    assert!(!state::commit(dir.path(), &kept_ids, &[], now));
}

#[tokio::test]
async fn corrupted_file_is_quarantined_and_treated_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE_NAME), b"{broken json").unwrap();
    let rows = state::load(dir.path());
    assert!(rows.is_empty(), "损坏按空快照继续");
    assert!(
        dir.path().join(format!("{FILE_NAME}.corrupt")).exists(),
        "损坏文件备份留证"
    );
    assert!(!dir.path().join(FILE_NAME).exists(), "原文件已被移走");
}

#[test]
fn snapshot_row_survives_serde_roundtrip() {
    // quota.json 行格式的 roundtrip（camelCase wire 面）
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
    let back: QuotaSnapshot = serde_json::from_str(&text).unwrap();
    assert_eq!(back, row);
    // snake_case 行态值（§11.5 六态 wire 名）
    assert!(text.contains("\"state\":\"ok\""));
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
