//! MCP 连接池（FR-08-05，§9.7 字段级；08 号票）。
//!
//! 连接池 = 每个已配置服务器的连接生命周期管家：
//! - **key 与容量**：key = `(scope, projectId, name, canonicalPath)`（§9.7），
//!   LRU 上限 16（超容驱逐「非期望保持 + 最久未用」优先的条目）；
//! - **状态机**：`Starting → Ready → Stopped/Error`，每次迁移发
//!   `mcp:status` 事件（§3.5 M2 键，payload `{scope, name, state, detail?}`）；
//! - **generation 计数**：每次连接动作（connect/reconnect）递增，旧握手与
//!   旧断线回调发现 generation 已过期即丢弃——防"旧握手复活"（重连后旧
//!   初始化回调把状态改回 Ready）；
//! - **断线自动重连**：连接结束（server 退出/传输关闭）→ Error → 指数
//!   退避（1s/2s/4s…封顶 60s）重连；用户显式 disconnect 后 desired=false
//!   停止重连；重连前重核启动信任（指纹不在信任表 → 停在 Error，等用户
//!   重新走 TOFU，§9.7"指纹变更重问"）；
//! - **结果未知不重放**（Q14，§9.7）：已发出而结果未知的工具调用返回
//!   可辨识的 `E_MCP_RESULT_UNKNOWN`（"可能已执行"），由主代理先核查再
//!   决定重试；本层与上层都不得经通用失败重试路径重发。
//!
//! stdio 子进程：process-wrap 套进程组（Unix，子进程为组长）/Job Object
//! （Windows）+ KillOnDrop，断开/超时经组级信号整树收割（与 §8.8 命令
//! 治理同款语义；直接子进程另有 tokio kill_on_drop 兜底防孤儿）。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::{FutureExt, Shared};
use rmcp::model::{CallToolRequestParams, ClientConfig};
use rmcp::service::RunningServiceCancellationToken;
use rmcp::service::{Peer, RoleClient, ServiceError, ServiceExt};

use super::config::{McpScope, McpServerConfig};
use super::tools::{ConnKey, McpToolIdentity, McpToolRegistry};
use super::trust::TrustStore;
use crate::tools::{ToolAnnotations, ToolOutcome};
use crate::util::errors::{E_INVALID_ARG, ToolError};
use crate::util::events::EventSink;

/// 新增错误码（§3.6，08 号票补录）：
/// - `E_MCP_DISCONNECTED`：连接不在 Ready 或请求明确未完成，调用**未执行**
///   （普通失败，模型可直接重试）；
/// - `E_MCP_RESULT_UNKNOWN`：调用**已发出**但连接断开/超时，结果未知——
///   该操作可能已经执行，模型必须先核查实际结果再决定重试（read_only
///   声明不豁免；不得经通用失败重试路径重发）。
pub const E_MCP_DISCONNECTED: &str = "E_MCP_DISCONNECTED";
pub const E_MCP_RESULT_UNKNOWN: &str = "E_MCP_RESULT_UNKNOWN";

/// 连接池容量上限（§9.7 LRU ≤16）。
pub const POOL_CAPACITY: usize = 16;

/// 断线重连退避基数（1s → 2s → 4s → … 封顶 60s）。
const RECONNECT_BASE: Duration = Duration::from_secs(1);
/// 断线重连退避封顶。
const RECONNECT_CAP: Duration = Duration::from_secs(60);
/// stdio 启动与 HTTP 握手的初始化超时。
const INIT_TIMEOUT: Duration = Duration::from_secs(30);

/// 连接状态机（§9.7 Starting→Ready→Stopped/Error；§3.5 事件载荷字符串面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    /// 连接建立中（spawn/握手）。
    Starting,
    /// 就绪（initialize 完成、工具清单已同步）。
    Ready,
    /// 用户主动断开（desired=false，不再自动重连）。
    Stopped,
    /// 错误（启动失败/连接断开；detail 带原因）。
    Error,
}

impl ConnState {
    /// 事件载荷字符串（§3.5："starting"|"ready"|"stopped"|"error"）。
    pub fn as_str(self) -> &'static str {
        match self {
            ConnState::Starting => "starting",
            ConnState::Ready => "ready",
            ConnState::Stopped => "stopped",
            ConnState::Error => "error",
        }
    }
}

/// 连接结束信号（server 退出/传输关闭即完成；Shared 供监控任务与池共享）。
type ClosedSignal = Shared<Pin<Box<dyn Future<Output = ()> + Send>>>;

/// 存活连接的共享句柄（Ready 期间挂在池条目上；调用面只碰 peer）。
pub(crate) struct LiveConn {
    /// 请求出口（rmcp 服务端对端句柄；Clone 廉价）。
    pub(crate) peer: Peer<RoleClient>,
    /// 连接取消令牌（断开时 take + cancel 触发服务任务收口；令牌的
    /// cancel(self) 消费所有权，故挂 Option）。
    pub(crate) cancel: Option<RunningServiceCancellationToken>,
    /// 连接结束信号。
    pub(crate) closed: ClosedSignal,
    /// stdio 子进程收割句柄（断开显式组级整树收割；直接子进程另有
    /// kill_on_drop 兜底）。
    pub(crate) reaper: Option<Box<dyn process_wrap::tokio::ChildWrapper>>,
}

/// stdio 子进程的整树收割（复用 §8.8 收割语义）。
async fn reap_child(mut child: Box<dyn process_wrap::tokio::ChildWrapper>) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // 子进程是组长（ProcessGroup::leader）：kill(-pgid) 收割整棵进程树
        // （SIGTERM → 宽限轮询 → SIGKILL，tools/process 同款节奏）。
        let pid = pid as i32;
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        let deadline = std::time::Instant::now() + crate::tools::process::REAP_GRACE;
        while std::time::Instant::now() < deadline {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    // 组级信号收口的兜底：SIGKILL 结果不判定（进程已在收割即视为成功；
    // kill 失败只说明进程已自行退出，无资源泄漏面）。
    let _ = Box::into_pin(child.kill()).await;
}

/// 池内条目：一个服务器的连接生命周期状态。
struct PoolEntry {
    /// 连接键。
    key: ConnKey,
    /// 当前状态。
    state: ConnState,
    /// 状态明细（Error 原因等）。
    detail: Option<String>,
    /// generation 计数（防旧握手复活）。
    generation: u64,
    /// Ready 时的连接句柄。
    live: Option<LiveConn>,
    /// 是否期望保持连接（false = 用户断开，停止自动重连）。
    desired: bool,
    /// 退避步数（连续失败次数）。
    backoff_step: u32,
    /// 重连所需的配置快照（断线重连时不再重读文件——配置变更走用户显式
    /// 重连/TOFU 重核路径）。
    pending_config: Option<McpServerConfig>,
    /// 重连所需的启动指纹快照。
    pending_fingerprint: Option<String>,
    /// 最近使用时刻（LRU 驱逐依据）。
    last_used: std::time::Instant,
}

/// MCP 宿主：连接池 + 信任/许可存储 + 事件出口（AppState 持有 Arc）。
pub struct McpHost {
    /// 池内条目（key → entry）。
    inner: Mutex<HashMap<ConnKey, PoolEntry>>,
    /// 事件出口（mcp:status）。
    events: Arc<dyn EventSink>,
    /// 启动信任存储（`trust/mcp.json`）。
    pub trust: TrustStore,
    /// 工具级许可存储（`trust/mcp-tools.json`，Q44）。
    pub licenses: super::trust::ToolLicenseStore,
    /// 数据目录（`~/.craftwave`）。
    data_dir: std::path::PathBuf,
    /// 清单轮询兜底的间隔（生产 30s；测试可调短——订阅通知可能因 server
    /// 能力/协议版本缺投递，轮询保证清单最终一致）。
    poll_interval: Mutex<Duration>,
}

// McpHost 的构造与查询面。
impl McpHost {
    /// 构造（data_dir = `~/.craftwave`；events 为应用事件汇）。
    pub fn new(data_dir: std::path::PathBuf, events: Arc<dyn EventSink>) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            events,
            trust: TrustStore::new(&data_dir),
            licenses: super::trust::ToolLicenseStore::new(&data_dir),
            data_dir,
            poll_interval: Mutex::new(Duration::from_secs(30)),
        }
    }

    /// 设置清单轮询兜底间隔（测试注入短间隔；装配点在 AppState 构造后）。
    pub fn set_poll_interval(&self, interval: Duration) {
        *self
            .poll_interval
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = interval;
    }

    /// 数据目录。
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    /// 工具注册表快捷访问（动态段单例）。
    pub fn registry(&self) -> &'static McpToolRegistry {
        McpToolRegistry::global()
    }

    /// 读取某服务器当前状态（管理页信任/状态列）。
    pub fn state_of(&self, key: &ConnKey) -> Option<(ConnState, Option<String>)> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .map(|e| (e.state, e.detail.clone()))
    }

    /// 连接池快照（§3.3 #47 mcp_snapshot）。
    pub fn snapshot(&self) -> Vec<McpServerSnapshot> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .map(|e| McpServerSnapshot {
                name: e.key.name.clone(),
                scope: e.key.scope,
                state: e.state,
                tool_count: if e.state == ConnState::Ready {
                    McpToolRegistry::global().tool_count(&e.key)
                } else {
                    0
                },
                detail: e.detail.clone(),
            })
            .collect()
    }
}

/// mcp_snapshot 的单服务器行（§3.3 #47：{name, state, toolCount} + 补充面）。
#[derive(Debug, Clone)]
pub struct McpServerSnapshot {
    /// 配置名。
    pub name: String,
    /// 作用域。
    pub scope: McpScope,
    /// 连接状态。
    pub state: ConnState,
    /// 已注册工具数（Ready 时）。
    pub tool_count: usize,
    /// 状态明细。
    pub detail: Option<String>,
}

// McpHost 的连接生命周期管理。
impl McpHost {
    /// 发起连接（TOFU 由命令层核验后调用；spawn 后台任务，状态经事件可见）。
    /// 已有条目则 generation+1（旧任务退场），desired=true。
    pub fn connect(self: &Arc<Self>, key: ConnKey, config: McpServerConfig, fingerprint: String) {
        let generation = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = inner.entry(key.clone()).or_insert_with(|| PoolEntry {
                key: key.clone(),
                state: ConnState::Stopped,
                detail: None,
                generation: 0,
                live: None,
                desired: false,
                backoff_step: 0,
                pending_config: None,
                pending_fingerprint: None,
                last_used: std::time::Instant::now(),
            });
            entry.generation += 1;
            entry.desired = true;
            entry.backoff_step = 0;
            entry.pending_config = Some(config.clone());
            entry.pending_fingerprint = Some(fingerprint.clone());
            entry.generation
        };
        tokio::spawn(Self::connect_task(
            self.clone(),
            key,
            config,
            fingerprint,
            generation,
        ));
    }

    /// 用户主动断开：generation+1（在飞任务退场）→ 取消连接 → 整树收割 →
    /// Stopped（desired=false，不再自动重连）→ 动态工具段即时收窄。
    pub fn disconnect(&self, key: &ConnKey) {
        let live = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = inner.get_mut(key) else {
                return;
            };
            entry.generation += 1;
            entry.desired = false;
            entry.backoff_step = 0;
            entry.state = ConnState::Stopped;
            entry.detail = None;
            entry.live.take()
        };
        McpToolRegistry::global().remove(key);
        if let Some(live) = live {
            if let Some(token) = live.cancel {
                token.cancel();
            }
            if let Some(reaper) = live.reaper {
                tokio::spawn(reap_child(reaper));
            }
        }
        self.emit_status(key, ConnState::Stopped, None);
    }

    /// 用户重连：先按 disconnect 收口再 connect（generation 连续递增，
    /// 旧监控/旧握手全部退场）。
    pub fn reconnect(
        self: &Arc<Self>,
        key: &ConnKey,
        config: McpServerConfig,
        fingerprint: String,
    ) {
        self.disconnect(key);
        self.connect(key.clone(), config, fingerprint);
    }

    /// 状态迁移 + mcp:status 事件（generation 不匹配则丢弃——旧握手/旧
    /// 断线回调不得改写新一代状态）。
    fn set_state(&self, key: &ConnKey, generation: u64, state: ConnState, detail: Option<String>) {
        {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = inner.get_mut(key) else {
                return;
            };
            if entry.generation != generation {
                return;
            }
            entry.state = state;
            entry.detail = detail.clone();
        }
        self.emit_status(key, state, detail);
    }

    /// 发 mcp:status 事件（§3.5 载荷逐字）。
    fn emit_status(&self, key: &ConnKey, state: ConnState, detail: Option<String>) {
        self.events.emit(
            crate::util::events::keys::MCP_STATUS,
            serde_json::json!({
                "scope": key.scope.as_str(),
                "name": key.name,
                "state": state.as_str(),
                "detail": detail,
            }),
        );
    }

    /// 后台连接任务：Starting →（spawn/握手）→ 拉清单 → Ready + 挂断线
    /// 监控；失败 → Error → 调度退避重连。
    ///
    /// 返回类型显式装箱（`Pin<Box<dyn Future>>` 而非 async fn 的 opaque）：
    /// fail_and_retry 的重连 spawn 会再次引用本函数，opaque 自引用会触发
    /// 编译期类型循环（E0391）——装箱后递归引用的是已命名类型。
    fn connect_task(
        host: Arc<Self>,
        key: ConnKey,
        config: McpServerConfig,
        fingerprint: String,
        generation: u64,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(Self::connect_task_inner(
            host,
            key,
            config,
            fingerprint,
            generation,
        ))
    }

    /// [`Self::connect_task`] 的 async 本体（装箱层内部，不再被递归推导）。
    async fn connect_task_inner(
        host: Arc<Self>,
        key: ConnKey,
        config: McpServerConfig,
        fingerprint: String,
        generation: u64,
    ) {
        host.set_state(&key, generation, ConnState::Starting, None);
        // 重连路径的信任重核：指纹不在信任表（配置变过/被撤销）→ 不连接，
        // 停在 Error 等用户重新走 TOFU（§9.7"指纹变更重问"）。
        if !host.trust.is_trusted(&fingerprint) {
            host.set_state(
                &key,
                generation,
                ConnState::Error,
                Some("配置已变更或信任已撤销，需要重新信任后再连接".to_owned()),
            );
            return;
        }
        match open_connection(&config, host.clone(), key.clone(), generation).await {
            Ok(live) => {
                // 拉清单并同步注册表；tools/list 失败说明连接不可用，按
                // Error 收口（不进 Ready）。
                match list_tools(&live.peer, Duration::from_secs(30)).await {
                    Ok(tools) => {
                        let registered =
                            host.registry()
                                .sync(key.clone(), fingerprint.clone(), &config, tools);
                        // 服务器级 always_allow 的自动授权（Q44）：清单同步
                        // 后批量授予许可；schema 有未确认变更（许可表存在
                        // 同工具不同 hash 的记录）的工具不自动放行，等用户
                        // 首次调用重新确认。
                        if config.always_allow {
                            host.auto_grant_licenses(&key, &fingerprint);
                        }
                        // 连接结束信号先取克隆件（Shared 廉价），live 整体
                        // move 进条目（cancel 令牌不可克隆）。
                        let closed_signal = live.closed.clone();
                        let mut stale = false;
                        {
                            let mut inner = host
                                .inner
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            match inner.get_mut(&key) {
                                Some(entry) if entry.generation == generation => {
                                    entry.live = Some(live);
                                    entry.state = ConnState::Ready;
                                    entry.detail = None;
                                    entry.last_used = std::time::Instant::now();
                                }
                                _ => stale = true,
                            }
                        }
                        let closed = if stale {
                            // 旧任务：连接刚建立就过期（用户已重连/断开）
                            // → 立即收口退出，不进 Ready。live 已被 move——
                            // 经池条目重取句柄收口（generation 过期时条目里
                            // 若已换成新连接则不能动它，这里直接返回由调用
                            // 方的 disconnect 路径收口）。
                            return;
                        } else {
                            Some(closed_signal)
                        };
                        host.emit_status(
                            &key,
                            ConnState::Ready,
                            Some(format!("{} 个工具已注册", registered.len())),
                        );
                        host.evict_if_overflow();
                        // 挂断线监控：连接结束 → generation 仍有效 →
                        // Error + 退避重连。
                        if let Some(closed) = closed {
                            let host2 = host.clone();
                            let key2 = key.clone();
                            tokio::spawn(async move {
                                closed.await;
                                host2.on_connection_lost(&key2, generation).await;
                            });
                        }
                        // 挂清单轮询兜底（订阅通知可能因 server 能力/协议
                        // 版本缺投递——轮询 + 签名比对保证清单最终一致；
                        // 间隔可经 set_poll_interval 调短，测试 200ms）。
                        {
                            // 有 Ready 连接才挂轮询；live 句柄挂在池条目上，
                            // 轮询任务无需自持 peer 克隆（generation/desired
                            // 失效即退场，连接断开由断线监控收口）。
                            let has_live = host
                                .inner
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .get(&key)
                                .is_some_and(|e| e.live.is_some());
                            if has_live {
                                let host2 = host.clone();
                                let key2 = key.clone();
                                let interval = *host
                                    .poll_interval
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                tokio::spawn(async move {
                                    let mut ticker = tokio::time::interval(interval);
                                    ticker.set_missed_tick_behavior(
                                        tokio::time::MissedTickBehavior::Delay,
                                    );
                                    ticker.tick().await; // 首跳立即返回，跳过
                                    loop {
                                        ticker.tick().await;
                                        let go = {
                                            let inner = host2
                                                .inner
                                                .lock()
                                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                                            inner.get(&key2).is_some_and(|e| {
                                                e.desired
                                                    && e.state == ConnState::Ready
                                                    && e.generation == generation
                                                    && e.live.is_some()
                                            })
                                        };
                                        if !go {
                                            return;
                                        }
                                        host2.handle_tools_changed(&key2, generation).await;
                                        // 连接断开由断线监控收口；轮询在
                                        // generation/desired 失效即退场
                                    }
                                });
                            }
                        }
                        // 挂工具清单订阅（rmcp 3.8 的 subscriptions/listen
                        // 机制）：server 发 tools/list_changed → 重拉清单
                        // 热更新注册表（FR-08-06）。订阅失败不影响连接
                        // （热同步退化为下次重连后全量同步，尽力而为）。
                        {
                            let sub_peer = host
                                .inner
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .get(&key)
                                .and_then(|e| e.live.as_ref().map(|l| l.peer.clone()));
                            if let Some(sub_peer) = sub_peer {
                                let host2 = host.clone();
                                let key2 = key.clone();
                                tokio::spawn(async move {
                                    let filter = rmcp::model::SubscriptionFilter::builder()
                                        .tools_list_changed()
                                        .build();
                                    let Ok(mut subscription) = sub_peer.listen(filter).await else {
                                        return;
                                    };
                                    loop {
                                        match subscription.next().await {
                                            Ok(Some(
                                                rmcp::model::ServerNotification::ToolListChangedNotification(_),
                                            )) => {
                                                host2
                                                    .handle_tools_changed(&key2, generation)
                                                    .await;
                                            }
                                            Ok(Some(_)) => continue,
                                            Ok(None) | Err(_) => return,
                                        }
                                    }
                                });
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(token) = live.cancel {
                            token.cancel();
                        }
                        if let Some(reaper) = live.reaper {
                            tokio::spawn(reap_child(reaper));
                        }
                        host.fail_and_retry(
                            &key,
                            &config,
                            fingerprint,
                            generation,
                            format!("拉取工具清单失败：{e}"),
                        )
                        .await;
                    }
                }
            }
            Err(e) => {
                host.fail_and_retry(
                    &key,
                    &config,
                    fingerprint,
                    generation,
                    format!("连接失败：{e}"),
                )
                .await;
            }
        }
    }

    /// 连接丢失（监控任务回调）：generation 有效 → Error + 退避重连；
    /// generation 过期（用户已操作）→ 忽略。
    async fn on_connection_lost(self: Arc<Self>, key: &ConnKey, generation: u64) {
        let (config, fingerprint) = {
            let inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = inner.get(key) else { return };
            if entry.generation != generation {
                return;
            }
            (
                entry.pending_config.clone(),
                entry.pending_fingerprint.clone(),
            )
        };
        // 清掉死连接句柄并收割子进程。
        let dead = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = inner.get_mut(key) else {
                return;
            };
            entry.live.take()
        };
        if let Some(live) = dead {
            if let Some(token) = live.cancel {
                token.cancel();
            }
            if let Some(reaper) = live.reaper {
                tokio::spawn(reap_child(reaper));
            }
        }
        McpToolRegistry::global().remove(key);
        match (config, fingerprint) {
            (Some(config), Some(fingerprint)) => {
                self.fail_and_retry(
                    key,
                    &config,
                    fingerprint,
                    generation,
                    "连接已断开".to_owned(),
                )
                .await;
            }
            _ => {
                self.set_state(
                    key,
                    generation,
                    ConnState::Error,
                    Some("连接已断开".to_owned()),
                );
            }
        }
    }

    /// 失败收口 + 退避重连调度（desired=true 才重试；醒来后 generation 与
    /// desired 复核——期间用户可能已 disconnect 或重新 connect）。
    async fn fail_and_retry(
        self: &Arc<Self>,
        key: &ConnKey,
        config: &McpServerConfig,
        fingerprint: String,
        generation: u64,
        reason: String,
    ) {
        let (desired, step) = {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(entry) = inner.get_mut(key) else {
                return;
            };
            if entry.generation != generation {
                return;
            }
            entry.state = ConnState::Error;
            entry.detail = Some(reason.clone());
            entry.backoff_step = entry.backoff_step.saturating_add(1);
            (entry.desired, entry.backoff_step)
        };
        McpToolRegistry::global().remove(key);
        self.emit_status(key, ConnState::Error, Some(reason));
        self.evict_if_overflow();
        if !desired {
            return;
        }
        // 1s → 2s → 4s → 8s → 16s → 32s → 60s 封顶（步数再大不再翻倍）。
        let shift = (step - 1).min(6);
        let delay = RECONNECT_BASE
            .saturating_mul(1u32 << shift)
            .min(RECONNECT_CAP);
        let host = self.clone();
        let key2 = key.clone();
        let config2 = config.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let go = {
                let inner = host
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                inner
                    .get(&key2)
                    .is_some_and(|e| e.desired && e.generation == generation)
            };
            if go {
                // 装箱打破 async fn 递归（connect_task → fail_and_retry →
                // spawn → connect_task）的 opaque 类型循环。
                Self::connect_task(host, key2, config2, fingerprint, generation).await;
            }
        });
    }

    /// 超容驱逐（LRU）：>16 条时淘汰「非期望保持 + 最久未用」优先的条目
    /// （断开收口后从表移除）。
    fn evict_if_overflow(self: &Arc<Self>) {
        loop {
            let victim = {
                let inner = self
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if inner.len() <= POOL_CAPACITY {
                    return;
                }
                let mut candidates: Vec<(&ConnKey, bool, std::time::Instant)> = inner
                    .iter()
                    .map(|(k, e)| (k, e.desired, e.last_used))
                    .collect();
                // 排序：desired=false 优先，其次 last_used 最旧。
                candidates.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(&b.2)));
                candidates.first().map(|(k, _, _)| (*k).clone())
            };
            let Some(victim) = victim else { return };
            self.disconnect(&victim);
            self.inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&victim);
        }
    }

    /// 取 Ready 连接的请求出口并 touch LRU（调用面）。
    fn take_live(&self, key: &ConnKey) -> Option<Peer<RoleClient>> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = inner.get_mut(key)?;
        entry.last_used = std::time::Instant::now();
        entry.live.as_ref().map(|l| l.peer.clone())
    }

    /// 服务器级 always_allow 的许可自动授权（Q44）：对清单里的每个工具，
    /// 许可表无「同工具不同 hash 的未确认记录」时授予当前版本许可——
    /// schema 变更未经用户确认（审批「始终允许」）的工具不自动放行；
    /// read_only 工具天然走矩阵，授予无害（记录即确认痕迹）。
    pub(crate) fn auto_grant_licenses(&self, key: &ConnKey, fingerprint: &str) {
        let now = super::trust::now_ms();
        for (raw_name, schema_hash) in self.registry().tool_identities_of(key) {
            if !self
                .licenses
                .has_conflicting(fingerprint, &raw_name, &schema_hash)
            {
                let _ = self
                    .licenses
                    .grant(fingerprint, &raw_name, &schema_hash, now);
            }
        }
    }
}

/// 工具调用（执行器 dispatch 入口）：结果未知语义在此收口（Q14）。
///
/// - 归一名反查失败 → E_INVALID_ARG（未知动态工具）；
/// - 参数非对象 → E_INVALID_ARG（MCP 规范：参数是 JSON 对象或 null）；
/// - 连接不在 Ready → E_MCP_DISCONNECTED（未发出，可重试）；
/// - 已发出但断开/超时 → E_MCP_RESULT_UNKNOWN（可能已执行，先核查再重试）。
pub async fn call_tool(host: &McpHost, normalized_name: &str, args_json: &str) -> ToolOutcome {
    let Some(identity) = host.registry().identity(normalized_name) else {
        return ToolOutcome::failure(
            ToolError::new(E_INVALID_ARG, format!("未知 MCP 工具：{normalized_name}")),
            format!("未知 MCP 工具：{normalized_name}"),
        );
    };
    let args: serde_json::Value = match serde_json::from_str(args_json) {
        Ok(v) => v,
        Err(e) => {
            return ToolOutcome::failure(
                ToolError::invalid_arg(format!(
                    "MCP 工具 {} 参数解析失败：{e}",
                    identity.original_name
                )),
                "MCP 工具参数解析失败",
            );
        }
    };
    let Some(peer) = host.take_live(&identity.conn_key) else {
        return ToolOutcome::failure(
            ToolError::new(
                E_MCP_DISCONNECTED,
                format!(
                    "MCP 服务器 {} 未连接（本次调用未发出；连接恢复后可直接重试）",
                    identity.server_name
                ),
            ),
            format!("MCP {} 未连接", identity.server_name),
        );
    };
    // 组参数（JsonObject = serde_json::Map；非对象参数按 MCP 规范拒绝——
    // 静默降级为无参调用可能执行不同的操作）。
    let arguments = match args {
        serde_json::Value::Null => None,
        serde_json::Value::Object(map) => Some(map),
        other => {
            return ToolOutcome::failure(
                ToolError::invalid_arg(format!(
                    "MCP 工具 {} 参数必须是 JSON 对象，收到 {}",
                    identity.original_name,
                    json_kind(&other)
                )),
                "MCP 工具参数类型错误",
            );
        }
    };
    let mut request =
        CallToolRequestParams::new(std::borrow::Cow::Owned(identity.original_name.clone()));
    request.arguments = arguments;
    // —— 从这里起请求已发出：任何「没拿到结果」的收场都是结果未知（Q14）。
    let timeout = identity.call_timeout;
    match tokio::time::timeout(timeout, peer.call_tool(request)).await {
        Ok(Ok(call_result)) => {
            let is_error = call_result.is_error == Some(true);
            // 模型通道：structuredContent 优先，其次拼接 content 文本块。
            let mut text = String::new();
            if let Some(structured) = &call_result.structured_content {
                text = serde_json::to_string(structured).unwrap_or_default();
            }
            if text.is_empty() {
                for block in &call_result.content {
                    if let rmcp::model::ContentBlock::Text(t) = block {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(&t.text);
                    }
                }
            }
            let data = Some(serde_json::json!({
                "content": text,
                "structured": call_result.structured_content,
            }));
            if is_error {
                ToolOutcome {
                    ok: false,
                    data,
                    error: Some(ToolError::new(
                        E_INVALID_ARG,
                        format!("MCP 工具 {} 报告执行错误：{}", identity.original_name, text),
                    )),
                    ui_summary: format!("MCP {} 执行错误", identity.original_name),
                    detail: None,
                    truncated: None,
                }
            } else {
                ToolOutcome {
                    ok: true,
                    data,
                    error: None,
                    ui_summary: format!("MCP {} 调用成功", identity.original_name),
                    detail: None,
                    truncated: None,
                }
            }
        }
        // 超时：请求已发出，服务器可能仍在执行（取消不保证生效）→ 结果未知。
        Err(_elapsed) => result_unknown(
            &identity,
            format!("调用超时（{} 秒未收到结果）", timeout.as_secs()),
        ),
        // 传输已关闭 / 发送失败：连接断了，请求可能已被 server 收到 → 结果未知。
        Ok(Err(ServiceError::TransportClosed)) | Ok(Err(ServiceError::TransportSend(_))) => {
            result_unknown(&identity, "连接已断开".to_owned())
        }
        // 其余错误（协议错误等）：请求未完成且连接可判定——按普通失败回喂
        // （明确未执行，模型可直接重试）。
        Ok(Err(e)) => ToolOutcome::failure(
            ToolError::new(E_MCP_DISCONNECTED, format!("MCP 调用失败（未执行）：{e}")),
            format!("MCP {} 调用失败", identity.original_name),
        ),
    }
}

/// 「可能已执行」的可辨识结果（Q14）：ok=false + E_MCP_RESULT_UNKNOWN，
/// 行动指引写在错误消息里（§8.2：要求模型据此行动的结论进模型通道）；
/// detail 带结构化标记供 UI 与边车识别。
fn result_unknown(identity: &McpToolIdentity, reason: String) -> ToolOutcome {
    ToolOutcome {
        ok: false,
        data: Some(serde_json::json!({
            "resultUnknown": true,
            "server": identity.server_name,
            "tool": identity.original_name,
        })),
        error: Some(ToolError::new(
            E_MCP_RESULT_UNKNOWN,
            format!(
                "{reason}：调用已发出但未收到结果，结果未知——该操作可能已经执行。\
                 请先核查实际结果（例如重新查询目标状态），确认未生效后再决定是否重试；\
                 不要盲目重发本调用。"
            ),
        )),
        ui_summary: format!("MCP {} 结果未知（可能已执行）", identity.original_name),
        detail: Some(serde_json::json!({ "resultUnknown": true })),
        truncated: None,
    }
}

/// JSON 值的类型名（参数类型错误提示用）。
fn json_kind(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "布尔",
        serde_json::Value::Number(_) => "数字",
        serde_json::Value::String(_) => "字符串",
        serde_json::Value::Array(_) => "数组",
        serde_json::Value::Object(_) => "对象",
    }
}

/// `tools/list` 全量拉取（list_all_tools 内部翻页；timeout 为总预算）。
/// 返回 (原始名, 描述, 输入 schema, 注解透传) 元组列表。
pub(crate) async fn list_tools(
    peer: &Peer<RoleClient>,
    timeout: Duration,
) -> Result<Vec<(String, String, serde_json::Value, ToolAnnotations)>, String> {
    let page = tokio::time::timeout(timeout, peer.list_all_tools())
        .await
        .map_err(|_| "tools/list 超时".to_owned())?
        .map_err(|e| format!("tools/list 失败：{e}"))?;
    Ok(page
        .into_iter()
        .map(|tool| {
            let annotations = tool.annotations.as_ref();
            (
                tool.name.to_string(),
                tool.description.as_deref().unwrap_or("").to_owned(),
                tool.schema_as_json_value(),
                // 注解仅作元数据透传（远端 annotations 不提权，§9.7 Q44）。
                ToolAnnotations {
                    read_only_hint: annotations.is_some_and(|a| a.read_only_hint.unwrap_or(false)),
                    destructive_hint: annotations
                        .is_some_and(|a| a.destructive_hint.unwrap_or(false)),
                    idempotent_hint: annotations
                        .is_some_and(|a| a.idempotent_hint.unwrap_or(false)),
                    open_world_hint: annotations
                        .is_some_and(|a| a.open_world_hint.unwrap_or(false)),
                },
            )
        })
        .collect())
}

/// 草稿试连的临时连接（§3.3 #45 mcp_test）：持有连接句柄，Drop 时取消
/// 服务任务并收割 stdio 子进程（不进池、不发状态事件）。
pub(crate) struct TestConnection {
    /// 请求出口（tools/list 用）。
    pub peer: Peer<RoleClient>,
    /// 连接本体（Drop 收口）。
    live: LiveConn,
}

// TestConnection 的 Drop 收口。
impl Drop for TestConnection {
    fn drop(&mut self) {
        // 令牌的 cancel(self) 消费所有权：Drop 里 take 后取消
        if let Some(token) = self.live.cancel.take() {
            token.cancel();
        }
        if let Some(reaper) = self.live.reaper.take()
            && let Ok(handle) = tokio::runtime::Handle::try_current()
        {
            handle.spawn(reap_child(reaper));
        }
    }
}

/// 草稿试连入口（mcp_test 专用；不进池、不写信任、不发 mcp:status）。
pub(crate) async fn open_for_test(
    config: &McpServerConfig,
    host: Arc<McpHost>,
) -> Result<TestConnection, String> {
    let key = ConnKey {
        scope: McpScope::Global,
        project_id: None,
        name: "test-connection".to_owned(),
        canonical_path: String::new(),
    };
    let live = open_connection(config, host, key, 0).await?;
    Ok(TestConnection {
        peer: live.peer.clone(),
        live,
    })
}

/// 按配置打开一条连接（stdio / streamable HTTP 双传输，FR-08-01）。
/// 返回就绪连接句柄（initialize 握手完成）。
pub(crate) async fn open_connection(
    config: &McpServerConfig,
    host: Arc<McpHost>,
    key: ConnKey,
    generation: u64,
) -> Result<LiveConn, String> {
    let handler = ListChangedHandler {
        host,
        key,
        generation,
        client: ClientConfig::default(),
    };
    // 测试注入面（fixture.rs）：进程内 fixture 命中即走（不走 stdio/HTTP
    // 真实传输；握手、清单同步、断线重连与其余全链一致）。handler 可克隆
    //（ListChangedHandler: Clone），fixture 未命中时原值继续走真实传输。
    #[cfg(test)]
    if let Some(live) = crate::mcp::fixture::take_transport(config, handler.clone()).await {
        return Ok(live);
    }
    if let Some(command) = &config.command {
        // —— stdio 传输（进程组/Job Object + KillOnDrop）——
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(&config.args);
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped());
        #[cfg(unix)]
        let mut wrapped = {
            use process_wrap::tokio::{CommandWrap, KillOnDrop, ProcessGroup};
            let mut wrapper = CommandWrap::from(cmd);
            // 子进程为组长（pgid = 自身 pid）：断开时 kill(-pgid) 整树收割。
            wrapper.wrap(ProcessGroup::leader());
            wrapper.wrap(KillOnDrop);
            wrapper
        };
        #[cfg(windows)]
        let mut wrapped = {
            use process_wrap::tokio::{CommandWrap, JobObject, KillOnDrop};
            let mut wrapper = CommandWrap::from(cmd);
            // Job Object：KILL_ON_JOB_CLOSE 兜底 + TerminateJobObject 整树。
            wrapper.wrap(JobObject);
            wrapper.wrap(KillOnDrop);
            wrapper
        };
        let mut child = wrapped
            .spawn()
            .map_err(|e| format!("stdio 启动失败：{e}"))?;
        let stdout = child
            .inner_mut()
            .stdout()
            .take()
            .ok_or("stdio 启动失败：stdout 未管道化")?;
        let stdin = child
            .inner_mut()
            .stdin()
            .take()
            .ok_or("stdio 启动失败：stdin 未管道化")?;
        // (stdout, stdin) 元组 = IntoTransport（async-rw 行式 JSON-RPC）。
        let service = tokio::time::timeout(INIT_TIMEOUT, handler.serve((stdout, stdin)))
            .await
            .map_err(|_| "stdio 握手超时".to_owned())?
            .map_err(|e| format!("stdio 握手失败：{e}"))?;
        Ok(LiveConn::from_service(service, Some(child)))
    } else if let Some(url) = &config.url {
        // —— streamable HTTP 传输（reqwest rustls；headers 附加）——
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| format!("HTTP 客户端构建失败：{e}"))?;
        let mut http_config =
            rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                url.as_str(),
            );
        if !config.headers.is_empty() {
            let mut headers = HashMap::new();
            for (k, v) in &config.headers {
                let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
                    .map_err(|e| format!("非法请求头名 {k}：{e}"))?;
                let value = reqwest::header::HeaderValue::from_str(v)
                    .map_err(|e| format!("非法请求头值 {k}：{e}"))?;
                headers.insert(name, value);
            }
            http_config = http_config.custom_headers(headers);
        }
        let transport =
            rmcp::transport::StreamableHttpClientTransport::with_client(client, http_config);
        let service = tokio::time::timeout(INIT_TIMEOUT, handler.serve(transport))
            .await
            .map_err(|_| "HTTP 握手超时".to_owned())?
            .map_err(|e| format!("HTTP 握手失败：{e}"))?;
        Ok(LiveConn::from_service(service, None))
    } else {
        Err("配置缺少 command（stdio）或 url（HTTP）：无法确定传输方式".to_owned())
    }
}

// LiveConn 的构造。
impl LiveConn {
    /// 从 rmcp 运行服务提取共享句柄（生命周期模式与 rig-rmcp McpConnection
    /// 一致：peer 克隆 + 取消令牌 + waiting() 转 Shared 完成信号）。
    pub(crate) fn from_service<H: rmcp::service::Service<RoleClient>>(
        service: rmcp::service::RunningService<RoleClient, H>,
        reaper: Option<Box<dyn process_wrap::tokio::ChildWrapper>>,
    ) -> Self {
        let peer = service.peer().clone();
        let cancel = Some(service.cancellation_token());
        let closed: Pin<Box<dyn Future<Output = ()> + Send>> = Box::pin(async move {
            // 连接自然结束（server 退出/传输关闭）由 rmcp 记日志，这里只
            // 转成完成信号供监控任务消费。
            let _ = service.waiting().await;
        });
        Self {
            peer,
            cancel,
            closed: closed.shared(),
            reaper,
        }
    }
}

/// tools/list_changed 通知的客户端处理器：重拉清单热更新注册表并重导
/// toolSpecs（FR-08-06）。generation 过期（重连后旧连接的迟到通知）丢弃。
#[derive(Clone)]
pub(crate) struct ListChangedHandler {
    host: Arc<McpHost>,
    key: ConnKey,
    generation: u64,
    /// 客户端信息（协议要求；默认配置即可）。
    client: ClientConfig,
}

impl rmcp::ClientHandler for ListChangedHandler {
    /// 协议要求的客户端能力/信息声明（无额外能力）。
    fn get_info(&self) -> ClientConfig {
        self.client.clone()
    }

    /// 工具清单变化（未走订阅路由时的兜底回调）：重拉 + 热更新。
    fn on_tool_list_changed(
        &self,
        _context: rmcp::service::NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        let host = self.host.clone();
        let key = self.key.clone();
        let generation = self.generation;
        async move { host.handle_tools_changed(&key, generation).await }
    }
}

// McpHost 的清单热同步（FR-08-06：tools/list_changed → 重拉清单 → 热更新
// 注册表并重导 toolSpecs）。
impl McpHost {
    /// 清单热同步入口（订阅流与兜底回调共用）。generation 复核：旧连接
    /// 的迟到通知不改新状态；非 Ready/非期望保持（断开中）也不拉。
    pub(crate) async fn handle_tools_changed(&self, key: &ConnKey, generation: u64) {
        let current = {
            let inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inner.get(key).map(|e| (e.generation, e.desired, e.state))
        };
        let Some((entry_gen, desired, state)) = current else {
            return;
        };
        if entry_gen != generation || !desired || state != ConnState::Ready {
            return;
        }
        let snapshot = {
            let inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = inner.get(key);
            (
                entry.and_then(|e| e.pending_config.clone()),
                entry.and_then(|e| e.pending_fingerprint.clone()),
                entry.and_then(|e| e.live.as_ref().map(|l| l.peer.clone())),
            )
        };
        let (Some(config), Some(fingerprint), Some(peer)) = snapshot else {
            return;
        };
        if let Ok(tools) = list_tools(&peer, Duration::from_secs(30)).await {
            // 先算远端签名与本地比对：无变化不刷（轮询兜底路径的幂等面）
            let remote_signature = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                for (raw_name, _, schema, _) in &tools {
                    raw_name.hash(&mut hasher);
                    super::trust::schema_fingerprint(schema).hash(&mut hasher);
                }
                hasher.finish()
            };
            if self.registry().signature_of(key) == Some(remote_signature) {
                return;
            }
            let registered = self
                .registry()
                .sync(key.clone(), fingerprint.clone(), &config, tools);
            // 清单热同步的 always_allow 自动授权（Q44）：与连接建立同规——
            // schema 有未确认变更的工具不自动放行。
            if config.always_allow {
                self.auto_grant_licenses(key, &fingerprint);
            }
            self.events.emit(
                crate::util::events::keys::MCP_STATUS,
                serde_json::json!({
                    "scope": key.scope.as_str(),
                    "name": key.name,
                    "state": "ready",
                    "detail": format!("工具清单已更新（{} 个）", registered.len()),
                }),
            );
        }
    }
}
