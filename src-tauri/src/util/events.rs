//! 事件汇抽象（§3.5 事件定义）：Rust → 前端的单向事件（tauri event）。
//!
//! 放 util：backend（executor 的 ask/plan 事件）与 host（审批挂起、
//! suggest、tokens:update）都要发射，util 是被所有层依赖的最底层；
//! tauri 侧实现（`TauriEventSink`）在 host 装配，测试用
//! [`RecordingEventSink`] 收集断言。事件发射为尽力而为：失败不影响
//! run 主链路（帧 Channel 才是权威面）。

use std::sync::Mutex;

/// 事件键常量（§3.5 P0 子集；17 号票前实装 5 键，本票补 run 终态三键与
/// 通知/退出两键）。
pub mod keys {
    /// 询问/审批开启（AskPanel 弹出）。
    pub const ASK_OPENED: &str = "ask:opened";
    /// 已答/已取消。
    pub const ASK_CLOSED: &str = "ask:closed";
    /// plan 工具变更。
    pub const PLAN_UPDATE: &str = "plan:update";
    /// suggest 生成完成（FR-06-09）。
    pub const RUN_SUGGESTIONS: &str = "run:suggestions";
    /// 会话累计用量（FR-01-13 会话口径）。
    pub const TOKENS_UPDATE: &str = "tokens:update";
    /// run 正常终态（17 号票；系统通知触发点之一）。
    pub const RUN_DONE: &str = "run:done";
    /// run 异常终态。
    pub const RUN_ERROR: &str = "run:error";
    /// run 用户取消终态。
    pub const RUN_CANCELLED: &str = "run:cancelled";
    /// 系统通知点击回跳（§9.6：payload 带 sessionId/runId/anchor）。
    pub const NOTIFY_ACTIVATE: &str = "notify:activate";
    /// 退出请求待确认（存在活跃 run 的退出拦截，§9.6）。
    pub const APP_EXIT_REQUESTED: &str = "app:exit_requested";
}

/// 事件发射抽象：`emit(key, payload)`，payload 为已序列化 JSON。
pub trait EventSink: Send + Sync {
    fn emit(&self, key: &str, payload: serde_json::Value);
}

/// 测试用事件汇：按序收集 (key, payload) 供断言。
#[derive(Default)]
pub struct RecordingEventSink {
    events: Mutex<Vec<(String, serde_json::Value)>>,
}

impl RecordingEventSink {
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

impl EventSink for RecordingEventSink {
    fn emit(&self, key: &str, payload: serde_json::Value) {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((key.to_owned(), payload));
    }
}
