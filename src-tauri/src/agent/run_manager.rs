//! RunManager：维护"会话 → 活跃 run"对应关系的注册表（技术设计文档 §5.1）。
//!
//! run 指一次模型任务的完整执行过程；本注册表记录每个会话当前是否有正在
//! 运行的 run，并保证三件事：
//! - 同一会话已有活跃 run 时，再次 `start` 返回错误 E_SESSION_RUNNING
//!   （§3.3 第 18 条：同一会话不允许并行两个 run）；
//! - `cancel` 只把取消令牌（CancellationToken，异步协作取消的标准原语，
//!   置位后所有监听它的任务都会感知到取消）置位就立刻返回（§3.3 第 19 条
//!   "置位即返回"），run 的真正收尾由 run 任务自己做，做完后调用
//!   [`RunManager::remove`] 把自己从注册表摘除；
//! - RunState（run 的运行时状态快照）随时可读；对账快照命令
//!   （`session_run_snapshot`，用于前后端互相核对状态是否一致）为后续
//!   工单，数据结构先就位。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::backend::exec::AskSlotCell;
use crate::tools::plan::PlanState;
use crate::util::errors::{E_SESSION_RUNNING, IpcError};

/// run 的生命周期状态（技术设计文档 §5.1 完整状态机在当前阶段的简化版）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// 运行中：模型循环仍在推进，帧（后端发给前端的事件消息）持续产生。
    Running,
    /// 收尾中：终态帧（runEnded）已发出，后台任务正在做最后的清理。
    Finishing,
}

/// run 的运行时状态快照（多线程共享，放在互斥锁里读写）。
#[derive(Debug, Clone)]
pub struct RunState {
    /// 当前所处阶段（运行中/收尾中）。
    pub status: RunStatus,
    /// run 的开始时刻（Unix 纪元毫秒）。
    pub started_at: i64,
    /// 当前轮次编号（turn = 一次"请求模型并收到完整回复"的过程，从 1 起）。
    pub turn: u32,
    /// 本 run 已执行的工具调用次数。
    pub tool_count: u32,
    /// 已发出的最后一帧序号（seq 为全 run 递增的流水号，前端按它排序去重）。
    pub last_seq: u64,
}

/// 单个活跃 run 的句柄：持有取消令牌与运行时状态，供宿主命令层操作这个
/// run（技术设计文档 §5.1 的当前阶段子集：帧通道由 run 任务独占，帧序号在
/// 帧出口统一递增）。用户插话队列（steer）为后续工单；ask 槽与计划状态由
/// backend 执行器写入、宿主命令 resolve_ask/approve_plan 读取。
#[derive(Debug)]
pub struct RunHandle {
    /// 本 run 的唯一标识。
    pub run_id: String,
    /// 所属会话的标识。
    pub session_id: String,
    /// 取消令牌：cancel_run 命令置位，run 任务监听它感知取消。
    pub cancel: CancellationToken,
    /// 运行时状态快照（互斥锁保护，供状态读取与统计更新）。
    pub state: Mutex<RunState>,
    /// 当前未决 ask（ask = 模型向用户发起的提问；oneshot 回执指"只能回传
    /// 一次应答"的异步通道，用户应答经它唤醒等待中的 run；见 §5.1 ask 槽）。
    pub ask: Arc<AskSlotCell>,
    /// 计划状态（模型用 plan 工具写 / approve_plan 命令冻结 / 审批门读取）。
    pub plan: Arc<Mutex<Option<PlanState>>>,
}

/// RunHandle 的固有方法实现。
impl RunHandle {
    /// 拷贝一份当前状态（对账比对与测试用；对账 = 前后端各自记录的状态
    /// 互相核对）。锁若被其他线程的 panic 污染，也直接取出内部数据继续用
    /// ——状态是纯内存结构，恢复使用比让后续操作全部失败更合理。
    pub fn snapshot(&self) -> RunState {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// 会话 → 活跃 run 的注册表。Clone 得到的是指向同一份注册表的共享句柄，
/// 因此 run 任务收尾时能调 [`RunManager::remove`] 摘除自己。
#[derive(Clone, Default)]
pub struct RunManager {
    /// 会话 id → 活跃 run 句柄。单进程单实例部署下，一把互斥锁包住
    /// HashMap 即可满足并发要求（没有跨进程竞争）。
    runs: std::sync::Arc<Mutex<HashMap<String, std::sync::Arc<RunHandle>>>>,
}

/// RunManager 的注册/查询/摘除操作。
impl RunManager {
    /// 创建空注册表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册新 run：同会话已有活跃 run 时返回错误 E_SESSION_RUNNING。
    /// 返回句柄后由调用方自行启动 run 异步任务（本注册表只管登记，
    /// 不关心任务具体怎么跑）。
    pub fn start(
        &self,
        session_id: &str,
        run_id: String,
        now_ms: i64,
    ) -> Result<std::sync::Arc<RunHandle>, IpcError> {
        // 拿注册表的互斥锁；若锁被其他线程的 panic 污染（poisoned）也直接
        // 取出内部数据继续用——注册表是纯内存结构，没有需要回滚的状态，
        // 恢复使用比让后续所有操作都失败更合理。本文件各处锁同此策略。
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
            ask: Arc::new(Mutex::new(None)),
            plan: Arc::new(Mutex::new(None)),
        });
        runs.insert(session_id.to_owned(), handle.clone());
        Ok(handle)
    }

    /// 取消指定会话的 run：只把取消令牌置位就立刻返回（§3.3 第 19 条，
    /// 不等待收尾完成），返回值表示是否命中了活跃 run。
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

    /// 查询会话当前的活跃 run 句柄（对账快照与命令路由用）。
    pub fn get(&self, session_id: &str) -> Option<std::sync::Arc<RunHandle>> {
        self.runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .cloned()
    }

    /// 是否还有任一活跃 run（应用退出前的拦截判定，§9.6：有活跃 run 时不直接退出）。
    pub fn has_active(&self) -> bool {
        !self
            .runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    /// 按未决询问的 askId 反查持有它的 run（resolve_ask 命令用它把应答
    /// 路由到正确的 run；当前单会话规模下遍历注册表足够）。
    pub fn find_by_ask(&self, ask_id: &str) -> Option<std::sync::Arc<RunHandle>> {
        let runs = self
            .runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runs.values()
            .find(|h| {
                h.ask
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .is_some_and(|s| s.ask_id == ask_id)
            })
            .cloned()
    }

    /// run 任务收尾时摘除（终态帧发出后调用；先比对 run_id 再摘，防止旧的
    /// run 收尾与同会话新 run 启动并发时摘错新 run）。
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

// 单元测试挂载：测试代码放在独立文件 run_manager_test.rs，仅 cargo test 时编译。
#[cfg(test)]
#[path = "run_manager_test.rs"]
mod run_manager_test;
