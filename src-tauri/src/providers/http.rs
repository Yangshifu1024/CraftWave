//! 代理 HTTP 客户端工厂：none/system/manual 三模式
//! 的 reqwest 客户端统一装配点 + 进程级缓存与热生效。
//!
//! 为什么收敛在这里：代理之前分散在两处各自为政—— `safety/ssrf.rs`
//! 的 `build_client`（网络工具侧）与 rig 的共享 client（模型请求侧，rig
//! 默认进程级 reqwest client 不读我们的代理配置）。本模块提供唯一的
//! 「ProxyConfig → reqwest::Client」装配函数，两侧共用（ssrf.rs 保留其
//! SSRF 特有的 resolver/重定向/超时装配，代理段改调这里）；模型请求侧由
//! [`HttpClientFactory`] 缓存配置好的 client，`resolve_model` 经
//! `rig_reqwest::ReqwestClient::from(client)` 注入 rig（代理经
//! rig-reqwest builder 注入）。
//!
//! 三模式语义（AppConfig.proxy 配置域）：
//! - `none`（及未配置 `proxy` 字段）：`ClientBuilder::no_proxy()` 显式禁用
//!   ——语义 = 直连，环境变量 `http_proxy` 等一律不读（防本机开发环境的
//!   代理变量泄漏进用户应用）；
//! - `system`：reqwest 内建探测 = 环境变量（http_proxy/https_proxy/all_proxy，
//!   尊重 NO_PROXY）+ macOS SystemConfiguration / Windows 系统代理设置
//!   （reqwest 0.13 `system-proxy` feature 的 hyper-util 探测）——不重复造轮子；
//! - `manual`：`Proxy::all(url)`；url 支持 http/https/socks5/socks5h
//!   （socks5h = 域名由代理远端解析），scheme 白名单校验在
//!   [`validate_proxy_url`]，`AppConfig::normalize` 与本模块共用同一口径。
//!
//! 热生效：`save_config` 保存成功 → [`HttpClientFactory::reload`]
//! 重建缓存 client（generation 计数递增）；进行中的 run 不切换——run 的
//! rig 客户端在 `resolve_model` 时按当时代工厂快照构建并整个 run 复用
//! （"客户端在 run 内复用"），下一次 start_chat 取新 client。

use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;

use crate::providers::types::{ProxyConfig, ProxyMode};
use crate::util::errors::{E_INVALID_ARG, IpcError};

/// 客户端 User-Agent（可观测性：服务端日志可辨识 CraftWave 流量）。
const USER_AGENT: &str = concat!("CraftWave/", env!("CARGO_PKG_VERSION"));

/// 连接建立超时（秒）：仅约束 TCP/TLS 握手阶段，不约束响应时长——模型
/// 请求的流式回复可达分钟级，绝不设整体 timeout（探测请求的超时由
/// probe.rs 按请求覆盖设置）。
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 代理地址的 scheme 白名单（socks5/socks5h；含大小写变体归一）。
pub const PROXY_SCHEMES: [&str; 4] = ["http", "https", "socks5", "socks5h"];

/// 校验代理地址：scheme 必须在 [`PROXY_SCHEMES`] 白名单内且
/// `://` 后带主机部分。`AppConfig::normalize`（保存前校验）与本模块装配
/// （运行时防御）共用同一口径，错误消息人读。
pub fn validate_proxy_url(url: &str) -> Result<(), String> {
    let hint = || {
        PROXY_SCHEMES
            .iter()
            .map(|s| format!("{s}://"))
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let trimmed = url.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err(format!("代理地址缺少协议前缀（支持 {}）", hint()));
    };
    if !PROXY_SCHEMES.contains(&scheme.to_ascii_lowercase().as_str()) {
        return Err(format!(
            "代理地址协议不支持：{scheme}://（支持 {}）",
            hint()
        ));
    }
    if rest.trim().is_empty() {
        return Err(format!(
            "代理地址缺少主机部分（如 127.0.0.1:7890；支持 {}）",
            hint()
        ));
    }
    Ok(())
}

/// 按 [`ProxyConfig`] 把代理装配进 reqwest builder（三模式语义见模块注释）。
/// 返回 `Err(String)` 为配置错误（调用方按 E_INVALID_ARG 归面）。
///
/// 收敛点：`safety/ssrf.rs::build_client` 的代理段改调本函数——
/// 此前该侧对 `system` 模式只有 reqwest 默认行为（本仓 reqwest 未开
/// system-proxy feature 时实际不读系统设置），统一后网络工具与模型请求
/// 的代理语义完全一致。
pub fn apply_proxy(
    builder: reqwest::ClientBuilder,
    proxy: Option<&ProxyConfig>,
) -> Result<reqwest::ClientBuilder, String> {
    match proxy {
        // none / 未配置：显式禁用代理（不探测环境变量，语义 = 直连）
        None
        | Some(ProxyConfig {
            mode: ProxyMode::None,
            ..
        }) => Ok(builder.no_proxy()),
        // system：reqwest 内建系统代理探测（环境变量 + macOS/Windows 系统设置）
        Some(ProxyConfig {
            mode: ProxyMode::System,
            ..
        }) => Ok(builder),
        // manual：必须带 url，scheme 白名单校验后交给 reqwest
        //（socks5/socks5h 的解析在 reqwest socks feature 内，非法地址在此报错）
        Some(ProxyConfig {
            mode: ProxyMode::Manual,
            url: Some(url),
            ..
        }) => {
            validate_proxy_url(url)?;
            let proxy = reqwest::Proxy::all(url.trim())
                .map_err(|e| format!("代理地址非法（{url}）：{e}"))?;
            Ok(builder.proxy(proxy))
        }
        Some(ProxyConfig {
            mode: ProxyMode::Manual,
            url: None,
            ..
        }) => Err("代理模式为 manual 但未配置代理地址（proxy.url）".to_string()),
    }
}

/// 按代理配置构建 reqwest 客户端（模型请求与探测共用的基座：UA +
/// 连接超时；不设整体超时——流式回复不设上限，探测按请求覆盖）。
pub fn build_proxy_client(proxy: Option<&ProxyConfig>) -> Result<reqwest::Client, IpcError> {
    apply_proxy(reqwest::Client::builder(), proxy)
        .map_err(|e| IpcError::new(E_INVALID_ARG, e))?
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| IpcError::internal(format!("HTTP 客户端构建失败：{e}")))
}

/// 进程级代理 HTTP 客户端缓存（热生效）：
///
/// - 读侧 [`Self::client`] 无锁取当前快照（`Arc<reqwest::Client>`，克隆即
///   引用计数 +1）——`resolve_model` 每 run 构建时取用；
/// - 写侧 [`Self::reload`] 整体换新并递增 generation——`save_config` 保存
///   成功后调用；进行中请求已持有旧 `Arc` 不受影响（ArcSwap 语义：新旧
///   client 各自存活到引用归零），下一次构建取新值；
/// - reload 失败（新配置非法）保留旧 client 不破坏可用性——调用方决定是否
///   把错误上报（save_config 路径上 normalize 已先行校验，此处失败属异常）。
pub struct HttpClientFactory {
    client: ArcSwap<reqwest::Client>,
    generation: AtomicU64,
}

// HttpClientFactory 的构造与热更新。
impl HttpClientFactory {
    /// 按代理配置构建工厂（初代 client 立即就绪；配置非法时返回错误——
    /// AppState 装配路径上的 proxy 已过 normalize，理论上不会失败）。
    pub fn from_config(proxy: Option<&ProxyConfig>) -> Result<Self, IpcError> {
        Ok(Self {
            client: ArcSwap::from_pointee(build_proxy_client(proxy)?),
            generation: AtomicU64::new(1),
        })
    }

    /// 当前生效的客户端快照（无锁；克隆共享同一连接池）。
    pub fn client(&self) -> std::sync::Arc<reqwest::Client> {
        self.client.load_full()
    }

    /// 当前代数（从 1 起；每次 reload 成功 +1）。测试与诊断用：
    /// 断言"保存后确实换了新客户端"的可观测面。
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// 热生效：按新代理配置重建缓存客户端（save_config 边界）。
    /// 成功 = generation +1 且后续 [`Self::client`] 返回新实例；失败保留
    /// 旧 client 并返回错误（不把工厂置于无客户端状态）。
    pub fn reload(&self, proxy: Option<&ProxyConfig>) -> Result<(), IpcError> {
        let next = build_proxy_client(proxy)?;
        self.client.store(next.into());
        self.generation.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}

#[cfg(test)]
#[path = "http_test.rs"]
mod http_test;
