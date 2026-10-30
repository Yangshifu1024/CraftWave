//! host 模块（§2.2）：IPC 层——命令实现（校验/转调）、事件汇、通知、托盘、菜单、退出拦截。
//!
//! 已落地：会话树两命令（10 号票，commands/session_tree.rs）、provider/config
//! 域 P0 命令（§3.3 #6–9）、start_chat/cancel_run（§3.3 #18/#19，13 号票，
//! commands/chat.rs）、resolve_ask/approve_plan（§3.3 #22/#26c，14 号票）、
//! AppState（数据目录 §7.1 + 协调器 + RunManager + 事件汇 + 会话用量累计）。
//! 2026-10-28：keyring 废除（§9.4 改写），API Key 明文存 config.json（0600）。
//! `IpcError` 本体在 `util::errors`（providers 亦依赖，util 为最底层）；
//! error.rs 仅存 `From<StorageError>` 映射。IPC manifest 单一事实源在
//! crate 根 lib.rs（`ipc_commands!` 宏，§12.3/NFR-12 防漂移）。

pub mod commands;
mod error;
pub mod followup;
pub mod notify;
pub mod run_persistence;
pub mod shell;
pub mod usage;

#[cfg(test)]
#[path = "run_persistence_test.rs"]
mod run_persistence_test;

pub use crate::util::errors::IpcError;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::agent::RunManager;
use crate::backend::ask::{AppAskSlot, AskAnswer};
use crate::backend::frames::UsageTotal;
use crate::host::followup::FollowUpManager;
use crate::providers::types::AppConfig;
use crate::storage::SessionStore;
use crate::storage::config_store::ConfigStore;
use crate::storage::project_store::ProjectStore;
use crate::util::events::{EventSink, keys};

/// 应用级共享状态（tauri manage）。全部字段都是共享句柄（Arc 或内含 Arc），
/// 因此 [`AppState::share_handles`] 能以零拷贝代价产出一份轻量副本供异步
/// 任务持有（06 号票：run 收尾的 followUp 自动投递要在此后调用
/// start_chat_core——命令层之外的第二个入口，此前各任务只捕获细粒度克隆件）。
pub struct AppState {
    /// 配置协调器（§7.6）：config.json 唯一写入口（API Key 明文承载，0600）。
    /// Arc 共享：17 号票起通知门（notify.rs）读同一份缓存，避免双实例缓存漂移。
    pub config: Arc<ConfigStore<AppConfig>>,
    /// 活跃 run 注册表（§5.1；M1 单会话核心链路，13 号票）。内部 Arc<Mutex>，
    /// Clone 即同源句柄。
    pub runs: RunManager,
    /// 事件汇（§3.5）：生产 tauri emit；测试 RecordingEventSink（14 号票）。
    pub events: Arc<dyn EventSink>,
    /// 会话累计用量（tokens:update 的全量口径；run 帧增量折算 + suggest 后台小调用）。
    pub session_usage: Arc<Mutex<HashMap<String, UsageTotal>>>,
    /// 会话存储器（15 号票：run 落盘器与命令层共享同一锁表/索引）。
    pub store: Arc<SessionStore>,
    /// 项目注册表（FR-10-01，16 号票）。Arc 共享：run 任务内解析会话 roots 用。
    pub projects: Arc<ProjectStore>,
    /// UI 状态（§3.3 #30/31，16 号票：Tab 布局等前端自管 Json）。
    pub ui_state: Arc<ConfigStore<Value>>,
    /// 活跃会话标识（前端 set_ui_state 顺带写入；通知防打扰精判共享，
    /// 17 号票）。headless 测试下无写入方，保持 None。
    pub active_session: Arc<Mutex<Option<String>>>,
    /// 退出确认状态机（§9.6 退出拦截 + 19 号票超时兜底：request 置位并
    /// 起算，resolve 回执解除；超时未回执由 request_exit 排定的定时任务
    /// 判定后放行）。
    pub exit_confirm: Arc<Mutex<shell::ExitConfirm>>,
    /// 会话级 followUp 队列（§5.5，06 号票：运行中排队续发 + 失败暂停 +
    /// 取消返还，`sessions/<sid>.queue.json` 持久化）。
    pub followups: FollowUpManager,
    /// MCP 宿主（08 号票，§9.7）：连接池 + 启动信任 + 工具级许可。
    pub mcp: Arc<crate::mcp::pool::McpHost>,
    /// 应用级询问槽表（09 号票由 08 号票 `mcp_asks` 泛化；backend/ask
    /// `AppAskSlot`）：不属任何 run 的询问（MCP TOFU、技能 projectTrust）
    /// 挂这里，resolve_ask 在 run 槽未命中时落到这里。
    pub app_asks: Arc<Mutex<HashMap<String, AppAskSlot>>>,
    /// 内置技能根的打包态路径（09 号票，§9.8 目录清单）：lib.rs setup 经
    /// AppHandle 的 Resource 目录解析好传入；None = 走源码树回退（dev/
    /// headless）或无内置来源。
    pub resource_skills_dir: Option<PathBuf>,
    /// 子代理注册表（10 号票，§5.9）：进程内全部活跃子代理 + 全局并发闸
    /// （所有会话共用上限，`agent.maxConcurrentSubagents`，缺省 4；
    /// save_config 时经 [`Self::apply_subagent_limit`] 热调整）。
    pub subs: Arc<crate::agent::SubRegistry>,
    /// 代理 HTTP 客户端工厂（19 号票，FR-02-10）：进程级 reqwest::Client
    /// 缓存（none/system/manual 三模式，`AppConfig.proxy` 装配）。
    /// `resolve_model` 经它注入 rig（run 内复用同一 client，§11.1）；
    /// save_config 时经 [`Self::apply_proxy_config`] 热重建（进行中 run
    /// 不切换——客户端按 run 构建，下一次 start_chat 用新代理）。
    pub http: Arc<crate::providers::http::HttpClientFactory>,
    /// 用量账本（20 号票，§7.7）：日/模型聚合 + run/会话四口径明细 +
    /// stats_cache 查询缓存三面的聚合句柄（落账与查询编排见 `usage` 模块）。
    pub ledger: usage::UsageLedger,
    /// 会话级审批记忆（23 号票，§3.2 always_session 升域）：全部活跃会话
    /// 的「始终允许（本会话）」签名桶。进程内单例、不持久化——应用退出
    /// 即清、会话删除随清（`forget`）、备份恢复经 `clear_all` 失效（Q41，
    /// 恢复流程接线由 33 号票负责）。
    pub approval_memory: crate::backend::gate::ApprovalMemory,
    /// 会话级资源信任集（24 号票，§9.8 allow_session「本次会话允许」内存
    /// 面）：projectTrust 询问选「本次会话允许」后登记（技能与项目指令
    /// 共用，键含会话/作用域/资源名/指纹）。不持久化——重启失效、新会话
    /// 不延续、备份恢复天然不复活（Q41）。
    pub session_trust: crate::knowledge::session_trust::SessionTrustSet,
}

impl AppState {
    /// `~/.craftwave` 数据目录（§7.1）下的 config.json。
    pub fn new_in(data_dir: PathBuf, events: Arc<dyn EventSink>) -> Self {
        let store = Arc::new(SessionStore::with_root(data_dir.clone()));
        // MCP 宿主与 AppState 共享同一事件汇（克隆在字段 move 之前完成）
        let events_rcvd = events.clone();
        // 代理工厂初代 client 按已存配置装配（19 号票 FR-02-10）；
        // 首次启动无配置 / 载入失败 → None = no_proxy 直连（防环境变量泄漏）
        let config_store: ConfigStore<AppConfig> = ConfigStore::new(data_dir.join("config.json"));
        let proxy = config_store.load().ok().and_then(|c| c.proxy.clone());
        let http = Arc::new(
            crate::providers::http::HttpClientFactory::from_config(proxy.as_ref()).unwrap_or_else(
                |_| {
                    crate::providers::http::HttpClientFactory::from_config(None)
                        .expect("无代理配置的客户端构建不会失败")
                },
            ),
        );
        Self {
            config: Arc::new(config_store),
            runs: RunManager::new(),
            events,
            session_usage: Arc::new(Mutex::new(HashMap::new())),
            store,
            projects: Arc::new(ProjectStore::new(&data_dir)),
            ui_state: Arc::new(ConfigStore::new(data_dir.join("ui-state.json"))),
            active_session: Arc::new(Mutex::new(None)),
            exit_confirm: Arc::new(Mutex::new(shell::ExitConfirm::new(
                shell::EXIT_CONFIRM_TIMEOUT,
            ))),
            followups: FollowUpManager::new(data_dir.clone()),
            mcp: Arc::new(crate::mcp::pool::McpHost::new(
                data_dir.clone(),
                events_rcvd,
            )),
            app_asks: Arc::new(Mutex::new(HashMap::new())),
            resource_skills_dir: None,
            subs: Arc::new(crate::agent::SubRegistry::new(
                crate::providers::types::DEFAULT_MAX_CONCURRENT_SUBAGENTS as usize,
            )),
            http,
            ledger: usage::UsageLedger::new(&data_dir),
            approval_memory: crate::backend::gate::ApprovalMemory::new(),
            session_trust: crate::knowledge::session_trust::SessionTrustSet::new(),
        }
    }

    /// 把配置里的子代理全局并发上限热应用到注册表（save_config 边界调用；
    /// Q13 热调整语义：已启动任务继续，降低暂停补入，提高立即放行排队任务）。
    pub fn apply_subagent_limit(&self) {
        let limit = self
            .config
            .load()
            .map(|c| {
                c.agent
                    .as_ref()
                    .map(|a| a.max_concurrent_subagents)
                    .unwrap_or(crate::providers::types::DEFAULT_MAX_CONCURRENT_SUBAGENTS)
            })
            .unwrap_or(crate::providers::types::DEFAULT_MAX_CONCURRENT_SUBAGENTS);
        self.subs.gate().set_limit(limit.max(1) as usize);
    }

    /// 把配置里的代理设置热应用到 HTTP 客户端工厂（19 号票 FR-02-10：
    /// save_config 边界调用——保存成功即重建缓存 client，下一次
    /// start_chat 的模型请求用新代理；进行中 run 不切换，客户端按 run
    /// 构建）。归一化已校验 proxy.url，重建失败属异常：保留旧 client 并
    /// 上抛（前端可见"保存成功但代理切换失败"）。
    pub fn apply_proxy_config(&self) -> Result<(), IpcError> {
        let proxy = self.config.load()?.proxy.clone();
        self.http.reload(proxy.as_ref())
    }

    /// 产出一份全部字段同源的轻量副本（Arc/内部 Arc 克隆，零深拷贝）——
    /// 异步任务（run 收尾的 followUp 自动投递，06 号票）经它持有完整状态
    /// 面，可直接调用 start_chat_core 开新 run。
    pub fn share_handles(&self) -> AppState {
        Self {
            config: self.config.clone(),
            runs: self.runs.clone(),
            events: self.events.clone(),
            session_usage: self.session_usage.clone(),
            store: self.store.clone(),
            projects: self.projects.clone(),
            ui_state: self.ui_state.clone(),
            active_session: self.active_session.clone(),
            exit_confirm: self.exit_confirm.clone(),
            followups: self.followups.clone(),
            mcp: self.mcp.clone(),
            app_asks: self.app_asks.clone(),
            resource_skills_dir: self.resource_skills_dir.clone(),
            subs: self.subs.clone(),
            http: self.http.clone(),
            ledger: self.ledger.clone(),
            approval_memory: self.approval_memory.clone(),
            session_trust: self.session_trust.clone(),
        }
    }

    /// 应用级询问槽的应答投递（resolve_ask 的 run 槽未命中兜底；09 号票由
    /// mcp::commands::resolve_trust_ask 泛化收编）。命中即取走槽位（oneshot
    /// 发送端不可克隆；取走后等待方收到应答并自清 ask:closed）。
    pub fn resolve_app_ask(&self, ask_id: &str, answer: AskAnswer) -> bool {
        let tx = self
            .app_asks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(ask_id)
            .map(|slot| slot.tx);
        match tx {
            Some(tx) => tx.send(answer).is_ok(),
            None => false,
        }
    }

    /// 累计会话用量并返回累计后全量（tokens:update 载荷，FR-01-13 会话口径）。
    pub fn accumulate_usage(&self, session_id: &str, delta: &UsageTotal) -> UsageTotal {
        let mut map = self
            .session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let total = map.entry(session_id.to_owned()).or_default();
        total.accumulate(delta);
        *total
    }
}

/// 生产事件汇：tauri `Emitter::emit`（尽力而为，失败不影响 run 主链路）；
/// 对通知触发键（run 终态三键 + ask:opened）旁路系统通知门（§9.6，17 号票）。
pub struct TauriEventSink {
    pub app: tauri::AppHandle,
    pub notify: Arc<notify::NotifyGate>,
}

impl EventSink for TauriEventSink {
    /// 双写出口：先照常 emit 给前端，再按事件键旁路系统通知门
    /// （通知触发判定收在通知门内部，这里只做路由分发）。
    fn emit(&self, key: &str, payload: serde_json::Value) {
        use tauri::Emitter;
        let _ = self.app.emit(key, payload.clone());
        // 通知旁路：只认四个触发键，payload 字段缺失时静默跳过
        let session_id = payload.get("sessionId").and_then(|v| v.as_str());
        let run_id = payload.get("runId").and_then(|v| v.as_str());
        match key {
            keys::ASK_OPENED => {
                if let (Some(session_id), Some(ask_id)) =
                    (session_id, payload.get("askId").and_then(|v| v.as_str()))
                {
                    self.notify.ask_opened(session_id, ask_id);
                }
            }
            keys::RUN_DONE | keys::RUN_ERROR | keys::RUN_CANCELLED => {
                if let (Some(session_id), Some(run_id)) = (session_id, run_id) {
                    let reason = match key {
                        keys::RUN_DONE => "done",
                        keys::RUN_ERROR => "error",
                        _ => "cancelled",
                    };
                    let duration_ms = payload
                        .get("durationMs")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    self.notify
                        .run_ended(session_id, run_id, reason, duration_ms);
                }
            }
            _ => {}
        }
    }
}
