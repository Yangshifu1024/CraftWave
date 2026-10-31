//! 事件汇抽象（事件定义）：Rust → 前端的单向事件（tauri event）。
//!
//! 事件汇（EventSink）：后端向前端界面"推送通知"的出口抽象——后端各处只
//! 依赖本 trait 发事件，不直接耦合 tauri 运行时，测试可换成录音实现来断言。
//!
//! 放 util：backend（executor 的 ask/plan 事件）与 host（审批挂起、
//! suggest、tokens:update）都要发射，util 是被所有层依赖的最底层；
//! tauri 侧实现（`TauriEventSink`）在 host 装配，测试用
//! [`RecordingEventSink`] 收集断言。事件发射为尽力而为：失败不影响
//! run 主链路（帧 Channel 才是权威面）。

use std::sync::Mutex;

/// 事件键常量（P0 子集；前实装 5 键，此处补 run 终态三键与
/// 通知/退出两键）。
pub mod keys {
    /// run 任务确认启动（P0 键；实装——自动投递与手动
    /// start_chat 都发，前端以它驱动自动投递新 run 的观察态接入）。
    pub const RUN_START: &str = "run:start";
    /// 询问/审批开启（AskPanel 弹出）。
    pub const ASK_OPENED: &str = "ask:opened";
    /// 已答/已取消。
    pub const ASK_CLOSED: &str = "ask:closed";
    /// plan 工具变更。
    pub const PLAN_UPDATE: &str = "plan:update";
    /// suggest 生成完成。
    pub const RUN_SUGGESTIONS: &str = "run:suggestions";
    /// 会话累计用量（会话口径）。
    pub const TOKENS_UPDATE: &str = "tokens:update";
    /// run 正常终态（系统通知触发点之一）。
    pub const RUN_DONE: &str = "run:done";
    /// run 异常终态。
    pub const RUN_ERROR: &str = "run:error";
    /// run 用户取消终态。
    pub const RUN_CANCELLED: &str = "run:cancelled";
    /// steer 被消费（P0 键，实装：acceptedAtTurn + entryId/text）。
    pub const RUN_INJECT: &str = "run:inject";
    /// followUp 队列状态变化（增补键，：入队/出队/移除/
    /// 暂停/恢复即发；returned 仅清空返还时出现）。
    pub const RUN_QUEUE: &str = "run:queue";
    /// 系统通知点击回跳（payload 带 sessionId/runId/anchor）。
    pub const NOTIFY_ACTIVATE: &str = "notify:activate";
    /// 退出请求待确认（存在活跃 run 的退出拦截）。
    pub const APP_EXIT_REQUESTED: &str = "app:exit_requested";
    /// 压缩开始（Composer 上方指示条）。
    pub const RUN_COMPACTING: &str = "run:compacting";
    /// 压缩完成（载荷 { sessionId, tokensBefore, tokensAfter }）。
    pub const RUN_COMPACTED: &str = "run:compacted";
    /// 压缩失败（载荷 { sessionId }；失败兜底规则）。
    pub const RUN_COMPACT_FAILED: &str = "run:compact_failed";
    /// MCP 连接池状态变化（增补键，：payload
    /// { scope, name, state: "starting"|"ready"|"stopped"|"error", detail? }）。
    pub const MCP_STATUS: &str = "mcp:status";
    /// 子代理委派启动（增补键，：payload
    /// { sessionId, runId, subId, role }，跨 Tab Inspector 订阅）。
    pub const SUB_SPAWN: &str = "sub:spawn";
    /// 子代理步数推进（payload { sessionId, runId, subId, turn }）。
    pub const SUB_STEP: &str = "sub:step";
    /// 子代理结构化汇报产出（payload { sessionId, runId, subId, report }）。
    pub const SUB_REPORT: &str = "sub:report";
    /// 子代理用量增量（payload { sessionId, runId, subId, usage }；
    /// 仅子代理面板展示自身消耗，不重复累进会话累计——会话累计由 tokens:update
    /// 单一出口承担，父级汇总只计一次）。
    pub const SUB_USAGE: &str = "sub:usage";
    /// 子代理终态（payload { sessionId, runId, subId, outcome }，
    /// outcome ∈ report|partial|no_report|cancelled）。
    pub const SUB_DONE: &str = "sub:done";
    /// 子代理异常终态（payload { sessionId, runId, subId, code, message }）。
    pub const SUB_ERROR: &str = "sub:error";
    /// 自动标题生成完成（P0 键，实装：payload
    /// { sessionId, title }；时机 = meta 行 + index 双写落盘之后， 失败
    /// 静默——超时/失败走兜底标题也发本事件，只有"无标题可落"才不发）。
    pub const SESSION_TITLE: &str = "session:title";
}

/// 事件发射抽象：`emit(key, payload)`，payload 为已序列化 JSON。
pub trait EventSink: Send + Sync {
    /// 发射一个事件：`key` 取 [`keys`] 模块里的常量，payload 为 JSON 值。
    fn emit(&self, key: &str, payload: serde_json::Value);
}

/// 测试用事件汇：按序收集 (key, payload) 供断言。
#[derive(Default)]
pub struct RecordingEventSink {
    /// 已收事件列表（(键, 载荷) 按到达序）；Mutex 允许多线程发射。
    events: Mutex<Vec<(String, serde_json::Value)>>,
}

impl RecordingEventSink {
    /// 构造一个空的录音汇。
    pub fn new() -> Self {
        Self::default()
    }

    /// 快照已收事件。
    pub fn events(&self) -> Vec<(String, serde_json::Value)> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// 指定键的事件载荷（按到达序）。
    pub fn payloads_of(&self, key: &str) -> Vec<serde_json::Value> {
        self.events()
            .into_iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v)
            .collect()
    }
}

/// 录音实现：emit 即把事件追加进内部列表（另一线程 panic 导致锁中毒时也
/// 直接恢复使用，不让测试被连带卡死）。
impl EventSink for RecordingEventSink {
    /// 收下一个事件并入列。
    fn emit(&self, key: &str, payload: serde_json::Value) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((key.to_owned(), payload));
    }
}
