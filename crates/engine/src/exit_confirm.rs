//! 退出确认状态机（自 host/shell.rs 剥离：纯逻辑，壳侧托盘/退出拦截使用）。
//!
//! 计时入参显式传入（request/timed_out 各自带 now），可确定性单测
//! （见 engine 侧测试）；真实时间由壳的 request_exit / resolve_exit_request 注入。

use std::time::{Duration, Instant};

/// 退出确认超时：`app:exit_requested` 发出后前端在此时限
/// 内未回执即兜底放行退出。用户退出意图优先——退出流程不能因前端无响应
/// （webview 卡死/已崩溃）而永远退不出去；活跃 run 的数据安全由 append-only
/// 崩溃恢复语义兜底（重启后标「已中断」的既有行为），而非靠拦截退出。
pub const EXIT_CONFIRM_TIMEOUT: Duration = Duration::from_secs(10);

/// 退出确认状态机（挂起标志 + 超时兜底判定）。
pub struct ExitConfirm {
    /// Some(发出 app:exit_requested 的时刻) = 等待前端回执中。
    pending_since: Option<Instant>,
    timeout: Duration,
}

impl ExitConfirm {
    pub fn new(timeout: Duration) -> Self {
        Self {
            pending_since: None,
            timeout,
        }
    }

    /// 记录一次退出请求（重复触发刷新计时起点，与「下次再触发」语义一致）。
    pub fn request(&mut self, now: Instant) {
        self.pending_since = Some(now);
    }

    /// 前端回执：解除挂起，返回 accepted（是否放行退出）。
    pub fn resolve(&mut self, accepted: bool) -> bool {
        self.pending_since = None;
        accepted
    }

    /// 超时兜底判定：挂起中且已超时 → 放行（同时解除挂起，防兜底重复放行）。
    pub fn timed_out(&mut self, now: Instant) -> bool {
        match self.pending_since {
            Some(since) if now.duration_since(since) >= self.timeout => {
                self.pending_since = None;
                true
            }
            _ => false,
        }
    }

    /// 是否挂起中（等待前端回执）。
    pub fn is_pending(&self) -> bool {
        self.pending_since.is_some()
    }
}
