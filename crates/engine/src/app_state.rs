//! 应用级共享状态（自 host/mod.rs 剥离；gpui-rewrite 阶段 1）。
//!
//! 全部字段都是共享句柄（Arc 或内含 Arc），因此 [`AppState::share_handles`]
//! 能以零拷贝代价产出一份轻量副本供异步任务持有（run 收尾的 followUp
//! 自动投递要在此后调用 start_chat_core——命令层之外的第二个入口，
//! 此前各任务只捕获细粒度克隆件）。
//!
//! 壳（Tauri / GPUI）负责构造本状态并注入 [`util::events::EventSink`]
//! 的生产实现；窗口几何记忆属壳专属（Tauri WebView 窗口 / GPUI 窗口各自
//! 实现），不在本状态内。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::agent::RunManager;
use crate::backend::ask::{AppAskSlot, AskAnswer};
use crate::backend::frames::UsageTotal;
use crate::exit_confirm::{ExitConfirm, EXIT_CONFIRM_TIMEOUT};
use crate::followup::FollowUpManager;
use crate::providers::types::AppConfig;
use crate::storage::SessionStore;
use crate::storage::config_store::ConfigStore;
use crate::storage::project_store::ProjectStore;
use crate::usage::UsageLedger;
use crate::util::errors::IpcError;
use crate::util::events::EventSink;

/// 应用级共享状态。全部字段都是共享句柄。
pub struct AppState {
    /// 配置协调器：config.json 唯一写入口（API Key 明文承载，0600）。
    /// Arc 共享：通知门（壳侧）读同一份缓存，避免双实例缓存漂移。
    pub config: Arc<ConfigStore<AppConfig>>,
    /// 活跃 run 注册表（单会话核心链路）。内部 Arc<Mutex>，
    /// Clone 即同源句柄。
    pub runs: RunManager,
    /// 事件汇：生产由壳提供（tauri emit / GPUI 事件）；测试 RecordingEventSink。
    pub events: Arc<dyn EventSink>,
    /// 会话累计用量（tokens:update 的全量口径；run 帧增量折算 + suggest 后台小调用）。
    pub session_usage: Arc<Mutex<HashMap<String, UsageTotal>>>,
    /// 会话存储器（run 落盘器与命令层共享同一锁表/索引）。
    pub store: Arc<SessionStore>,
    /// 项目注册表。Arc 共享：run 任务内解析会话 roots 用。
    pub projects: Arc<ProjectStore>,
    /// UI 状态（Tab 布局等前端自管 Json）。
    pub ui_state: Arc<ConfigStore<Value>>,
    /// 活跃会话标识（前端 set_ui_state 顺带写入；通知防打扰精判共享）。
    /// headless 测试下无写入方，保持 None。
    pub active_session: Arc<Mutex<Option<String>>>,
    /// 退出确认状态机（退出拦截 + 超时兜底：request 置位并
    /// 起算，resolve 回执解除；超时未回执由 request_exit 排定的定时任务
    /// 判定后放行）。
    pub exit_confirm: Arc<Mutex<ExitConfirm>>,
    /// 会话级 followUp 队列（运行中排队续发 + 失败暂停 +
    /// 取消返还，`sessions/<sid>.queue.json` 持久化）。
    pub followups: FollowUpManager,
    /// MCP 宿主：连接池 + 启动信任 + 工具级许可。
    pub mcp: Arc<crate::mcp::pool::McpHost>,
    /// 应用级询问槽表（由 `mcp_asks` 泛化；backend/ask
    /// `AppAskSlot`）：不属任何 run 的询问（MCP TOFU、技能 projectTrust）
    /// 挂这里，resolve_ask 在 run 槽未命中时落到这里。
    pub app_asks: Arc<Mutex<HashMap<String, AppAskSlot>>>,
    /// 内置技能根的打包态路径（目录清单）：壳 setup 解析好传入；None =
    /// 走源码树回退（dev/headless）或无内置来源。
    pub resource_skills_dir: Option<PathBuf>,
    /// 子代理注册表：进程内全部活跃子代理 + 全局并发闸
    /// （所有会话共用上限，`agent.maxConcurrentSubagents`，缺省 4；
    /// save_config 时经 [`Self::apply_subagent_limit`] 热调整）。
    pub subs: Arc<crate::agent::SubRegistry>,
    /// 代理 HTTP 客户端工厂：进程级 reqwest::Client
    /// 缓存（none/system/manual 三模式，`AppConfig.proxy` 装配）。
    /// `resolve_model` 经它注入 rig（run 内复用同一 client）；
    /// save_config 时经 [`Self::apply_proxy_config`] 热重建（进行中 run
    /// 不切换——客户端按 run 构建，下一次 start_chat 用新代理）。
    pub http: Arc<crate::providers::http::HttpClientFactory>,
    /// 用量账本：日/模型聚合 + run/会话四口径明细 +
    /// stats_cache 查询缓存三面的聚合句柄（落账与查询编排见 `usage` 模块）。
    pub ledger: UsageLedger,
    /// 会话级审批记忆（always_session 升域）：全部活跃会话
    /// 的「始终允许（本会话）」签名桶。进程内单例、不持久化——应用退出
    /// 即清、会话删除随清（`forget`）。
    pub approval_memory: crate::backend::gate::ApprovalMemory,
    /// 会话级资源信任集（allow_session「本次会话允许」内存
    /// 面）：projectTrust 询问选「本次会话允许」后登记（技能与项目指令
    /// 共用，键含会话/作用域/资源名/指纹）。不持久化——重启失效、新会话
    /// 不延续。
    pub session_trust: crate::knowledge::session_trust::SessionTrustSet,
    /// 会话级计划基线扩展桶（范围门「本会话始终允许」）：
    /// 用户授权的计划外写入目录（canonical；目录含子路径），同会话主 run
    /// 与子代理共用。进程内单例、不持久化——应用退出即清、会话删除随清
    /// （`forget`）。
    pub plan_scopes: crate::backend::gate::PlanScopeMemory,
    /// 会话级日志器（日志布局）：`logs/app.log`
    /// 应用通道 + `logs/sessions/<sid>.jsonl` 会话通道，级别随
    /// `config.log.level` 热过滤；会话删除经 [`crate::session_log::SessionLogger::close_and_purge`]
    /// 联动清理专属日志与轮转副本。
    pub session_log: Arc<crate::session_log::SessionLogger>,
}

impl AppState {
    /// `~/.craftwave` 数据目录下的 config.json。
    pub fn new_in(data_dir: PathBuf, events: Arc<dyn EventSink>) -> Self {
        let store = Arc::new(SessionStore::with_root(data_dir.clone()));
        // MCP 宿主与 AppState 共享同一事件汇（克隆在字段 move 之前完成）
        let events_rcvd = events.clone();
        // 代理工厂初代 client 按已存配置装配；
        // 首次启动无配置 / 载入失败 → None = no_proxy 直连（防环境变量泄漏）
        let config_store: ConfigStore<AppConfig> = ConfigStore::new(data_dir.join("config.json"));
        let proxy = config_store.load().ok().and_then(|c| c.proxy.clone());
        // 配置缓存 Arc 提前构造：AppState.config 与会话日志器
        // （级别热过滤数据源）共享同一份——save_config 热替换后日志级别
        // 下一行即生效
        let config = Arc::new(config_store);
        let http = Arc::new(
            crate::providers::http::HttpClientFactory::from_config(proxy.as_ref()).unwrap_or_else(
                |_| {
                    crate::providers::http::HttpClientFactory::from_config(None)
                        .expect("无代理配置的客户端构建不会失败")
                },
            ),
        );
        Self {
            config: config.clone(),
            runs: RunManager::new(),
            events,
            session_usage: Arc::new(Mutex::new(HashMap::new())),
            store,
            projects: Arc::new(ProjectStore::new(&data_dir)),
            ui_state: Arc::new(ConfigStore::new(data_dir.join("ui-state.json"))),
            active_session: Arc::new(Mutex::new(None)),
            exit_confirm: Arc::new(Mutex::new(ExitConfirm::new(EXIT_CONFIRM_TIMEOUT))),
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
            ledger: UsageLedger::new(&data_dir),
            approval_memory: crate::backend::gate::ApprovalMemory::new(),
            session_trust: crate::knowledge::session_trust::SessionTrustSet::new(),
            plan_scopes: crate::backend::gate::PlanScopeMemory::new(),
            // 日志根（~/.craftwave/logs/）；config 缓存句柄
            // 复用上方已构造的同一份（save_config 热替换后级别即时生效）
            session_log: Arc::new(crate::session_log::SessionLogger::new(
                data_dir.join("logs"),
                config.clone(),
            )),
        }
    }

    /// 重绑配置缓存句柄（壳 setup 装配用）：外部传入的 config 实例是
    /// 唯一权威（通知门共享它），`AppState.config` 与会话日志器
    /// 必须指向同一实例——否则 save_config 的内存缓存热替换只到通知门、
    /// 日志级别过滤读不到新值（热生效失效）。日志根目录不变，只换缓存源。
    pub fn rebind_config(&mut self, config: Arc<ConfigStore<AppConfig>>) {
        let logs_dir = self.session_log.root().to_path_buf();
        self.config = config.clone();
        self.session_log = Arc::new(crate::session_log::SessionLogger::new(logs_dir, config));
    }

    /// 把配置里的子代理全局并发上限热应用到注册表（save_config 边界调用；
    /// 热调整语义：已启动任务继续，降低暂停补入，提高立即放行排队任务）。
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

    /// 把配置里的代理设置热应用到 HTTP 客户端工厂
    /// （save_config 边界调用——保存成功即重建缓存 client，下一次
    /// start_chat 的模型请求用新代理；进行中 run 不切换，客户端按 run
    /// 构建）。归一化已校验 proxy.url，重建失败属异常：保留旧 client 并
    /// 上抛（前端可见"保存成功但代理切换失败"）。
    pub fn apply_proxy_config(&self) -> Result<(), IpcError> {
        let proxy = self.config.load()?.proxy.clone();
        self.http.reload(proxy.as_ref())
    }

    /// 产出一份全部字段同源的轻量副本（Arc/内部 Arc 克隆，零深拷贝）——
    /// 异步任务（run 收尾的 followUp 自动投递）经它持有完整状态
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
            plan_scopes: self.plan_scopes.clone(),
            session_log: self.session_log.clone(),
        }
    }

    /// 应用级询问槽的应答投递（resolve_ask 的 run 槽未命中兜底；由
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

    /// 累计会话用量并返回累计后全量（tokens:update 载荷，会话口径）。
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
