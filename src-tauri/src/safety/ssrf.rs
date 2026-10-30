//! SSRF 防护共享层（技术设计文档 §9.2 五步防线，12 号票实装；13 号票
//! web_search 复用本模块的校验与抓取原语）。
//!
//! 名词解释：SSRF（Server-Side Request Forgery，服务端请求伪造）指模型诱导
//! 宿主应用去请求内网/本机地址（如 `http://169.254.169.254/` 云元数据接口、
//! `http://127.0.0.1:6379/` 本机服务），借宿主的网络位置攻击内网。防线思路：
//! 「URL 说了什么」不可信，「实际连接到哪个 IP」才可信——所以校验发生在
//! DNS 解析之后（对解析出的全部 IP 逐一判定），且解析与连接用同一份已校验
//! 地址（钉址），不给「校验时看 A 地址、连接时换 B 地址」的空子（DNS
//! rebinding，指攻击者控制域名让两次解析返回不同地址）。
//!
//! 五步防线（§9.2，逐条对应 [`check_url`] / [`SsrfResolver`] / 手动重定向
//! 循环 / [`MAX_BODY_BYTES`]）：
//! 1. URL 预校验：仅 http/https；host 为 IP 字面量时立即做地址校验
//!    （IP 直连不经过 DNS，resolver 拦不到，必须请求前拦）；
//! 2. DNS 解析 + 全地址校验：[`SsrfResolver`] 在每次解析时对全部解析结果
//!    逐一判定，任一命中保留段即整体拒绝；
//! 3. 连接前钉址（防 rebinding）：reqwest 的连接地址就来自 resolver 的返回
//!    值——校验与连接同源，无二次解析窗口；
//! 4. 重定向逐跳重校验：本层关闭 reqwest 自动重定向（`Policy::none`），由
//!    [`fetch`] 手动循环——每跳 URL 重跑第 1 步（新 host 触发 resolver 重跑
//!    第 2/3 步）、HTTPS 不降级、跨源剥敏感头、≤10 跳；
//! 5. 正文 ≤50MiB：Content-Length 预检 + 流式计数双保险。
//!
//! 另有每 host 1s 节流（[`HostThrottle`]，fetch 循环内执行，IP 直连也覆盖）；
//! SSRF 拒绝统一错误码 `E_SSRF_BLOCKED`（§3.6，message 带命中类别）。
//!
//! 代理接线收敛点（§9.2 注记）：HTTP 客户端按 `AppConfig.proxy` 构建；19 号
//! 票 client 重建路径落地前按 config.proxy 直建（每工具调用现建，无全局缓存）。

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use url::Url;

use crate::providers::types::ProxyConfig;
use crate::util::errors::ToolError;

/// 正文大小上限：50MiB（§8.3 web_fetch 行 / §9.2 第 5 步）。
pub const MAX_BODY_BYTES: usize = 50 * 1024 * 1024;

/// 重定向跳数上限（§9.2 第 4 步：≤10 跳）。
pub const MAX_REDIRECTS: usize = 10;

/// 每 host 最小访问间隔（§9.2：每 host 1s 节流）。
pub const HOST_THROTTLE_INTERVAL: Duration = Duration::from_secs(1);

/// 连接超时（防御性兜底：不给 run 留无限挂死的可能；非票面字段，
/// 决议记录说明）。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// 单请求总超时（含 body 下载；同上防御性兜底）。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

// ---- SSRF 拒绝类别（§3.6：message 带命中类别）----

/// SSRF 拒绝类别：错误码统一 `E_SSRF_BLOCKED`，类别由本枚举区分，
/// [`SsrfCategory::label`] 给出中文类别名（进错误 message）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsrfCategory {
    /// URL 协议不是 http/https。
    SchemeUnsupported,
    /// RFC1918 私网段（10/8、172.16/12、192.168/16）。
    PrivateNetwork,
    /// 回环地址（127/8、::1）。
    Loopback,
    /// 链路本地（169.254/16、fe80::/10）——含云厂商元数据服务段。
    LinkLocal,
    /// 运营商级 NAT 保留段（100.64/10）。
    Cgnat,
    /// IPv6 唯一本地地址（fc00::/7）。
    Ula,
    /// 未指定/组播/IANA 保留/文档等其余不可路由段。
    Reserved,
    /// 重定向超过 10 跳。
    RedirectLoop,
    /// 重定向从 https 降到 http（明文降级）。
    HttpsDowngrade,
}

// SsrfCategory 的中文类别名（错误 message 用，§3.6「错误类别可读」）。
impl SsrfCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::SchemeUnsupported => "协议不支持",
            Self::PrivateNetwork => "私网地址",
            Self::Loopback => "回环地址",
            Self::LinkLocal => "链路本地地址",
            Self::Cgnat => "CGNAT 保留段",
            Self::Ula => "IPv6 ULA 地址",
            Self::Reserved => "保留地址段",
            Self::RedirectLoop => "重定向超限",
            Self::HttpsDowngrade => "重定向 HTTPS 降级",
        }
    }
}

// Display：错误格式化串里可直接 `{category}`（等价 label）。
impl fmt::Display for SsrfCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// SSRF 拒绝错误（自定义类型：reqwest 会把 resolver 的错误包进 source 链，
/// [`classify_reqwest_error`] 沿链找回本类型还原类别）。
#[derive(Debug)]
pub struct SsrfError {
    pub category: SsrfCategory,
    pub detail: String,
}

// SsrfError 的构造与错误 trait 实现（供 reqwest 包装与 downcast 找回）。
impl SsrfError {
    pub fn new(category: SsrfCategory, detail: impl Into<String>) -> Self {
        Self {
            category,
            detail: detail.into(),
        }
    }
}

// Display / Error：reqwest 错误链要求元素实现 std::error::Error。
impl fmt::Display for SsrfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}：{}", self.category.label(), self.detail)
    }
}

impl Error for SsrfError {}

// ---- 地址判定（§9.2 第 2 步「全地址校验」的判定核）----

/// SSRF 策略：`allow_private_network` 为 §3.2 network 组的设置例外——
/// true 时跳过全部地址段判定（本地开发/内网部署场景；缺省 false 最保守）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SsrfPolicy {
    /// 允许访问私网/回环/链路本地/CGNAT/ULA 地址（§3.2 network.allowPrivateNetwork）。
    pub allow_private_network: bool,
}

/// 判定单个 IPv4 地址是否在拒绝段。返回 `Ok(())` 放行，`Err(类别)` 拒绝。
/// 判定按段手工枚举（不依赖 `IpAddr::is_global`——标准库的 global 语义随
/// RFC 更新变动，安全边界要显式可控、可逐向量测试）。
pub fn check_ipv4(ip: Ipv4Addr, policy: SsrfPolicy) -> Result<(), SsrfCategory> {
    if policy.allow_private_network {
        return Ok(());
    }
    let [a, b, c, _d] = ip.octets();
    let category = if a == 10 {
        // 10.0.0.0/8 私网
        SsrfCategory::PrivateNetwork
    } else if a == 172 && (b & 0xf0) == 16 {
        // 172.16.0.0/12 私网（172.16–172.31）
        SsrfCategory::PrivateNetwork
    } else if a == 192 && b == 168 {
        // 192.168.0.0/16 私网
        SsrfCategory::PrivateNetwork
    } else if a == 127 {
        // 127.0.0.0/8 回环
        SsrfCategory::Loopback
    } else if a == 169 && b == 254 {
        // 169.254.0.0/16 链路本地（含 AWS/GCP 元数据 169.254.169.254）
        SsrfCategory::LinkLocal
    } else if a == 100 && (b & 0xc0) == 64 {
        // 100.64.0.0/10 CGNAT（b ∈ 64..=127）
        SsrfCategory::Cgnat
    } else if a == 0 {
        // 0.0.0.0/8 "this network"（含未指定地址 0.0.0.0）
        SsrfCategory::Reserved
    } else if (224..=239).contains(&a) {
        // 224.0.0.0/4 组播
        SsrfCategory::Reserved
    } else if a >= 240 {
        // 240.0.0.0/4 保留（含受限广播 255.255.255.255）
        SsrfCategory::Reserved
    } else if a == 192 && b == 0 && c == 0 {
        // 192.0.0.0/24 IETF 协议分配
        SsrfCategory::Reserved
    } else if a == 192 && b == 0 && c == 2 {
        // 192.0.2.0/24 TEST-NET-1 文档段
        SsrfCategory::Reserved
    } else if a == 198 && (b & 0xfe) == 18 {
        // 198.18.0.0/15 基准测试保留段
        SsrfCategory::Reserved
    } else if a == 198 && b == 51 && c == 100 {
        // 198.51.100.0/24 TEST-NET-2 文档段
        SsrfCategory::Reserved
    } else if a == 203 && b == 0 && c == 113 {
        // 203.0.113.0/24 TEST-NET-3 文档段
        SsrfCategory::Reserved
    } else {
        return Ok(());
    };
    Err(category)
}

/// 判定单个 IPv6 地址。IPv4 映射（::ffff:0:0/96）与 NAT64 封装
/// （64:ff9b::/96）解包出内嵌 IPv4 后按 IPv4 规则判——否则
/// `::ffff:10.0.0.1` 这类伪装形式会绕过 IPv4 段判定。
pub fn check_ipv6(ip: Ipv6Addr, policy: SsrfPolicy) -> Result<(), SsrfCategory> {
    // IPv4 映射地址 ::ffff:a.b.c.d → 解包按 IPv4 判定
    if let Some(v4) = ip.to_ipv4_mapped() {
        return check_ipv4(v4, policy);
    }
    let seg = ip.segments();
    // NAT64 有界前缀 64:ff9b::/96（RFC 6052）：末 32 位为内嵌 IPv4
    if seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2] == 0 && seg[3] == 0 && seg[4] == 0 && seg[5] == 0
    {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            (seg[6] & 0xff) as u8,
            (seg[7] >> 8) as u8,
            (seg[7] & 0xff) as u8,
        );
        return check_ipv4(v4, policy);
    }
    if policy.allow_private_network {
        return Ok(());
    }
    let category = if ip.is_loopback() {
        // ::1 回环
        SsrfCategory::Loopback
    } else if ip.is_unspecified() {
        // :: 未指定地址
        SsrfCategory::Reserved
    } else if (seg[0] & 0xffc0) == 0xfe80 {
        // fe80::/10 链路本地
        SsrfCategory::LinkLocal
    } else if (seg[0] & 0xfe00) == 0xfc00 {
        // fc00::/7 ULA（唯一本地地址）
        SsrfCategory::Ula
    } else if (seg[0] & 0xff00) == 0xff00 {
        // ff00::/8 组播
        SsrfCategory::Reserved
    } else if seg[0] == 0 && seg[1] == 0 && seg[2] == 0 && seg[3] == 0 && seg[4] == 0 && seg[5] == 0
    {
        // ::/96 IPv4 兼容地址（已废弃仍保守拒绝；::1/:: 已在上方命中）
        SsrfCategory::Reserved
    } else if seg[0] == 0x0100 && seg[1] == 0 {
        // 100::/64 丢弃专用（discard-only）
        SsrfCategory::Reserved
    } else if seg[0] == 0x2001 && seg[1] == 0x0db8 {
        // 2001:db8::/32 文档段
        SsrfCategory::Reserved
    } else if seg[0] == 0x2001 && (seg[1] & 0xfff0) == 0 {
        // 2001::/23 IANA 特殊分配区（teredo/orchid 等保留段）
        SsrfCategory::Reserved
    } else if (seg[0] & 0xfe00) == 0x0200 {
        // 200::/7 OSI NSAP 与 IPX 保留分配
        SsrfCategory::Reserved
    } else {
        return Ok(());
    };
    Err(category)
}

/// 判定单个 IP 地址（v4/v6 分发）。
pub fn check_ip(ip: IpAddr, policy: SsrfPolicy) -> Result<(), SsrfCategory> {
    match ip {
        IpAddr::V4(v4) => check_ipv4(v4, policy),
        IpAddr::V6(v6) => check_ipv6(v6, policy),
    }
}

/// URL 预校验（§9.2 第 1 步 + IP 字面量拦截）：仅 http/https；host 为
/// IP 字面量时立即做地址判定（IP 直连不触发 DNS，resolver 拦不到）。
/// `detail` 供错误 message 拼装上下文（如「重定向目标」）。
pub fn check_url(url: &Url, policy: SsrfPolicy, context: &str) -> Result<(), ToolError> {
    let blocked = |category: SsrfCategory, detail: String| {
        ToolError::ssrf_blocked(format!("SSRF 防护拦截（{category}）：{context}{detail}"))
    };
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(blocked(
            SsrfCategory::SchemeUnsupported,
            format!("仅允许 http/https（实际 {}）", url.scheme()),
        ));
    }
    match url.host() {
        Some(url::Host::Domain(host)) => {
            // 域名：交给 resolver 在解析后校验（此处仅做非空防御）
            if host.is_empty() {
                return Err(blocked(SsrfCategory::SchemeUnsupported, "URL 缺少主机名".into()));
            }
            Ok(())
        }
        Some(url::Host::Ipv4(ip)) => match check_ipv4(ip, policy) {
            Ok(()) => Ok(()),
            Err(cat) => Err(blocked(cat, format!("{ip}"))),
        },
        Some(url::Host::Ipv6(ip)) => match check_ipv6(ip, policy) {
            Ok(()) => Ok(()),
            Err(cat) => Err(blocked(cat, format!("[{ip}]"))),
        },
        None => Err(blocked(SsrfCategory::SchemeUnsupported, "URL 缺少主机名".into())),
    }
}

/// 批量地址校验（域名解析结果的全部地址逐一判定，任一命中即拒）。
/// 独立成函数供测试与 13 号票复用（不依赖 reqwest）。
pub fn check_all_addresses(
    addrs: impl IntoIterator<Item = IpAddr>,
    policy: SsrfPolicy,
) -> Result<(), SsrfError> {
    for ip in addrs {
        check_ip(ip, policy)
            .map_err(|category| SsrfError::new(category, format!("解析结果包含 {ip}")))?;
    }
    Ok(())
}

// ---- 每 host 节流（§9.2：每 host 1s）----

/// 每 host 最小访问间隔的进程级节流器：`Mutex<HashMap<host, Instant>>` 起步
/// （票面定案：够用不引库）。`reserve` 返回需要等待的时长（None = 立即可发），
/// 等待动作由调用方执行——纯判定与等待分离，节流逻辑可用假时钟单测
/// （票面测试要求「mock clock」）。
///
/// 连续调用逐次排队：第 2 次等 1 个间隔、第 3 次等 2 个间隔……登记值为
/// 「上次放行时间 + 间隔」而非当前时刻，保证突发连发时请求间距恒 ≥ 间隔。
pub struct HostThrottle {
    min_interval: Duration,
    last: Mutex<HashMap<String, Instant>>,
}

// HostThrottle 的构造与登记。
impl HostThrottle {
    /// 以最小间隔构造（生产用 [`Self::production`]；测试传小间隔实测）。
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last: Mutex::new(HashMap::new()),
        }
    }

    /// 生产节流器：每 host 1s（§9.2）。
    pub fn production() -> Self {
        Self::new(HOST_THROTTLE_INTERVAL)
    }

    /// 计算需等待的时长（纯函数）：`last` 为该 host 的已预约/已放行时刻。
    /// None = 无需等待。独立暴露供 mock clock 单测。
    ///
    /// `last` 允许超前于 `now`（连发场景：前次调用已把预约点推到
    /// 「上次放行 + 间隔」）——此时等待 = 预约点距现在的时长 + 一个间隔，
    /// 保证突发连发的实际发出间距恒 ≥ 间隔（逐次排队）。
    pub fn pending_wait(
        last: Option<Instant>,
        now: Instant,
        min_interval: Duration,
    ) -> Option<Duration> {
        let Some(last) = last else {
            return None;
        };
        // 已流逝时长（last 超前 now 时饱和为 0）与超前时长（反向同理）
        let elapsed = now.checked_duration_since(last).unwrap_or_default();
        let ahead = last.checked_duration_since(now).unwrap_or_default();
        if elapsed < min_interval {
            Some(min_interval - elapsed + ahead)
        } else {
            None
        }
    }

    /// 登记一次对 `host` 的访问意图，返回需要等待的时长（None = 立即可发）。
    /// 等待结束后实际放行时间点 = 上次放行 + 间隔（连发逐次排队）。
    pub fn reserve(&self, host: &str, now: Instant) -> Option<Duration> {
        let mut map = self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let last = map.get(host).copied();
        let wait = Self::pending_wait(last, now, self.min_interval);
        let booked = match (last, wait) {
            // 已在等待窗口内：登记为「上次放行 + 间隔」（逐次排队）
            (Some(last), Some(_)) => last + self.min_interval,
            // 窗口外或首次：登记当前时刻
            _ => now,
        };
        map.insert(host.to_owned(), booked);
        wait
    }
}

// ---- DNS 解析器（§9.2 第 2/3 步：全地址校验 + 钉址）----

/// SSRF 校验型 DNS 解析器：替换 reqwest 的默认解析器，每次解析先做全地址
/// 校验，只把已校验地址交给连接层——校验与连接同源即「钉址」（防 DNS
/// rebinding，§9.2 第 3 步）。域名每跳重定向自动重走本 resolver。
pub struct SsrfResolver {
    policy: SsrfPolicy,
}

// SsrfResolver 的构造。
impl SsrfResolver {
    pub fn new(policy: SsrfPolicy) -> Self {
        Self { policy }
    }
}

// 实现 reqwest 的 Resolve trait：reqwest 连接前经此解析。
impl Resolve for SsrfResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = self.policy;
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn Error + Send + Sync> {
                    Box::new(SsrfError::new(
                        SsrfCategory::Reserved,
                        format!("域名解析失败：{host}（{e}）"),
                    ))
                })?
                .collect();
            check_all_addresses(addrs.iter().map(|a| a.ip()), policy)
                .map_err(|e| -> Box<dyn Error + Send + Sync> { Box::new(e) })?;
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

// ---- HTTP 客户端构建（代理接线收敛点，§9.2 注记）----

/// 按代理配置构建 SSRF 客户端：resolver 钉址 + 关自动重定向（fetch 手动
/// 循环）+ 超时兜底 + UA。`proxy` 为 `AppConfig.proxy`（None = 未配置）；
/// 19 号票 client 重建路径落地前按配置直建（§9.2 收敛点注记）。
pub fn build_client(
    policy: SsrfPolicy,
    proxy: Option<&ProxyConfig>,
) -> Result<reqwest::Client, ToolError> {
    let mut builder = reqwest::Client::builder()
        .dns_resolver(SsrfResolver::new(policy))
        // 重定向由 fetch 手动循环逐跳校验（§9.2 第 4 步），关闭自动重定向
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("CraftWave/", env!("CARGO_PKG_VERSION")));
    builder = match proxy {
        // none / 未配置：显式禁用代理（不探测系统环境变量）
        None | Some(ProxyConfig { mode: crate::providers::types::ProxyMode::None, .. }) => {
            builder.no_proxy()
        }
        // system：跟随系统代理设置（reqwest 默认行为；system 探测按 19 号票
        // 收敛点完善，当前缺省直连不误伤）
        Some(ProxyConfig { mode: crate::providers::types::ProxyMode::System, .. }) => builder,
        // manual：使用配置的代理地址
        Some(ProxyConfig {
            mode: crate::providers::types::ProxyMode::Manual,
            url: Some(url),
            ..
        }) => {
            let proxy = reqwest::Proxy::all(url)
                .map_err(|e| ToolError::invalid_arg(format!("代理地址非法（{url}）：{e}")))?;
            builder.proxy(proxy)
        }
        Some(ProxyConfig { mode: crate::providers::types::ProxyMode::Manual, url: None, .. }) => {
            return Err(ToolError::invalid_arg(
                "代理模式为 manual 但未配置代理地址（proxy.url）",
            ));
        }
    };
    builder
        .build()
        .map_err(|e| ToolError::new("E_INTERNAL", format!("HTTP 客户端构建失败：{e}")))
}

// ---- 手动重定向的抓取循环（§9.2 第 4/5 步）----

/// 跨源时需要剥除的敏感头（§9.2 第 4 步：Authorization/Cookie 等不跨源携带；
/// 保险起见对 http_request 用户自定义头同样生效——凭据只属于发源站点）。
const SENSITIVE_HEADERS: [&str; 5] = [
    "authorization",
    "cookie",
    "proxy-authorization",
    "x-api-key",
    "www-authenticate",
];

/// 重定向跳间判定（§9.2 第 4 步，纯函数供向量单测）：HTTPS 不降级
/// （https→http 拒）；跨源（scheme://host:port 变化）剥敏感头。
/// 返回剥头后的请求头（同源原样返回语义等价）。
pub fn next_hop_allowed(
    prev: &Url,
    next: &Url,
    headers: &reqwest::header::HeaderMap,
) -> Result<reqwest::header::HeaderMap, ToolError> {
    if prev.scheme() == "https" && next.scheme() != "https" {
        return Err(ToolError::ssrf_blocked(format!(
            "SSRF 防护拦截（{}）：{prev} → {next}",
            SsrfCategory::HttpsDowngrade.label()
        )));
    }
    let mut headers = headers.clone();
    if origin_of(prev) != origin_of(next) {
        for name in SENSITIVE_HEADERS {
            if let Ok(h) = reqwest::header::HeaderName::from_bytes(name.as_bytes()) {
                headers.remove(&h);
            }
        }
    }
    Ok(headers)
}

/// 一次抓取请求（首跳；重定向跳由 fetch 内部构造）。
#[derive(Debug)]
pub struct FetchRequest {
    /// 目标 URL（首跳）。
    pub url: Url,
    /// HTTP 方法。
    pub method: reqwest::Method,
    /// 请求头（跨源跳自动剥敏感头）。
    pub headers: reqwest::header::HeaderMap,
    /// 请求体（仅 http_request 可能携带；GET/HEAD 恒 None）。
    pub body: Option<Vec<u8>>,
}

/// 抓取结果（终跳响应）。
#[derive(Debug)]
pub struct FetchResponse {
    /// 最终 URL（重定向后的终点，回喂与 Readability 的 document_url 用）。
    pub url: Url,
    /// 状态码。
    pub status: u16,
    /// 响应头（小写名）。
    pub headers: reqwest::header::HeaderMap,
    /// 响应体（≤50MiB）。
    pub body: Vec<u8>,
}

/// 抓取一次（含重定向逐跳校验与节流）。`sent` 由本函数置位：响应头已收到
/// 即 true——取消时据此判定「请求可能已到达对端」（http_request 的副作用
/// 语义；连接阶段被取消则保持 false）。
pub async fn fetch(
    client: &reqwest::Client,
    request: FetchRequest,
    policy: SsrfPolicy,
    throttle: &HostThrottle,
    token: &tokio_util::sync::CancellationToken,
    sent: &std::sync::atomic::AtomicBool,
) -> Result<FetchResponse, ToolError> {
    let mut current = request;
    for hops in 0..=MAX_REDIRECTS {
        let url = current.url.clone();
        // 第 1 步：scheme + IP 字面量校验（每跳都做）
        check_url(&url, policy, if hops == 0 { "目标" } else { "重定向目标" })?;
        // 节流（每跳按其 host）
        if let Some(wait) = throttle.reserve(url.host_str().unwrap_or(""), Instant::now()) {
            tokio::select! {
                biased;
                _ = token.cancelled() => {
                    return Err(ToolError::new("E_CANCELLED", "抓取在节流等待期间被取消"));
                }
                _ = tokio::time::sleep(wait) => {}
            }
        }
        // 构建本跳请求
        let mut req = client.request(current.method.clone(), url.clone());
        req = req.headers(current.headers.clone());
        if let Some(body) = &current.body {
            req = req.body(body.clone());
        }
        let req = req.build().map_err(|e| ToolError::new("E_INVALID_ARG", format!("请求构造失败：{e}")))?;
        // 响应头已收到即视为「请求已发出」（取消语义的 executed 判定面）
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                return Err(ToolError::new("E_CANCELLED", "抓取被取消"));
            }
            resp = client.execute(req) => {
                let mut resp = resp.map_err(|e| classify_reqwest_error(&e))?;
                sent.store(true, std::sync::atomic::Ordering::Relaxed);
                let status = resp.status().as_u16();
                // 第 4 步：3xx → 解析 Location 进入下一跳
                if matches!(status, 301 | 302 | 303 | 307 | 308) {
                    if hops == MAX_REDIRECTS {
                        return Err(ToolError::ssrf_blocked(format!(
                            "SSRF 防护拦截（{}）：重定向超过 {MAX_REDIRECTS} 跳",
                            SsrfCategory::RedirectLoop.label()
                        )));
                    }
                    let location = resp
                        .headers()
                        .get(reqwest::header::LOCATION)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned)
                        .ok_or_else(|| {
                            ToolError::new("E_INTERNAL", format!("重定向 {status} 缺少 Location 头"))
                        })?;
                    let next = url.join(&location).map_err(|e| {
                        ToolError::new("E_INTERNAL", format!("Location 解析失败（{location}）：{e}"))
                    })?;
                    // 跳间判定（§9.2 第 4 步）：HTTPS 不降级 + 跨源剥敏感头
                    // （纯函数抽离，向量单测覆盖降级与剥头）
                    current.headers = next_hop_allowed(&url, &next, &current.headers)?;
                    // 浏览器语义：301/302/303 对非 GET/HEAD 转 GET 并剥 body；
                    // 307/308 保持方法与 body
                    if matches!(status, 301 | 302 | 303)
                        && current.method != reqwest::Method::GET
                        && current.method != reqwest::Method::HEAD
                    {
                        current.method = reqwest::Method::GET;
                        current.body = None;
                    }
                    current.url = next;
                    continue;
                }
                // 第 5 步：正文 ≤50MiB（Content-Length 预检 + 流式计数双保险）
                if let Some(len) = resp.content_length()
                    && len as usize > MAX_BODY_BYTES
                {
                    return Err(ToolError::invalid_arg(format!(
                        "响应体大小 {} 字节超过 50MiB 上限，已终止下载",
                        len
                    )));
                }
                let headers = resp.headers().clone();
                let mut body = Vec::new();
                loop {
                    let chunk = tokio::select! {
                        biased;
                        _ = token.cancelled() => {
                            return Err(ToolError::new("E_CANCELLED", "下载被取消"));
                        }
                        chunk = resp.chunk() => chunk,
                    };
                    let Some(bytes) = chunk.map_err(|e| classify_reqwest_error(&e))? else {
                        break;
                    };
                    if body.len() + bytes.len() > MAX_BODY_BYTES {
                        return Err(ToolError::invalid_arg(format!(
                            "响应体超过 50MiB 上限（已收 {} 字节），已终止下载",
                            body.len() + bytes.len()
                        )));
                    }
                    body.extend_from_slice(&bytes);
                }
                return Ok(FetchResponse {
                    url,
                    status,
                    headers,
                    body,
                });
            }
        }
    }
    // 循环耗尽（理论不可达：MAX_REDIRECTS+1 跳在循环内已拦）
    unreachable!("重定向循环计数越界")
}

/// URL 的源（scheme://host:port）：跨源判定用。默认端口归一（https=443、
/// http=80），`https://a.com/` 与 `https://a.com:443/` 同源。
fn origin_of(url: &Url) -> String {
    let port = url
        .port_or_known_default()
        .map(|p| format!(":{p}"))
        .unwrap_or_default();
    format!("{}://{}{}", url.scheme(), url.host_str().unwrap_or(""), port)
}

/// 把 reqwest 错误映射为工具错误：source 链里找到 [`SsrfError`] →
/// E_SSRF_BLOCKED（带类别）；其余网络失败 → E_INTERNAL 兜底（§3.6 无
/// 通用网络码，决议记录说明）。
pub fn classify_reqwest_error(err: &reqwest::Error) -> ToolError {
    let mut source: Option<&(dyn Error + 'static)> = Some(err);
    while let Some(s) = source {
        if let Some(ssrf) = s.downcast_ref::<SsrfError>() {
            return ToolError::ssrf_blocked(format!(
                "SSRF 防护拦截（{}）：{}",
                ssrf.category.label(),
                ssrf.detail
            ));
        }
        source = s.source();
    }
    ToolError::new("E_INTERNAL", format!("网络请求失败：{err}"))
}

#[cfg(test)]
#[path = "ssrf_test.rs"]
mod ssrf_test;
