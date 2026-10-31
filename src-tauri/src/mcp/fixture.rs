//! 测试基建：本地 in-process MCP echo server。
//!
//! 「外部依赖一律 fixture/录制回放——MCP 用本地 echo server」（spec Testing
//! Decisions）：本模块提供一个 `rmcp` 进程内 server（stdio 行式 JSON-RPC
//! 走 tokio 内存管道），与客户端同进程、零网络。能力面：
//! - `echo` / `add` 两个固定工具（调用回喂 + 执行计数）；
//! - `add_tool` 控制工具：动态新增一个工具并触发 `tools/list_changed`
//!   通知（清单热同步路径）；
//! - `die_after_record` + [`EchoFixture::arm_die_after_record`]：记录已执行
//!   操作后**不返回响应**（挂住，等测试侧 [`EchoFixture::kill_server`]）
//! ——精确复现「记录已执行操作后、返回响应前断连」的 场景；
//! - [`EchoFixture::kill_server`]：服务端主动断连（模拟 server 崩溃/被
//!   kill），触发客户端断线自动重连与 `mcp:status` 状态迁移。
//!
//! 连接注入：[`EchoFixture::install`] 按配置名把 fixture 挂进注册表，池的
//! 测试钩子（`pool::open_connection` 的 `cfg(test)` 分支）命中即建立新
//! 内存管道 + 新 server 实例——真实重连语义（新连接、新清单同步）与
//! 生产一致。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ListToolsResult, PaginatedRequestParams,
    Tool,
};
use rmcp::service::{RoleServer, RunningService, ServiceExt};

use super::config::McpServerConfig;
use super::pool::{ListChangedHandler, LiveConn};

/// 已注册的 fixture 表（键 = 配置的 command 字段名；fixture 场景一服务器
/// 一注册项）。
static REGISTRY: OnceLock<Mutex<HashMap<String, Arc<EchoFixture>>>> = OnceLock::new();

/// fixture 全链测试的跨文件串行锁（McpToolRegistry 是进程级单例——
/// 归一名「echo」等全局查重，多个 fixture 测试并发会互相改名/串许可；
/// 全部全链测试须持 [`test_lock_guard`] 运行）。tokio Mutex：guard 本就
/// 设计为跨 await 持有整个测试体（std Mutex 的 guard 跨 await 会触发
/// clippy::await_holding_lock，多线程轮询下还有死锁面）。
pub(crate) static TEST_CHAIN_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

/// 取测试串行锁（tokio Mutex 无 poison；guard 持有到测试体结束）。
pub(crate) async fn test_lock_guard() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_CHAIN_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

/// 注册/替换一个 fixture（配置名 → fixture；池测试钩子按名消费）。
pub fn install(fixture: &Arc<EchoFixture>, config_name: &str) {
    let table = REGISTRY.get_or_init(|| Mutex::new(HashMap::new()));
    table
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(config_name.to_owned(), fixture.clone());
}

/// 池测试钩子入口：按配置名建立一条进程内连接（未注册 → None 走真实
/// 传输）。每次调用（每次连接/自动重连）都建新管道 + 新 server 实例；
/// `handler` 为池侧的清单变更处理器（客户端握手的 owner，重连时各带
/// 各的 generation）。
pub(crate) async fn take_transport(
    config: &McpServerConfig,
    handler: ListChangedHandler,
) -> Option<LiveConn> {
    let command = config.command.as_ref()?;
    let fixture = REGISTRY
        .get()?
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(command)?
        .clone();
    match connect_in_process(&fixture, handler).await {
        Ok(live) => Some(live),
        Err(e) => panic!("fixture transport 建立失败：{e}"),
    }
}

/// echo server 的共享状态（运行行为开关；测试断言面在 EchoFixture）。
#[derive(Default)]
pub struct EchoState {
    /// 已执行操作记录（按序；与 call_count 同源）。
    pub calls: Mutex<Vec<String>>,
    /// 动态新增的工具名（`add_tool` 控制；list_tools 输出随行）。
    pub extra_tools: Mutex<Vec<String>>,
    /// 下一次调用记录后不返回响应（die_after_record 生效开关）。
    pub die_on_next: AtomicBool,
}

/// 进程内 echo fixture：server 状态 + 活跃 server 服务句柄（kill_server 用）。
pub struct EchoFixture {
    /// 共享状态（server handler 与测试断言同源）。
    pub state: Arc<EchoState>,
    /// 已执行操作数（原子面； 断言「调用计数保持 1」的权威面）。
    pub call_count: Arc<AtomicUsize>,
    /// 活跃 server 服务（每次连接一个；take + cancel = 服务端断连）。
    servers: Mutex<Vec<RunningService<RoleServer, EchoServer>>>,
}

// EchoFixture 的构造与控制面。
impl EchoFixture {
    /// 构造（状态即断言面）。
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(EchoState::default()),
            call_count: Arc::new(AtomicUsize::new(0)),
            servers: Mutex::new(Vec::new()),
        })
    }

    /// 已执行操作数。
    pub fn count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    /// 已执行操作记录快照（按序）。
    pub fn calls(&self) -> Vec<String> {
        self.state
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// 预约：下一次调用记录后不返回响应（「记录已执行操作后、返回响应
    /// 前断连」的 server 侧配合面——挂住等待测试 kill_server）。
    pub fn arm_die_after_record(&self) {
        self.state.die_on_next.store(true, Ordering::SeqCst);
    }

    /// 服务端主动断连（模拟 server 崩溃/被 kill）：取消全部活跃 server
    /// 服务 → 内存管道对端关闭 → 客户端 TransportClosed → 自动重连。
    pub fn kill_server(&self) {
        let mut servers = self
            .servers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for service in servers.drain(..) {
            service.cancellation_token().cancel();
        }
    }

    /// 注册进程内 transport（池测试钩子按配置名消费）。
    pub fn install(self: &Arc<Self>, config_name: &str) {
        super::fixture::install(self, config_name);
    }

    /// 预约一个动态工具（下一次 list_tools 即出现；不经 add_tool 调用面）。
    pub fn add_tool_name(&self, name: &str) {
        self.state
            .extra_tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(name.to_owned());
    }
}

/// 进程内连接建立：起 server 端（句柄留存供 kill_server）+ 返回 client 侧
/// 裸管道（pool 的 open_connection 测试分支据此走完整握手——与生产路径
/// 唯一差异是字节走内存而非子进程 stdio）。
pub(crate) async fn connect_in_process(
    fixture: &Arc<EchoFixture>,
    handler: ListChangedHandler,
) -> Result<LiveConn, String> {
    let server = EchoServer {
        state: fixture.state.clone(),
        call_count: fixture.call_count.clone(),
    };
    let (client_side, server_side) = tokio::io::duplex(64 * 1024);
    let (c1_read, c1_write) = tokio::io::split(server_side);
    let (c2_read, c2_write) = tokio::io::split(client_side);
    // 双端并发握手（rmcp 的 serve 双向等待 initialize——串行会互等死锁）：
    // server 端 spawn，client 端原地等（两者经内存管道互答）。
    let server_task = tokio::spawn(server.serve((c1_read, c1_write)));
    let client_service = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        handler.serve((c2_read, c2_write)),
    )
    .await
    .map_err(|_| "fixture client 握手超时".to_owned())?
    .map_err(|e| format!("fixture client 握手失败：{e}"))?;
    let server_service = tokio::time::timeout(std::time::Duration::from_secs(10), server_task)
        .await
        .map_err(|_| "fixture server 起动超时".to_owned())?
        .map_err(|e| format!("fixture server 任务失败：{e}"))?
        .map_err(|e| format!("fixture server 起动失败：{e}"))?;
    fixture
        .servers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(server_service);
    Ok(LiveConn::from_service(client_service, None))
}

/// 工具 schema 小助手（JSON Schema 对象；Tool 是 non-exhaustive 结构，
/// 走 Default + 字段赋值）。
fn tool(name: &str, description: &str, properties: serde_json::Value, required: &[&str]) -> Tool {
    let mut t = Tool::default();
    t.name = name.to_owned().into();
    t.description = Some(description.to_owned().into());
    t.input_schema = Arc::new(
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
        })
        .as_object()
        .cloned()
        .expect("字面量为对象"),
    );
    t
}

use std::sync::Arc as StdArc;

/// server 实现：echo/add/add_tool/die_after_record 四工具（手写
/// ServerHandler，不依赖宏——工具集小且需要动态清单）。
#[derive(Clone)]
pub struct EchoServer {
    /// 共享状态。
    state: StdArc<EchoState>,
    /// 执行计数（原子面）。
    call_count: StdArc<AtomicUsize>,
}

// EchoServer 的构造（HTTP loopback 测试复用同一 server 工具面）。
impl EchoServer {
    /// 用共享状态与执行计数构造（与 [`EchoFixture`] 的进程内路径同源形状）。
    pub fn new(state: StdArc<EchoState>, call_count: StdArc<AtomicUsize>) -> Self {
        Self { state, call_count }
    }
}

impl ServerHandler for EchoServer {
    /// 工具清单（固定两个 + 控制两个 + 动态新增）。
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, rmcp::model::ErrorData>> + Send + '_
    {
        let extra = self
            .state
            .extra_tools
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut tools = vec![
            tool(
                "echo",
                "回显输入的 message 字段",
                serde_json::json!({ "message": { "type": "string" } }),
                &["message"],
            ),
            tool(
                "add",
                "返回 a + b 的和",
                serde_json::json!({ "a": { "type": "number" }, "b": { "type": "number" } }),
                &["a", "b"],
            ),
            tool(
                "add_tool",
                "控制面：动态新增一个工具并触发 tools/list_changed 通知",
                serde_json::json!({ "name": { "type": "string" } }),
                &["name"],
            ),
            tool(
                "die_after_record",
                "控制面：记录已执行操作后不返回响应（测试结果未知语义）",
                serde_json::json!({}),
                &[],
            ),
        ];
        for name in extra {
            tools.push(tool(&name, "动态新增的工具", serde_json::json!({}), &[]));
        }
        std::future::ready(Ok(ListToolsResult::with_all_items(tools)))
    }

    /// 工具调用：记录 → 分派 →（die 开关挂住不返回）。
    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: rmcp::service::RequestContext<RoleServer>,
    ) -> impl std::future::Future<
        Output = Result<rmcp::model::CallToolResponse, rmcp::model::ErrorData>,
    > + Send
    + '_ {
        // 记录（断言的权威面：计数只进一次）
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.state
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.name.to_string());
        let die = self.state.die_on_next.swap(false, Ordering::SeqCst);
        let name = request.name.to_string();
        let args = request.arguments.clone().unwrap_or_default();
        // add_tool：记录后向客户端发 tools/list_changed 通知（清单热同步
        // 路径；peer 由请求上下文给出）。
        let notify_peer = (name == "add_tool").then(|| context.peer.clone());
        async move {
            let text = match name.as_str() {
                "echo" => {
                    let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("");
                    format!("echo: {message}")
                }
                "add" => {
                    let a = args.get("a").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    let b = args.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0);
                    format!("{}", a + b)
                }
                "add_tool" => {
                    let new_name = args
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("dynamic_tool")
                        .to_owned();
                    self.state
                        .extra_tools
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(new_name.clone());
                    // 触发通知（发送失败不影响响应——fixture 尽力而为）
                    if let Some(peer) = notify_peer {
                        let notification = rmcp::model::ToolListChangedNotification {
                            method: rmcp::model::ToolListChangedNotificationMethod,
                            extensions: Default::default(),
                        };
                        let _ = peer
                            .send_notification(
                                rmcp::model::ServerNotification::ToolListChangedNotification(
                                    notification,
                                ),
                            )
                            .await;
                    }
                    format!("added {new_name}")
                }
                "die_after_record" => {
                    if die {
                        // 挂住不返回：测试侧 kill_server 后响应随连接消亡，
                        // 客户端收 TransportClosed（结果未知）。
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    }
                    String::from("survived")
                }
                other => format!("unknown tool {other}"),
            };
            Ok(rmcp::model::CallToolResponse::Complete(
                CallToolResult::success(vec![ContentBlock::text(text)]),
            ))
        }
    }
}
