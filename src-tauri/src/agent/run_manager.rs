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
//!   （`session_run_snapshot`，用于前后端互相核对状态是否一致）由 05 号票
//!   实装（本文件的 [`RunHandle`] 即数据源头）。
//!
//! 帧枢纽（§5.8/D16，05 号票）：每个 run 的 [`FrameHub`] 承担"seq 递增 →
//! 入环形缓冲 → 下发"的帧出口职责，环形缓冲（512 帧）供 WebView 重载后
//! `attach_run(sinceSeq)` 重放续流，发送通道可热切换（重放后新帧只走新通道）。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio_util::sync::CancellationToken;

use crate::backend::exec::AskSlotCell;
use crate::backend::frames::{
    ChatFrame, FrameSink, RunSnapshot, RunSnapshotStatus, SequencedFrame, SequencedSink, UsageTotal,
};
use crate::tools::plan::PlanState;
use crate::util::errors::{E_SESSION_RUNNING, IpcError};

/// 环形帧缓冲容量（D16 定案：512 帧；溢出丢最旧并记 dropped 计数）。
pub const FRAME_BUFFER_CAPACITY: usize = 512;

/// `attach_run` 重连结果（§5.8，§3.3 #57）：`replayed` = 重放的区间帧数
/// （不含摘要帧）；`snapshot_sent` = 是否补发了 runSnapshot 摘要帧
/// （重放窗口耗尽——sinceSeq 落在环形缓冲之前时为 true）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachOutcome {
    pub replayed: u64,
    pub snapshot_sent: bool,
}

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
    /// 本 run 已执行的工具调用次数（帧面口径：已收到的工具结果帧数）。
    pub tool_count: u32,
    /// 已发出的最后一帧序号（seq 为全 run 递增的流水号，前端按它排序去重）。
    /// 读取面以 [`FrameHub`] 的实际序号为准（见 [`RunHandle::snapshot`]）。
    pub last_seq: u64,
    /// 本 run 的 token 用量累计（帧面 usage 口径：usage 帧载荷即运行累计值）。
    pub usage: UsageTotal,
}

/// 单个活跃 run 的句柄：持有取消令牌与运行时状态，供宿主命令层操作这个
/// run（技术设计文档 §5.1）。用户插话队列（steer）为后续工单；ask 槽与
/// 计划状态由 backend 执行器写入、宿主命令 resolve_ask/approve_plan 读取；
/// 帧枢纽 [`FrameHub`] 承担帧出口（seq 递增 → 入环 → 下发）与 attach 重连
/// （§5.8，05 号票）。
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
    /// 帧枢纽（§5.8）：环形帧缓冲 + 可热切换的下发通道，`attach_run`
    /// 重放续流的数据源（Arc 共享给 sink 装饰链与宿主命令两侧）。
    pub hub: Arc<FrameHub>,
}

/// RunHandle 的固有方法实现。
impl RunHandle {
    /// 拷贝一份当前状态（对账比对与测试用；对账 = 前后端各自记录的状态
    /// 互相核对）。锁若被其他线程的 panic 污染，也直接取出内部数据继续用
    /// ——状态是纯内存结构，恢复使用比让后续操作全部失败更合理。
    /// `last_seq` 以帧枢纽的实际序号为准（运行状态层不重复记账）。
    pub fn snapshot(&self) -> RunState {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        state.last_seq = self.hub.last_seq();
        state
    }

    /// `attach_run` 重连（§5.8，§3.3 #57）：以当前运行状态构造 runSnapshot
    /// 摘要帧所需数据，交给帧枢纽完成"重放 `(sinceSeq, 头]` + 窗口耗尽补
    /// 摘要帧 + 热切换下发通道"。
    pub fn attach(&self, since_seq: u64, out: Box<dyn SequencedSink>) -> AttachOutcome {
        let state = self.snapshot();
        let snapshot = RunSnapshot {
            status: RunSnapshotStatus::Running,
            run_id: Some(self.run_id.clone()),
            started_at: Some(state.started_at),
            turn: Some(state.turn),
            tool_count: Some(state.tool_count),
            usage: state.usage,
            // 摘要帧真正占用的 seq 在枢纽内分配，lastSeq 由枢纽按"摘要帧之前
            // 的最大帧序号"填写，此处占位。
            last_seq: 0,
        };
        self.hub.attach(since_seq, snapshot, out)
    }

    /// §3.2 RunSnapshot 的命令面构造（`session_run_snapshot` 返回体，
    /// FR-03-08 对账用）：以当前运行状态填充，status 恒为 Running（无活跃
    /// run 的会话在命令层返回 null）。
    pub fn run_snapshot(&self) -> RunSnapshot {
        let state = self.snapshot();
        RunSnapshot {
            status: RunSnapshotStatus::Running,
            run_id: Some(self.run_id.clone()),
            started_at: Some(state.started_at),
            turn: Some(state.turn),
            tool_count: Some(state.tool_count),
            usage: state.usage,
            last_seq: state.last_seq,
        }
    }
}

/// 帧枢纽（§5.8/D16，05 号票）：run 的帧出口枢纽——"seq 递增 → 入环形缓冲
/// （512 帧，溢出丢最旧并记 dropped）→ 下发"收在一处；下发通道可热切换
/// （`attach_run` 重放后续流帧只走新通道）。互斥锁同时保护缓冲与通道：
/// run 任务写入与 attach 命令的重放/切换共用一把锁，保证切换后新帧必在
/// 全部重放帧之后发出（帧序严格不乱，§3.4 排序规则）。
#[derive(Debug)]
pub struct FrameHub {
    inner: Mutex<HubInner>,
}

/// 帧枢纽的内部状态（互斥锁保护；下发端为 trait 对象不带 Debug，手动实现
/// 只打印可比较的记账字段）。
struct HubInner {
    /// 环形帧缓冲（按 seq 升序，最旧在头；容量 [`FRAME_BUFFER_CAPACITY`]）。
    buf: VecDeque<SequencedFrame>,
    /// 溢出丢弃的帧数（seq 从 0 连续递增，故环内最旧帧 seq == dropped；
    /// 该计数同时是 attach 判定"重放窗口耗尽"的基准）。
    dropped: u64,
    /// 下一个待分配的帧序号（已发出最大帧序号 = next_seq - 1）。
    next_seq: u64,
    /// 当前下发端（None = 只入环不下发，run 未装配通道的间隙；attach 热
    /// 切换即替换此端）。
    out: Option<Box<dyn SequencedSink>>,
}

/// HubInner 的调试打印（下发端不参与，只报记账字段）。
impl std::fmt::Debug for HubInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubInner")
            .field("buf_len", &self.buf.len())
            .field("dropped", &self.dropped)
            .field("next_seq", &self.next_seq)
            .field("out_attached", &self.out.is_some())
            .finish()
    }
}

/// 帧枢纽的固有方法实现。
impl FrameHub {
    /// 构造枢纽：初始下发端由调用方给（生产 = tauri Channel 适配器，测试 =
    /// 收集器；`start_chat` 的压缩编排帧也经此出口——run 注册前入环不影响
    /// attach 语义，压缩帧随重放恢复）。
    pub fn new(out: Box<dyn SequencedSink>) -> Self {
        Self {
            inner: Mutex::new(HubInner {
                buf: VecDeque::with_capacity(FRAME_BUFFER_CAPACITY),
                dropped: 0,
                next_seq: 0,
                out: Some(out),
            }),
        }
    }

    /// 已发出的最大帧序号（无帧时为 0；§3.2 RunSnapshot.lastSeq 口径）。
    pub fn last_seq(&self) -> u64 {
        self.lock().next_seq.saturating_sub(1)
    }

    /// 已丢弃的帧数（环形缓冲溢出累计；对账健康度指标）。
    pub fn dropped(&self) -> u64 {
        self.lock().dropped
    }

    /// 帧入口（§5.8）：seq 递增 → 入环（溢出丢最旧并记 dropped）→ 下发。
    /// 下发失败（webview 已重载）不影响入环——帧已在缓冲里，可被 attach
    /// 重放（§5.8"发送失败不影响入环"）。锁被污染时直接恢复使用：帧枢纽
    /// 是纯内存结构，没有需要回滚的状态。
    pub fn push(&self, frame: ChatFrame) {
        let mut inner = self.lock();
        let seq = inner.next_seq;
        inner.next_seq += 1;
        let sequenced = SequencedFrame {
            seq,
            ts: Some(now_ms()),
            frame,
        };
        if inner.buf.len() >= FRAME_BUFFER_CAPACITY {
            inner.buf.pop_front();
            inner.dropped += 1;
        }
        inner.buf.push_back(sequenced.clone());
        if let Some(out) = inner.out.as_mut() {
            out.send_seq(sequenced);
        }
    }

    /// `attach_run` 主体（§5.8）：锁内完成"收集重放区间 → 窗口耗尽判定 →
    /// 补 runSnapshot 摘要帧（占一个新 seq 号，不进环形缓冲）→ 热切换下发
    /// 通道 → 逐帧下发"。摘要帧与重放帧都经新通道发出，此后新帧只走新
    /// 通道且必排在本批之后。
    ///
    /// `snapshot` 为调用方按当前运行状态预构造的 §3.2 RunSnapshot（运行中
    /// 形态；`last_seq` 由本方法按摘要帧之前的最大帧序号回填）。
    pub fn attach(
        &self,
        since_seq: u64,
        mut snapshot: RunSnapshot,
        out: Box<dyn SequencedSink>,
    ) -> AttachOutcome {
        let mut inner = self.lock();
        // ① 重放区间：sinceSeq 的语义 = 前端"已见最大帧序号"（0 = 一无所见，
        //    从 seq 0 起全量重放——重载后前端的唯一取值）；其余取值重放
        //    `(sinceSeq, 头]`。帧原样携带 seq/ts（§3.4 帧基座，前端按既有
        //    去重规则忽略已见）。
        let wanted_from = if since_seq == 0 {
            0
        } else {
            since_seq.saturating_add(1)
        };
        let replay: Vec<SequencedFrame> = inner
            .buf
            .iter()
            .filter(|f| f.seq >= wanted_from)
            .cloned()
            .collect();
        // ② 窗口耗尽判定：重放起点之前仍有前端未见的帧（即
        //    `[起点, 缓冲最旧帧)` 存在不可见缺口）→ 补摘要。环满溢出
        //    （dropped>0）是该缺口的典型来源。
        let oldest = inner.buf.front().map(|f| f.seq).unwrap_or(inner.next_seq);
        let need_snapshot = oldest > wanted_from;
        // ③ 摘要帧占号（在重放帧之后、后续新帧之前，seq 连续递增）
        let mut to_send = replay;
        let replayed = to_send.len() as u64;
        if need_snapshot {
            let seq = inner.next_seq;
            inner.next_seq += 1;
            snapshot.last_seq = seq.saturating_sub(1);
            to_send.push(SequencedFrame {
                seq,
                ts: Some(now_ms()),
                frame: ChatFrame::RunSnapshot {
                    snapshot: snapshot.clone(),
                },
            });
        }
        // ④ 热切换 + 逐帧下发（切换在同一把锁内完成：run 任务的下一次
        //    push 会等锁，新帧必在本批之后经新通道发出）
        inner.out = Some(out);
        if let Some(sink) = inner.out.as_mut() {
            for frame in to_send {
                sink.send_seq(frame);
            }
        }
        AttachOutcome {
            replayed,
            snapshot_sent: need_snapshot,
        }
    }

    /// 锁获取（污染即恢复：见 [`FrameHub::push`] 注释）。
    fn lock(&self) -> std::sync::MutexGuard<'_, HubInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// FrameSink 适配器：把帧交给共享枢纽（Arc 克隆进 sink 装饰链最内层——
/// seq 递增、入环、下发都发生在枢纽内，原 `ChannelSink` 的职责收编于此，
/// 外层装饰器不感知重放）。
#[derive(Debug)]
pub struct HubSink {
    /// 共享的帧枢纽。
    pub hub: Arc<FrameHub>,
}

/// HubSink 对帧出口接口的实现：转发给枢纽入口。
impl FrameSink for HubSink {
    fn send(&mut self, frame: ChatFrame) {
        self.hub.push(frame);
    }
}

/// 当前时间的 Unix 毫秒时间戳（取不到时兜底 0，只影响展示字段不参与逻辑）。
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
    /// `hub` 为本次 run 的帧枢纽（调用方构造——初始下发端随构造注入），
    /// 返回句柄后由调用方自行启动 run 异步任务（本注册表只管登记，
    /// 不关心任务具体怎么跑）。
    pub fn start(
        &self,
        session_id: &str,
        run_id: String,
        now_ms: i64,
        hub: Arc<FrameHub>,
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
                usage: UsageTotal::default(),
            }),
            ask: Arc::new(Mutex::new(None)),
            plan: Arc::new(Mutex::new(None)),
            hub,
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
