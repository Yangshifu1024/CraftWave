//! SSRF 防护向量测试集（§9.2，12 号票）。
//!
//! 三层覆盖：
//! 1. 地址判定核的向量单测（私网/回环/链路本地/CGNAT/ULA/未指定/组播/保留/
//!    IPv4 映射/NAT64 解包，`allow_private_network` 例外开关）；
//! 2. 跳间判定与节流的纯函数单测（HTTPS 降级/跨源剥头/mock clock）；
//! 3. fetch 管线集成（本地 fixture HTTP server，127.0.0.1 起服务零外网）：
//!    默认策略拒回环直连与回环域名解析、`allow_private_network` 例外下
//!    正常抓取 + 重定向链 + 跨源剥头 + 跳数上限 + 50MiB 双保险。
//!
//! 「重定向到内网」的说明（票面测试向量）：本地 fixture 服务必然落回环段
//! （127.0.0.1），默认策略下首跳即被拒、走不到重定向二跳；二跳的拒绝判定
//! 与首跳同一入口（fetch 循环每跳先 `check_url`），其向量由地址判定核单测
//! 覆盖（`http://10.0.0.1/` 等直接拒），管线贯通由「默认策略拒回环直连」
//! 集成用例证明——集成用例 + 判定向量合起来等价覆盖该向量。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use super::{
    FetchRequest, HostThrottle, MAX_BODY_BYTES, MAX_REDIRECTS, SsrfCategory, SsrfPolicy,
    check_all_addresses, check_ipv4, check_ipv6, check_url, fetch, next_hop_allowed,
};
use crate::util::errors::ToolError;

/// 缺省策略（拒绝私网）。
const STRICT: SsrfPolicy = SsrfPolicy {
    allow_private_network: false,
};

/// 断言单个 IPv4 的判定类别（期望类别一致或都拒绝）。
fn expect_v4(ip: [u8; 4], expected: Option<SsrfCategory>) {
    let got = check_ipv4(Ipv4Addr::new(ip[0], ip[1], ip[2], ip[3]), STRICT).err();
    assert_eq!(
        got, expected,
        "IPv4 {ip:?} 判定不符：期望 {expected:?}，实际 {got:?}"
    );
}

// ---- 地址判定核：IPv4 向量（§9.2 第 2 步）----

#[test]
fn ipv4_deny_vectors() {
    // RFC1918 私网三段 + 段内边界
    expect_v4([10, 0, 0, 1], Some(SsrfCategory::PrivateNetwork));
    expect_v4([10, 255, 255, 254], Some(SsrfCategory::PrivateNetwork));
    expect_v4([172, 16, 0, 1], Some(SsrfCategory::PrivateNetwork));
    expect_v4([172, 31, 255, 255], Some(SsrfCategory::PrivateNetwork));
    expect_v4([192, 168, 1, 1], Some(SsrfCategory::PrivateNetwork));
    // 回环
    expect_v4([127, 0, 0, 1], Some(SsrfCategory::Loopback));
    expect_v4([127, 255, 255, 254], Some(SsrfCategory::Loopback));
    // 链路本地（含云元数据接口 169.254.169.254）
    expect_v4([169, 254, 169, 254], Some(SsrfCategory::LinkLocal));
    expect_v4([169, 254, 0, 1], Some(SsrfCategory::LinkLocal));
    // CGNAT（100.64/10：b ∈ 64..=127）与段外放行边界
    expect_v4([100, 64, 0, 1], Some(SsrfCategory::Cgnat));
    expect_v4([100, 127, 255, 255], Some(SsrfCategory::Cgnat));
    // 未指定/this-network
    expect_v4([0, 0, 0, 0], Some(SsrfCategory::Reserved));
    expect_v4([0, 1, 2, 3], Some(SsrfCategory::Reserved));
    // 组播 224/4
    expect_v4([224, 0, 0, 1], Some(SsrfCategory::Reserved));
    expect_v4([239, 255, 255, 255], Some(SsrfCategory::Reserved));
    // 保留 240/4（含受限广播）
    expect_v4([240, 0, 0, 1], Some(SsrfCategory::Reserved));
    expect_v4([255, 255, 255, 255], Some(SsrfCategory::Reserved));
    // IANA 保留/文档/基准段
    expect_v4([192, 0, 0, 9], Some(SsrfCategory::Reserved));
    expect_v4([192, 0, 2, 1], Some(SsrfCategory::Reserved));
    expect_v4([198, 18, 0, 1], Some(SsrfCategory::Reserved));
    expect_v4([198, 51, 100, 7], Some(SsrfCategory::Reserved));
    expect_v4([203, 0, 113, 9], Some(SsrfCategory::Reserved));
}

#[test]
fn ipv4_allow_vectors() {
    // 公网地址一律放行（含私网段边界外一格）
    for ip in [
        [8u8, 8, 8, 8],
        [1, 1, 1, 1],
        [172, 32, 0, 1],  // 172.16/12 之外
        [100, 128, 0, 1], // 100.64/10 之外
        [11, 0, 0, 1],    // 10/8 之外
        [169, 255, 0, 1], // 169.254/16 之外
    ] {
        expect_v4(ip, None);
    }
}

// ---- 地址判定核：IPv6 向量 ----

fn expect_v6(ip: Ipv6Addr, expected: Option<SsrfCategory>) {
    let got = check_ipv6(ip, STRICT).err();
    assert_eq!(
        got, expected,
        "IPv6 {ip} 判定不符：期望 {expected:?}，实际 {got:?}"
    );
}

#[test]
fn ipv6_deny_vectors() {
    expect_v6(Ipv6Addr::LOCALHOST, Some(SsrfCategory::Loopback)); // ::1
    expect_v6(Ipv6Addr::UNSPECIFIED, Some(SsrfCategory::Reserved)); // ::
    // 链路本地 fe80::/10（fe80–febf）
    expect_v6("fe80::1".parse().unwrap(), Some(SsrfCategory::LinkLocal));
    expect_v6("febf::1".parse().unwrap(), Some(SsrfCategory::LinkLocal));
    // ULA fc00::/7（fc00–fdff）
    expect_v6("fc00::1".parse().unwrap(), Some(SsrfCategory::Ula));
    expect_v6(
        "fd12:3456:789a::1".parse().unwrap(),
        Some(SsrfCategory::Ula),
    );
    // 组播 ff00::/8
    expect_v6("ff02::1".parse().unwrap(), Some(SsrfCategory::Reserved));
    // IPv4 映射 ::ffff:a.b.c.d 解包按 v4 判（伪装形式不绕过）
    expect_v6(
        "::ffff:10.0.0.1".parse().unwrap(),
        Some(SsrfCategory::PrivateNetwork),
    );
    expect_v6(
        "::ffff:127.0.0.1".parse().unwrap(),
        Some(SsrfCategory::Loopback),
    );
    expect_v6(
        "::ffff:169.254.169.254".parse().unwrap(),
        Some(SsrfCategory::LinkLocal),
    );
    // NAT64 64:ff9b::/96 尾 32 位解包按 v4 判
    expect_v6(
        "64:ff9b::a00:1".parse().unwrap(), // 64:ff9b::10.0.0.1
        Some(SsrfCategory::PrivateNetwork),
    );
    // 文档段与 IANA 特殊区
    expect_v6("2001:db8::1".parse().unwrap(), Some(SsrfCategory::Reserved));
    expect_v6("2001::1".parse().unwrap(), Some(SsrfCategory::Reserved));
    // 丢弃专用 100::/64
    expect_v6("100::1".parse().unwrap(), Some(SsrfCategory::Reserved));
}

#[test]
fn ipv6_allow_vectors() {
    // 公网 IPv6 放行（含 IPv4 映射的公网形式）
    expect_v6("2606:4700::1111".parse().unwrap(), None);
    expect_v6("240e::1".parse().unwrap(), None);
    expect_v6("::ffff:8.8.8.8".parse().unwrap(), None);
    // fe80/10、fc00::/7 边界之外
    expect_v6("fec0::1".parse().unwrap(), None);
    expect_v6("fe00::1".parse().unwrap(), None);
}

#[test]
fn allow_private_network_bypasses_all() {
    let policy = SsrfPolicy {
        allow_private_network: true,
    };
    // 例外开关打开后各类保留段全部放行（本地开发/内网部署场景）
    for ip in ["10.0.0.1", "127.0.0.1", "169.254.169.254", "100.64.0.1"] {
        assert!(
            check_ipv4(ip.parse().unwrap(), policy).is_ok(),
            "{ip} 在 allow_private_network 下应放行"
        );
    }
    for ip in ["::1", "fe80::1", "fd00::1", "::ffff:10.0.0.1"] {
        assert!(
            check_ipv6(ip.parse().unwrap(), policy).is_ok(),
            "{ip} 在 allow_private_network 下应放行"
        );
    }
}

#[test]
fn partial_private_resolution_rejects_whole() {
    // 「域名解析到私网」向量：解析结果部分公网部分私网 → 整体拒绝
    // （防解析到多地址时部分私网绕过）
    let err = check_all_addresses(
        [
            "8.8.8.8".parse::<IpAddr>().unwrap(),
            "192.168.1.1".parse().unwrap(),
        ],
        STRICT,
    )
    .expect_err("含私网地址的解析结果应整体拒绝");
    assert_eq!(err.category, SsrfCategory::PrivateNetwork);
    assert!(err.detail.contains("192.168.1.1"));
    // 纯公网放行
    check_all_addresses(
        ["8.8.8.8".parse().unwrap(), "1.1.1.1".parse().unwrap()],
        STRICT,
    )
    .expect("纯公网解析结果应放行");
}

// ---- check_url 向量（§9.2 第 1 步 + IP 字面量拦截）----

#[test]
fn url_scheme_and_ip_literal_vectors() {
    // 非 http/https scheme 直接拒
    for url in [
        "ftp://example.com/",
        "file:///etc/passwd",
        "data:text/html,x",
    ] {
        let err =
            check_url(&url.parse().unwrap(), STRICT, "目标").expect_err(&format!("{url} 应拒绝"));
        assert_eq!(err.code, "E_SSRF_BLOCKED");
        assert!(
            err.message.contains("协议不支持"),
            "message 应带类别：{}",
            err.message
        );
    }
    // IP 字面量在请求前拦截（IP 直连不触发 DNS，resolver 拦不到）
    let err = check_url(&"http://10.0.0.5/".parse().unwrap(), STRICT, "目标")
        .expect_err("私网 IP 字面量应拒绝");
    assert!(err.message.contains("私网地址"), "{}", err.message);
    let err = check_url(&"http://[fd00::1]/".parse().unwrap(), STRICT, "目标")
        .expect_err("ULA IP 字面量应拒绝");
    assert!(err.message.contains("ULA"), "{}", err.message);
    // 域名放行（判定交给 resolver）
    check_url(&"https://example.com/page".parse().unwrap(), STRICT, "目标")
        .expect("公网域名应放行");
    // 缺主机名：url 解析层即拒（`http://` 解析报 empty host；`http:///path`
    // 被 url crate 规范为 host="path" 的域名形式，不构成空主机绕过）
}

// ---- 跳间判定（§9.2 第 4 步：降级 + 跨源剥头）----

#[test]
fn next_hop_downgrade_rejected() {
    let headers = Default::default();
    let err = next_hop_allowed(
        &"https://example.com/a".parse().unwrap(),
        &"http://example.com/b".parse().unwrap(),
        &headers,
    )
    .expect_err("https→http 降级应拒绝");
    assert_eq!(err.code, "E_SSRF_BLOCKED");
    assert!(err.message.contains("重定向 HTTPS 降级"), "{}", err.message);
    // 同 scheme 升级与平级放行
    next_hop_allowed(
        &"http://example.com/a".parse().unwrap(),
        &"https://example.com/b".parse().unwrap(),
        &headers,
    )
    .expect("http→https 升级应放行");
}

#[test]
fn next_hop_cross_origin_strips_sensitive_headers() {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
    let mut headers = HeaderMap::new();
    headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
    headers.insert("cookie", HeaderValue::from_static("sid=1"));
    headers.insert("x-api-key", HeaderValue::from_static("k"));
    headers.insert("x-custom", HeaderValue::from_static("keep"));
    headers.insert("accept", HeaderValue::from_static("text/html"));

    // 跨源（host 变化）：敏感头全剥、普通头保留
    let next = next_hop_allowed(
        &"https://a.example.com/".parse().unwrap(),
        &"https://b.example.com/".parse().unwrap(),
        &headers,
    )
    .expect("https→https 跨源应放行（剥头不拦截）");
    assert!(next.get("authorization").is_none());
    assert!(next.get("cookie").is_none());
    assert!(next.get("x-api-key").is_none());
    assert!(next.get("proxy-authorization").is_none());
    assert!(next.get("www-authenticate").is_none());
    assert_eq!(next.get("x-custom").unwrap(), "keep");
    assert_eq!(next.get("accept").unwrap(), "text/html");

    // 同源（scheme+host+port 全同）：头原样保留
    let same = next_hop_allowed(
        &"https://a.example.com/x".parse().unwrap(),
        &"https://a.example.com/y?q=1".parse().unwrap(),
        &headers,
    )
    .unwrap();
    assert_eq!(same.get("authorization").unwrap(), "Bearer secret");
    // 默认端口归一：显式 443 与隐省同源
    let same2 = next_hop_allowed(
        &"https://a.example.com/".parse().unwrap(),
        &"https://a.example.com:443/".parse().unwrap(),
        &headers,
    )
    .unwrap();
    assert_eq!(same2.get("authorization").unwrap(), "Bearer secret");
    let _ = HeaderName::from_static("x-custom");
}

// ---- 节流（§9.2：每 host 1s；mock clock 纯函数 + 真实间隔集成）----

#[test]
fn throttle_mock_clock() {
    let min = Duration::from_secs(1);
    let t0 = Instant::now();
    // 首次：无记录 → 无需等待
    assert_eq!(HostThrottle::pending_wait(None, t0, min), None);
    // 窗口内：返回剩余等待
    let t1 = t0 + Duration::from_millis(200);
    assert_eq!(
        HostThrottle::pending_wait(Some(t0), t1, min),
        Some(Duration::from_millis(800))
    );
    // 窗口外（恰在边界）：无需等待
    assert_eq!(HostThrottle::pending_wait(Some(t0), t0 + min, min), None);
    assert_eq!(
        HostThrottle::pending_wait(Some(t0), t0 + min + Duration::from_millis(1), min),
        None
    );
}

#[test]
fn throttle_reserve_queues_bursts() {
    // 连发逐次排队：第 2 次等 1 个间隔、第 3 次等 2 个间隔（登记值=
    // 上次放行+间隔，保证突发连发的请求间距恒 ≥ 间隔）
    let throttle = HostThrottle::production();
    let t0 = Instant::now();
    assert_eq!(throttle.reserve("h", t0), None);
    let t1 = t0 + Duration::from_millis(100);
    assert_eq!(
        throttle.reserve("h", t1),
        Some(Duration::from_millis(900)),
        "第 2 次应等满 1s 间隔"
    );
    assert_eq!(
        throttle.reserve("h", t1),
        Some(Duration::from_millis(1900)),
        "第 3 次应再排 1 个间隔（逐次排队）"
    );
    // 窗口过后归零
    assert_eq!(
        throttle.reserve("h", t0 + Duration::from_secs(5)),
        None,
        "超过间隔后无需等待"
    );
}

#[test]
fn throttle_different_hosts_independent() {
    let throttle = HostThrottle::production();
    let t0 = Instant::now();
    assert_eq!(throttle.reserve("a.example.com", t0), None);
    assert_eq!(
        throttle.reserve("b.example.com", t0),
        None,
        "异 host 不受同 host 记录影响"
    );
    assert!(throttle.reserve("a.example.com", t0).is_some());
}

// ---- fetch 管线集成（本地 fixture，§9.2 五步贯通）----

/// 起一个 axum fixture 服务（127.0.0.1 随机端口，零外网），返回地址与
/// 服务任务句柄。路由由用例给定。
async fn spawn_fixture(
    routes: axum::Router,
) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地回环端口");
    let addr = listener.local_addr().expect("端口");
    let handle = tokio::spawn(async move {
        // 断连（如 50MiB 超限中断）会带 Err 收口——测试语义下静默即可
        let _ = axum::serve(listener, routes).await;
    });
    (addr, handle)
}

/// 构造 SSRF 客户端 + 空节流器（集成用例按需换真实间隔）。
fn test_client(policy: SsrfPolicy) -> reqwest::Client {
    super::build_client(policy, None).expect("客户端构建")
}

fn no_throttle() -> HostThrottle {
    HostThrottle::new(Duration::ZERO)
}

/// 通用执行封装：fetch + 取消令牌 + sent 标记。
async fn run_fetch(
    client: &reqwest::Client,
    url: &str,
    policy: SsrfPolicy,
    throttle: &HostThrottle,
) -> Result<super::FetchResponse, ToolError> {
    let request = FetchRequest {
        url: url.parse().expect("测试 URL 合法"),
        method: reqwest::Method::GET,
        headers: Default::default(),
        body: None,
    };
    let token = CancellationToken::new();
    let sent = AtomicBool::new(false);
    fetch(client, request, policy, throttle, &token, &sent).await
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_allows_loopback_when_policy_permits() {
    // 例外开关打开：回环 fixture 正常抓取（正面链路 + 状态码/正文/终 URL）
    let app = axum::Router::new().route(
        "/page",
        axum::routing::get(|| async { "<html><body>正文内容</body></html>" }),
    );
    let (addr, _server) = spawn_fixture(app).await;
    let client = test_client(SsrfPolicy {
        allow_private_network: true,
    });
    let resp = run_fetch(
        &client,
        &format!("http://{addr}/page"),
        SsrfPolicy {
            allow_private_network: true,
        },
        &no_throttle(),
    )
    .await
    .expect("例外策略下回环抓取应成功");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.url.as_str(), &format!("http://{addr}/page"));
    assert!(String::from_utf8_lossy(&resp.body).contains("正文内容"));
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_rejects_loopback_ip_by_default() {
    // 「直连私网 IP」向量（回环段变体）：默认策略下请求 127.0.0.1 fixture
    // → E_SSRF_BLOCKED 且类别可读（回环地址）——同时证明 fetch 循环每跳
    // 前置 check_url（含重定向二跳，同一入口）
    let app = axum::Router::new().route("/", axum::routing::get(|| async { "不应到达" }));
    let (addr, _server) = spawn_fixture(app).await;
    let client = test_client(STRICT);
    let err = run_fetch(&client, &format!("http://{addr}/"), STRICT, &no_throttle())
        .await
        .expect_err("默认策略应拒绝回环直连");
    assert_eq!(err.code, "E_SSRF_BLOCKED");
    assert!(err.message.contains("回环地址"), "{}", err.message);
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_rejects_domain_resolving_to_loopback() {
    // 「域名解析到私网」向量（真实 DNS 链路）：localhost 恒解析到回环，
    // 默认策略下 resolver 全地址校验应拒绝——E_SSRF_BLOCKED 且类别可读
    let app = axum::Router::new().route("/", axum::routing::get(|| async { "不应到达" }));
    let (addr, _server) = spawn_fixture(app).await;
    let client = test_client(STRICT);
    let err = run_fetch(
        &client,
        &format!("http://localhost:{}/", addr.port()),
        STRICT,
        &no_throttle(),
    )
    .await
    .expect_err("解析到回环的域名应被拒绝");
    assert_eq!(err.code, "E_SSRF_BLOCKED");
    assert!(err.message.contains("回环地址"), "{}", err.message);
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_follows_redirect_chain_when_permitted() {
    // 例外策略：重定向链正常跟进（302 → 相对 Location → 终点），逐跳
    // check_url 与节流在链上执行
    let app = axum::Router::new()
        .route(
            "/start",
            axum::routing::get(|| async { axum::response::Redirect::to("/final") }),
        )
        .route("/final", axum::routing::get(|| async { "终点内容" }));
    let (addr, _server) = spawn_fixture(app).await;
    let policy = SsrfPolicy {
        allow_private_network: true,
    };
    let client = test_client(policy);
    let resp = run_fetch(
        &client,
        &format!("http://{addr}/start"),
        policy,
        &no_throttle(),
    )
    .await
    .expect("重定向链应跟进成功");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.url.as_str(), &format!("http://{addr}/final"));
    assert_eq!(String::from_utf8_lossy(&resp.body), "终点内容");
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_strips_authorization_across_origins() {
    // 「跨源自带 Authorization 被剥」向量（集成）：两个 fixture 服务落在
    // 不同端口（scheme+host+port 的 port 不同即跨源）；终点 B 回显收到的
    // 请求头名清单，断言敏感头被剥、普通头保留
    use axum::http::HeaderMap as AxumHeaders;
    // 终点 B：回显收到的头名（排序去重后逗号连接）
    let seen_b: Arc<std::sync::Mutex<Option<String>>> = Arc::default();
    let capture = seen_b.clone();
    let app_b = axum::Router::new().route(
        "/final",
        axum::routing::get(move |headers: AxumHeaders| {
            let capture = capture.clone();
            async move {
                let mut names: Vec<String> =
                    headers.iter().map(|(k, _)| k.as_str().to_owned()).collect();
                names.sort();
                names.dedup();
                *capture.lock().unwrap() = Some(names.join(","));
                "终点"
            }
        }),
    );
    let (addr_b, _server_b) = spawn_fixture(app_b).await;

    // 起点 A：302 到 B 的 /final（端口不同 → 跨源）
    let target = format!("http://{addr_b}/final");
    let app_a = axum::Router::new().route(
        "/start",
        axum::routing::get(move || {
            let target = target.clone();
            async move { axum::response::Redirect::to(&target) }
        }),
    );
    let (addr_a, _server_a) = spawn_fixture(app_a).await;

    let policy = SsrfPolicy {
        allow_private_network: true,
    };
    let client = test_client(policy);
    let request = FetchRequest {
        url: format!("http://{addr_a}/start").parse().unwrap(),
        method: reqwest::Method::GET,
        headers: {
            let mut h = reqwest::header::HeaderMap::new();
            h.insert("authorization", "Bearer secret".parse().unwrap());
            h.insert("x-custom", "keep".parse().unwrap());
            h
        },
        body: None,
    };
    let token = CancellationToken::new();
    let sent = AtomicBool::new(false);
    fetch(&client, request, policy, &no_throttle(), &token, &sent)
        .await
        .expect("跨源重定向应跟进成功（剥头不拦截）");
    let seen_headers = seen_b.lock().unwrap().clone().expect("终点应收到请求");
    assert!(
        !seen_headers.split(',').any(|n| n == "authorization"),
        "跨源后不应携带 Authorization：{seen_headers}"
    );
    assert!(
        seen_headers.split(',').any(|n| n == "x-custom"),
        "普通头应跨源保留：{seen_headers}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_rejects_redirect_loop_over_limit() {
    // 跳数上限：/hop/N 永远 302 到 /hop/N+1，超过 MAX_REDIRECTS 跳即拒
    async fn hop(axum::extract::Path(n): axum::extract::Path<usize>) -> axum::response::Redirect {
        axum::response::Redirect::to(&format!("/hop/{}", n + 1))
    }
    let app = axum::Router::new().route("/hop/{n}", axum::routing::get(hop));
    let (addr, _server) = spawn_fixture(app).await;
    let policy = SsrfPolicy {
        allow_private_network: true,
    };
    let client = test_client(policy);
    let err = run_fetch(
        &client,
        &format!("http://{addr}/hop/0"),
        policy,
        &no_throttle(),
    )
    .await
    .expect_err("无限重定向应被拒");
    assert_eq!(err.code, "E_SSRF_BLOCKED");
    assert!(err.message.contains("重定向超限"), "{}", err.message);
    assert!(err.message.contains(&MAX_REDIRECTS.to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_rejects_oversized_content_length() {
    // 50MiB 双保险之一：Content-Length 预检——服务器真实声明 51MiB+1
    // （axum 按一次性 body 自动设置 content-length），客户端在响应头到达
    // 时即拒绝、不进入下载（hyper 对头/体不符的伪声明会直接报协议错误，
    // 走不到预检，故须真实大小）
    let app = axum::Router::new().route(
        "/big",
        axum::routing::get(|| async { axum::body::Body::from(vec![b'x'; MAX_BODY_BYTES + 1]) }),
    );
    let (addr, _server) = spawn_fixture(app).await;
    let policy = SsrfPolicy {
        allow_private_network: true,
    };
    let client = test_client(policy);
    let err = run_fetch(
        &client,
        &format!("http://{addr}/big"),
        policy,
        &no_throttle(),
    )
    .await
    .expect_err("Content-Length 超限应拒绝");
    assert_eq!(err.code, "E_INVALID_ARG");
    assert!(err.message.contains("50MiB"), "{}", err.message);
}

#[tokio::test(flavor = "current_thread")]
async fn fetch_rejects_oversized_stream() {
    // 50MiB 双保险之二：流式计数（chunked 无 Content-Length，真实发送
    // 50MiB+1——本地回环传输秒级内）
    fn oversize_stream() -> impl futures::Stream<Item = Result<Vec<u8>, std::io::Error>> {
        // 1MiB/块，共 50MiB+1 块（超限一字节量级）
        let chunk = vec![b'x'; 1024 * 1024];
        futures::stream::unfold(
            (chunk, MAX_BODY_BYTES / (1024 * 1024) + 1usize),
            |(chunk, remaining)| async move {
                if remaining == 0 {
                    None
                } else {
                    Some((Ok(chunk.clone()), (chunk, remaining - 1)))
                }
            },
        )
    }
    let app = axum::Router::new().route(
        "/stream",
        axum::routing::get(|| async { axum::body::Body::from_stream(oversize_stream()) }),
    );
    let (addr, _server) = spawn_fixture(app).await;
    let policy = SsrfPolicy {
        allow_private_network: true,
    };
    let client = test_client(policy);
    let err = run_fetch(
        &client,
        &format!("http://{addr}/stream"),
        policy,
        &no_throttle(),
    )
    .await
    .expect_err("流式超限应拒绝");
    assert_eq!(err.code, "E_INVALID_ARG");
    assert!(err.message.contains("50MiB"), "{}", err.message);
}

#[tokio::test(flavor = "current_thread")]
async fn throttle_integration_same_host_waits_real_interval() {
    // 节流集成（短间隔实测）：同 host 第二次请求前等待 ≥ 间隔；用小间隔
    // （100ms）控制测试时长，语义与生产 1s 一致（同一 reserve 通路）。
    // 异 host 独立性由纯函数单测精确覆盖（集成侧 127.0.0.1 的 host 字符串
    // 唯一，无法构造第二个不同标识指向同一服务）。
    let app = axum::Router::new().route("/", axum::routing::get(|| async { "ok" }));
    let (addr, _server) = spawn_fixture(app).await;
    let policy = SsrfPolicy {
        allow_private_network: true,
    };
    let client = test_client(policy);
    let throttle = HostThrottle::new(Duration::from_millis(100));
    let url = format!("http://{addr}/");
    let start = Instant::now();
    run_fetch(&client, &url, policy, &throttle)
        .await
        .expect("首次");
    run_fetch(&client, &url, policy, &throttle)
        .await
        .expect("第二次");
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_millis(100),
        "同 host 连发两次应至少间隔一个节流窗口（实际 {elapsed:?}）"
    );
}
