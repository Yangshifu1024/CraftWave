//! RunManager（§5.1）：会话 → 活跃 run 的注册表。
//!
//! - 同会话已有活跃 run 时 `start` 返回 E_SESSION_RUNNING（§3.3 #18）；
//! - `cancel` 置位 CancellationToken 即返回（§3.3 #19「置位即返回」），
//!   run 终态由 run 任务收尾调 [`RunManager::remove`] 摘除；
//! - RunState 快照可读（`session_run_snapshot` 对账命令为后续票，结构先就位）。

use std::collections::HashMap;
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;

use crate::util::errors::{E_SESSION_RUNNING, IpcError};

/// run 状态（§5.1 RunState 的 M1 子集）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    /// 终态帧已发、任务收尾中。
    Finishing,
}

#[derive(Debug, Clone)]
pub struct RunState {
    pub status: RunStatus,
    pub started_at: i64,
    pub turn: u32,
    pub tool_count: u32,
    pub last_seq: u64,
}

/// 单个活跃 run 的句柄（§5.1 RunHandle 的 M1 子集：帧通道由 run 任务独占，
/// seq 在帧出口递增；steer 队列与 ask 槽随对应票接入）。
#[derive(Debug)]
pub struct RunHandle {
    pub run_id: String,
    pub session_id: String,
    pub cancel: CancellationToken,
    pub state: Mutex<RunState>,
}

impl RunHandle {
    /// 快照一份状态（对账/测试用）。
    pub fn snapshot(&self) -> RunState {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// 会话 → 活跃 run 注册表（Clone = 同一注册表的共享句柄，供 run 任务收尾摘除）。
#[derive(Clone, Default)]
pub struct RunManager {
    /// session_id → 活跃 run（M1 单进程单实例，Mutex<HashMap> 足量）。
    runs: std::sync::Arc<Mutex<HashMap<String, std::sync::Arc<RunHandle>>>>,
}

impl RunManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册新 run：同会话已有活跃 run → E_SESSION_RUNNING。
    /// 返回句柄后由调用方 spawn run 任务（RunManager 不感知任务形态）。
    pub fn start(
        &self,
        session_id: &str,
        run_id: String,
        now_ms: i64,
    ) -> Result<std::sync::Arc<RunHandle>, IpcError> {
        let mut runs = self
            .runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runs.contains_key(session_id) {
            return Err(IpcError::new(
                E_SESSION_RUNNING,
                format!("会话已有活跃 run：{session_id}"),
            ));
        }
        let handle = std::sync::Arc::new(RunHandle {
            run_id,
            session_id: session_id.to_owned(),
            cancel: CancellationToken::new(),
            state: Mutex::new(RunState {
                status: RunStatus::Running,
                started_at: now_ms,
                turn: 0,
                tool_count: 0,
                last_seq: 0,
            }),
        });
        runs.insert(session_id.to_owned(), handle.clone());
        Ok(handle)
    }

    /// 取消：置位即返回是否命中活跃 run（§3.3 #19）。
    pub fn cancel(&self, session_id: &str) -> bool {
        let runs = self
            .runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match runs.get(session_id) {
            Some(handle) => {
                handle.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// 活跃 run 查询（对账快照面）。
    pub fn get(&self, session_id: &str) -> Option<std::sync::Arc<RunHandle>> {
        self.runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .cloned()
    }

    /// run 任务收尾摘除（终态帧发出后调用；run_id 校验防摘错新 run）。
    pub fn remove(&self, session_id: &str, run_id: &str) {
        let mut runs = self
            .runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if runs.get(session_id).is_some_and(|h| h.run_id == run_id) {
            runs.remove(session_id);
        }
    }
}

#[cfg(test)]
#[path = "run_manager_test.rs"]
mod run_manager_test;
