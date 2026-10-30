//! run 主循环（技术设计文档 §5.2）+ 工具批的审批挂起机制 + 取消传播的
//! 收齐语义（§5.3）+ 受控并发批次调度（§5.4 ⑤，07 号票）。
//!
//! 名词解释：run = 一次模型任务的完整执行；主循环（RunLoop）= 驱动这个
//! 过程的循环——反复"调模型 → 执行工具批 → 再调模型"，直到模型给出最终
//! 答复；AgentRun 是 rig 库提供的可步进、可序列化的任务状态机，本模块
//! 一步步驱动它，同时生产帧（后端发给前端的事件消息）。
//!
//! 本模块由早期验证原型（`spike/turn.rs` 的手动循环与 `spike/cancel.rs` 的
//! 取消版）产品化而来，当前版本要点：
//! - 工具批的门控预判（§5.4 ①–④）：先对整批调用做完审批判定再执行——
//!   首个需挂起项即挂起（**挂起时批内零执行**；恢复后主循环原样重放
//!   全部调用，因此不存在重复副作用）；硬拦项回喂错误结果帧（不中断 run）；
//! - **策略门（Q35/Q42，07 号票）**：批内含 ask（Interactive 能力）时只
//!   执行源序首个 ask，其余调用（含其余 ask）明确回喂"未执行"
//!   （E_TOOL_SKIPPED）——先筛选再门控预判，被跳过项不申请审批；
//! - **受控并发执行（§5.4 ⑤，07 号票）**：批内 Allow 项交执行器的批次
//!   契约（[`CancellableExecutor::execute_batch`]，生产实现 = FileWrite
//!   全局互斥串行 + 其余信号量并发），结果按源序收集后统一发帧回喂；
//! - [`DriveState`]：跨挂起/恢复延续的驱动状态（计时、用量累计、工具
//!   计数与调用 id 映射——恢复后不重发 runStarted、用量不回退、
//!   toolResult 与挂起前的 toolStarted/toolArgs 帧同源）。
//!
//! 错误语义：`PromptError`（rig 侧失败）直接冒泡给调用方（RunManager 启动
//! 的 run 任务），由它发 runEnded(error) 终态帧——保证任何退出路径下前端
//! 都能收到终态。

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rig::completion::Message;
use rig::run::response::PromptResponse;
use rig::run::{AgentRun, AgentRunStep, PendingToolCall, PromptError};
use tokio_util::sync::CancellationToken;

use super::frames::{ChatFrame, DeltaThrottler, EndReason, FrameError, FrameSink, UsageTotal};
use super::rig_bridge::RigEventBridge;
use super::turn::{StreamExit, TurnDriver, usage_total};
use super::watchdog::{RunWatchdog, StallGuard, WatchdogAction, is_model_delta};
use crate::util::errors::E_RUN_STALLED;

/// 被取消工具的结果帧补挂的错误码（§3.4 toolResult.error.code 与 §3.6
/// 错误码注册表中的取消错误）。
pub const E_CANCELLED: &str = "E_CANCELLED";

/// ask 混批（Q35/Q42）与同批写冲突（Q32）中被跳过调用的错误码（§3.6，
/// 07 号票）：工具级失败回喂"未执行"，模型据说明重新决定或纠正。
const E_TOOL_SKIPPED: &str = "E_TOOL_SKIPPED";

/// 工具执行产物：帧面（给界面）与模型回喂面（给模型）的双通道裁剪
/// （§8.2 双通道在主循环侧的形状）。
#[derive(Debug, Clone, PartialEq)]
pub enum ToolExecution {
    /// 执行完毕（成功或工具级失败——失败也是收齐的结果，回喂模型促其纠正）。
    Completed {
        /// 是否成功。
        ok: bool,
        /// 界面摘要（一行人类可读说明）。
        ui_summary: String,
        /// 错误面（失败时携带）。
        error: Option<FrameError>,
        /// 模型通道的压缩文本（§8.2：成功时序列化数据；失败时为
        /// 错误码 + 消息）。
        model_text: String,
    },
    /// 取消终止：`executed=false` 为批内尚未执行（补标记不留悬空调用）；
    /// `executed=true` 为执行中途被收割（§5.3：已产生副作用的文件写不回滚）。
    /// 并发批（07 号票）中两态分别对应"未派发"与"在飞被收割"。
    Cancelled { executed: bool, message: String },
}

/// 一批待执行调用中的单项（源序；07 号票批次执行契约的输入单元）。
#[derive(Debug, Clone)]
pub struct BatchCall {
    /// 模型侧调用 id。
    pub call_id: String,
    /// 工具名。
    pub name: String,
    /// 工具参数（JSON 文本）。
    pub args_json: String,
}

/// 取消可感知的工具执行器契约（§5.4 门控与执行在 backend 侧的接口；
/// 生产实现见 [`super::exec::ToolsetExecutor`]，command 工具在实现内部
/// 以取消令牌 select! 收割进程组——原语见 `tools::process`）。
pub trait CancellableExecutor: Send + Sync {
    /// 执行一次工具调用：返回执行产物（完成/取消），实现须响应取消令牌。
    fn execute<'a>(
        &'a self,
        call_id: &'a str,
        name: &'a str,
        args_json: &'a str,
        token: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>>;

    /// 执行一批工具调用（§5.4 ⑤，07 号票批次契约）：返回结果与 `batch`
    /// 按**源序**一一对应（完成顺序不改序——并发实现 JoinSet 收集后按下标
    /// 重排）。取消收齐语义（§5.3 并发版）由实现保证：cancel 置位 → 停止
    /// 派发未开始任务 → join 全部在飞任务，未派发项以
    /// `Cancelled { executed: false }` 收口。
    ///
    /// 默认实现 = 逐个串行（M1 语义）：测试假执行器与未覆写实现沿用，
    /// 生产 [`super::exec::ToolsetExecutor`] 覆写为受控并发 + 同批写冲突
    /// 检测 + FileWrite 全局互斥。
    fn execute_batch<'a>(
        &'a self,
        batch: &'a [BatchCall],
        token: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Vec<ToolExecution>> + Send + 'a>> {
        Box::pin(async move {
            let mut results = Vec::with_capacity(batch.len());
            for call in batch {
                if token.is_cancelled() {
                    // 串行路径的取消收齐：余项补未执行标记
                    results.push(ToolExecution::Cancelled {
                        executed: false,
                        message: "取消：工具未执行".to_owned(),
                    });
                    continue;
                }
                results.push(
                    self.execute(&call.call_id, &call.name, &call.args_json, token)
                        .await,
                );
            }
            results
        })
    }
}

/// 工具门控判定接口（§5.4 步骤①–④）。`Allow` 直接执行；`Suspend` 表示需
/// 审批/围栏确认——主循环把整只 run 序列化（把内存对象转成 JSON 文本保存
/// 现场）后挂起并返回 [`RunExit::Suspended`]，宿主拿到审批应答后反序列化
/// 恢复继续（跨重启恢复天然支持，早期原型已验证）；`Blocked` 硬拦（围栏
/// 黑名单/权限过滤/计划门）——回喂错误结果帧，不中断 run。生产策略实现
/// 见 [`super::gate::ApprovalGate`]。
pub trait ToolGate: Send + Sync {
    /// 判定一次工具调用能否执行。
    fn judge(&self, name: &str, args_json: &str) -> GateVerdict;
}

/// 门控判定的三种结论。
#[derive(Debug, Clone, PartialEq)]
pub enum GateVerdict {
    /// 放行：直接执行。
    Allow,
    /// 挂起等待审批（`fence_reason`：围栏要求确认的写信号原因；`external_dir`：
    /// 外部目录授权目标（24 号票，§9.9——roots 外路径须"始终允许"授权该
    /// 目录及子目录）；`plan_scope`：计划范围门的计划外写路径（25 号票，
    /// §8.6——已批准计划基线外的写入须用户确认范围）。三者互斥，最多一个
    /// Some）。
    Suspend {
        fence_reason: Option<String>,
        external_dir: Option<String>,
        plan_scope: Option<String>,
    },
    /// 硬拦：错误结果帧回喂（code 取值为 E_FENCE_BLOCKED / E_TOOL_BLOCKED /
    /// E_PLAN_STALE / E_PLAN_REQUIRED / E_INVALID_ARG 之一）。
    Blocked { code: &'static str, message: String },
}

/// 全放行门（测试装配用；生产审批门为 [`super::gate::ApprovalGate`]）。
pub struct AllowAllGate;

/// 全放行门的判定实现：任何调用都放行。
impl ToolGate for AllowAllGate {
    fn judge(&self, _name: &str, _args_json: &str) -> GateVerdict {
        GateVerdict::Allow
    }
}

/// 挂起点信息：触发挂起的那次调用（宿主构造询问载荷、恢复后按签名放行
/// 都要用它）。
#[derive(Debug, Clone)]
pub struct PendingCall {
    /// 模型侧调用 id（approve 登记签名与挂起现场文件名用）。
    pub model_call_id: String,
    /// 工具名。
    pub name: String,
    /// 工具参数（JSON 文本）。
    pub args_json: String,
    /// 围栏要求确认的写信号原因（None = 纯权限矩阵审批）。
    pub fence_reason: Option<String>,
    /// 外部目录授权目标（24 号票，§9.9）：Some = 外部目录授权挂起（载荷
    /// 形态 externalDir，恢复循环在允许后调 allow_external_dir 并入会话
    /// roots 快照）；None = 普通审批/围栏挂起。
    pub external_dir: Option<String>,
    /// 计划范围门的计划外写路径（25 号票，§8.6）：Some = 范围门挂起（载荷
    /// 形态 approval.planScope，恢复循环在 allow_once 放行 / always_session
    /// 把目标父目录并入会话基线扩展）；None = 非范围门挂起。
    pub plan_scope: Option<String>,
}

/// 计划提醒的触发阈值（FR-06-04，附录 B 定 N=8，25 号票）：根会话每累计
/// 8 次变更工具未伴随 plan:update → 下一拍请求注入一次性提醒。
pub const PLAN_REMINDER_EVERY: u32 = 8;

/// 跨挂起/恢复延续的驱动状态（恢复后不重发 runStarted、用量与工具计数
/// 不回退、调用 id 映射同源）。
#[derive(Debug, Clone, Default)]
pub struct DriveState {
    /// run 的开始时刻（恢复时延续，不重置）。
    pub started_at: Option<Instant>,
    /// run 的用量累计（各轮相加）。
    pub total_usage: UsageTotal,
    /// 最近一次模型请求的真实输入 token 数（usage 收口时刷新；ask 关闭后
    /// 压缩触发点的占用估算基准，§6.2 触发点②）。
    pub last_input: u64,
    /// 已执行的工具调用计数。
    pub tool_count: u32,
    /// 最新轮次编号。
    pub last_turn: u32,
    /// 模型调用 id → 桥接帧面 id 映射（挂起时带出、恢复时播种新桥接器）。
    pub model_call_ids: HashMap<String, String>,
    /// 监督看门狗状态（§5.7，37 号票）：工具结果滑动窗口 + 无进展计数 +
    /// 各信号纠偏水位。挂在驱动状态上跨审批挂起延续——恢复不丢历史、
    /// 重放批不重复计数（挂起时批内零执行，恢复后原样执行一次）。
    pub watchdog: RunWatchdog,
    /// 计划提醒开关（FR-06-04，25 号票，§8.6）：仅根会话 run 装配为 true
    ///（宿主 start_chat 构造初始 DriveState 时置位）；子代理 run 用缺省
    /// Default（false）——"根会话"口径，子会话不计数不提醒。缺省 false
    /// 保证既有测试装配（resume: None）零行为漂移。
    pub plan_reminder_enabled: bool,
    /// 自上次计划更新（plan 工具成功执行）以来的变更工具（FileWrite/
    /// Network 能力）已执行次数：达 [`PLAN_REMINDER_EVERY`] 注入一次性提醒
    /// 并清零；plan 成功更新即清零。挂在驱动状态上跨审批挂起延续。
    pub plan_change_count: u32,
}

/// 主循环的退出态。
#[derive(Debug)]
pub enum RunExit {
    /// 自然终态（§5.3 Streaming → Done）。
    Done(Box<PromptResponse>),
    /// 取消终态：工具结果已全部收齐并补上标记，runEnded(cancelled) 已发出。
    Cancelled,
    /// 审批挂起：整只 run 已序列化为 JSON（恢复 = [`resume_run`] 后重新
    /// `drive_run`，主循环会重新给出未应答的调用；挂起时批内零执行，
    /// 恢复重放无重复副作用）。Box 收纳大负载（避免枚举整体变大，
    /// clippy large_enum_variant）。
    Suspended(Box<SuspendedInfo>),
    /// 看门狗终止终态（§5.7，37 号票）：runEnded(error, E_RUN_STALLED) 已由
    /// [`drive_run`] 发出（已输出内容与工具结果帧保留、不撤回，触发批的
    /// 已执行工具不重放副作用）；message 说明停滞或重复失败原因，宿主随
    /// `run:error` 事件带出、followUp 队列按失败暂停（§5.5）。
    Stalled { message: String },
    /// 每拍请求刷新失败终态（18 号票，§11.1 运行中切换的预算重查）：常见
    /// 为切到窗口更小的模型后现有上下文放不下（E_CONTEXT_OVERFLOW，§6.2
    /// 兜底语义——暂停保留输入）。runEnded(error, {code, message}) 已由
    /// [`drive_run`] 发出，宿主按失败分流（`run:error` 事件 + 队列暂停）。
    Failed { code: String, message: String },
}

/// 每拍模型请求前的刷新契约（18 号票，FR-02-07 修正案 / §11.1）：
/// "下一请求生效"覆盖同一 run 内的下一次模型请求——每拍 CallModel 前
/// [`drive_run`] 调用宿主实现，宿主读最新会话偏好，需要时替换模型客户端
/// （`TurnDriver::swap_model`）与请求参数（`TurnDriver::update_spec`），
/// 并对新模型重做预算检查；正在进行的模型请求不被切换打断。
///
/// 测试直驱路径传 None（无刷新源）。宿主副作用（读 prefs 文件、解析模型
/// 配置、预算估算）都收在宿主实现内，backend 层只见判定结果。
pub trait RequestRefresher: Send + Sync {
    /// 本拍请求前的刷新。`state` 提供最近请求的真实输入 token 数
    /// （`last_input`，预算估算基准）；首拍前为 0（run 开始前已做过完整
    /// 预算编排，此时只做能力面切换）。
    ///
    /// Err = 本拍请求不得发出，run 以 [`RunExit::Failed`] 终止（错误码随
    /// [`RefreshFailure`]，帧面 runEnded(error) 由 drive_run 发出）。
    fn before_request(&self, state: &DriveState) -> Result<(), RefreshFailure>;
}

/// 刷新失败（§3.6 错误码面）。
#[derive(Debug, Clone)]
pub enum RefreshFailure {
    /// 切换后的模型窗口装不下现有上下文 + 回复预留（§6.2：暂停保留输入，
    /// 提示压缩或新会话）。
    ContextOverflow { estimated: u64, window: u64 },
    /// 其他刷新失败（模型解析失败等，参数面问题——E_INVALID_ARG 面向上抛
    /// 为终止原因）。
    Failed(String),
}

impl RefreshFailure {
    /// (错误码, 消息)（§3.6）。
    fn parts(&self) -> (&'static str, String) {
        match self {
            RefreshFailure::ContextOverflow { estimated, window } => (
                "E_CONTEXT_OVERFLOW",
                format!(
                    "上下文占用约 {estimated} token，超过切换后模型窗口（{window}）的硬预算：\
                     已暂停本次运行，输入与已产出内容均已保留，可重试压缩或切换窗口更大的模型"
                ),
            ),
            RefreshFailure::Failed(message) => ("E_INVALID_ARG", message.clone()),
        }
    }
}

/// 挂起信息（[`RunExit::Suspended`] 的载体）。
#[derive(Debug)]
pub struct SuspendedInfo {
    /// 序列化后的 run 现场（恢复 = 反序列化后重新驱动）。
    pub run_json: String,
    /// 触发挂起的调用（构造询问载荷与恢复后按签名放行）。
    pub pending: PendingCall,
    /// 跨挂起延续的驱动状态。
    pub state: DriveState,
}

/// 审批挂起机制的基础：把整只 run 序列化为 JSON 文本（§5.2）。
pub fn suspend_run(run: &AgentRun) -> serde_json::Result<String> {
    serde_json::to_string(run)
}

/// 审批挂起的恢复：从 JSON 文本反序列化回 run（rig 的信封带 format 版本
/// 校验，版本不符拒绝加载）。
pub fn resume_run(run_json: &str) -> serde_json::Result<AgentRun> {
    serde_json::from_str(run_json)
}

/// steer 插话源（§5.5，06 号票）：RunLoop 在 turn 边界（工具批结果回喂后、
/// 下一次模型调用前）经它取一条插话注入下一请求。副作用（canonical 落盘 +
/// `run:inject` 回执事件）收在宿主实现里——backend 层只见纯文本的取出口，
/// 不依赖存储与事件汇。测试直驱路径传 None（无插话源）。
pub trait SteerSource: Send + Sync {
    /// 消费一条插话：宿主实现出队 + 落盘 + 回执后返回注入文本；
    /// 队列空返回 None。`turn` 为即将开始的模型调用轮次（回执载荷
    /// `acceptedAtTurn` 的取值面）。
    fn take(&self, turn: u32) -> Option<String>;
}

/// 主循环驱动（§5.2 的"每轮"循环 + §3.4 的帧生产）：
///
/// - 首次调用（`resume` 为 None）发 runStarted 起手帧；恢复调用延续
///   [`DriveState`]（不重发 runStarted、用量与计数延续）；
/// - CallModel（调模型）：发 stepStarted → 流式调用（增量经 64ms 节流器
///   出帧）→ 收口发 usage 帧（§10.5 累计口径，rate = 本轮输出速度，
///   token/秒）→ 收尾 flush；
/// - **模型停滞看门狗（§5.7 计时边界，Q33，37 号票）**：停滞计时随单次模型
///   请求生灭——`stall_timeout` 从请求发出起算，`select!` 让流式调用与停滞
///   截止竞争，有效模型增量（三类增量帧）刷新计时，停滞胜出即丢弃在飞
///   HTTP 流（与取消同形）按 E_RUN_STALLED 终止；工具执行、审批挂起等待、
///   子代理等待不在计时生命周期内（各阶段 `drive_run` 已退出）；
/// - **工具窗口看门狗（§5.7，37 号票）**：工具批收口（结果回喂前）把结果
///   记入滑动窗口并做 turn 边界评估——纠偏注入 hidden system 提示（下一拍
///   CallModel 的请求面 history，不落 canonical），终止按 E_RUN_STALLED
///   收尾（runEnded(error) 只发一次，已输出内容与工具结果保留）；
/// - **steer 消费（§5.5，06 号票）**：非首拍的 CallModel 前（即上一 turn
///   已收尾——工具批结果回喂后）经 `steer` 取一条插话，以 user 消息追加进
///   本拍请求的 history（落盘与回执副作用在宿主实现内）；
/// - **每拍请求刷新（§11.1，18 号票）**：每拍 CallModel 前经 `refresh`
///   询问宿主（会话模型/thinking 偏好可能已变）——宿主换模型/换参数后
///   本拍即用新组合；刷新失败按 [`RunExit::Failed`] 终止（预算越界暂停
///   保留输入）；
/// - CallTools（执行工具批）：**先策略门筛选、再整批判定、后受控并发**
///   （§5.4 ①–⑤，07 号票）——批内含 ask 时只执行源序首个 ask（其余回喂
///   E_TOOL_SKIPPED"未执行"）；真正执行的调用中首个 Suspend 即把整只 run
///   序列化挂起（批内零执行）；Blocked 回喂错误结果；其余交执行器批次
///   契约并发执行（FileWrite 全局互斥串行 + 其余信号量限流），结果收集后
///   **按源序**发 toolResult 帧（调用 id 经桥接映射与流式三帧同源）；
/// - 取消（§5.3）：流中取消即收尾 flush + runEnded(cancelled)；工具批内
///   任一任务报告取消（或批次执行前已取消）→ 未派发/未执行项补
///   E_CANCELLED，全部收齐后才发终态帧；
/// - Done：runEnded(done) 必为最后一帧。
#[allow(clippy::too_many_arguments)]
pub async fn drive_run<W, T>(
    driver: &TurnDriver<W, T>,
    run: AgentRun,
    run_id: &str,
    model_name: &str,
    token: CancellationToken,
    executor: Arc<dyn CancellableExecutor>,
    gate: Arc<dyn ToolGate>,
    sink: &mut dyn FrameSink,
    resume: Option<DriveState>,
    steer: Option<&dyn SteerSource>,
    refresh: Option<&dyn RequestRefresher>,
    stall_timeout: Duration,
) -> Result<RunExit, PromptError>
where
    W: rig::wire::Wire<Op = rig::operation::Completion>,
    T: rig::driver::Transport<W>,
{
    let mut state = resume.unwrap_or_default();
    // 首次进入（无恢复状态）：记录开始时刻并发起手帧；恢复进入则跳过，
    // 不重发 runStarted。
    if state.started_at.is_none() {
        state.started_at = Some(Instant::now());
        sink.send(ChatFrame::RunStarted {
            run_id: run_id.to_owned(),
            model: model_name.to_owned(),
        });
    }
    let started_at = state.started_at.expect("入口已置位");

    let mut throttler = DeltaThrottler::new();
    // 当前 turn 的流事件桥（CallModel 每拍重置；CallTools 以其映射反查帧面 callId；
    // 首拍播种挂起前映射——恢复后重放的挂起批 toolResult 与挂起前三帧同源）。
    let mut bridge = RigEventBridge::new();
    bridge.seed_model_call_ids(state.model_call_ids.clone());

    // 看门狗的跨拍状态（37 号票，§5.7）：
    // - 纠偏提示待注入（上一工具批评估产生，下一拍 CallModel 注入请求面
    //   history——hidden system 提示，不落 canonical、不进用户可见树）；
    // - 本轮模型输出是否出现过正文增量（textDelta；无进展信号的数据源之一，
    //   推理增量不算——向用户可见的输出才算进展）。
    let mut pending_corrective: Option<String> = None;
    let mut turn_had_text = false;
    // 计划提醒待注入（FR-06-04，25 号票，§8.6）：变更工具计数达阈值后置位，
    // 下一拍 CallModel 以 hidden system 消息一次性注入（一次性 = take 后不
    // 重发；不产生工具结果帧、不阻塞）。仅 plan_reminder_enabled 的根会话
    // run 计数（子代理缺省关闭）。
    let mut pending_plan_reminder: Option<String> = None;

    let mut run = run;
    loop {
        match run.next_step()? {
            AgentRunStep::CallModel {
                prompt,
                history,
                turn,
            } => {
                state.last_turn = turn as u32;
                // 每拍请求刷新（§11.1，18 号票）：会话模型/thinking 偏好在
                // 上一拍结束后可能已变——宿主实现重读偏好并按需换模型客户端
                // 与请求参数（FR-02-07「下一请求生效」= 同 run 内的下一次
                // 模型请求，不只下一次 start_chat）。失败按 E_CONTEXT_
                // OVERFLOW 等错误码终止（§6.2 暂停保留输入语义）。
                if let Some(refresh) = refresh
                    && let Err(failure) = refresh.before_request(&state)
                {
                    let (code, message) = failure.parts();
                    sink.send(ChatFrame::RunEnded {
                        reason: EndReason::Error,
                        duration_ms: started_at.elapsed().as_millis() as u64,
                        tool_count: state.tool_count,
                        turn: state.last_turn,
                        error: Some(FrameError::new(code, message.clone())),
                    });
                    return Ok(RunExit::Failed {
                        code: code.to_owned(),
                        message,
                    });
                }
                // steer 消费点（§5.5，06 号票）：turn 边界 = 上一拍已收尾
                // （turn > 1 意味着走完了「模型输出 → 工具批回喂」的边界）。
                // 首拍（turn == 1）不消费——插话不并入模型见到的第一批输入。
                let mut history = history;
                if turn > 1
                    && let Some(text) = steer.and_then(|source| source.take(turn as u32))
                {
                    history.push(Arc::new(Message::User {
                        content: vec![rig::message::UserContent::text(text)],
                    }));
                }
                // 纠偏注入（§5.7，37 号票）：上一工具批评估出的 hidden system
                // 提示追加在本拍请求 history 末尾（紧跟工具结果，语义最近；
                // 请求面生效、不入史册）。
                if let Some(text) = pending_corrective.take() {
                    history.push(Arc::new(Message::System { content: text }));
                }
                // 计划提醒注入（FR-06-04，25 号票，§8.6）：变更工具计数达
                // 阈值后的一次性 hidden system 提醒，同纠偏机制（请求面生效、
                // 不入史册、不产生工具结果帧、不阻塞）；排在纠偏之后。
                if let Some(text) = pending_plan_reminder.take() {
                    history.push(Arc::new(Message::System { content: text }));
                }
                turn_had_text = false;
                sink.send(ChatFrame::StepStarted { turn: turn as u32 });
                let mut turn_bridge = RigEventBridge::new();
                turn_bridge.seed_model_call_ids(bridge.model_call_ids().clone());
                bridge = turn_bridge;
                // 模型停滞计时（§5.7 计时边界，Q33，37 号票）：随本拍请求生灭——
                // armed 即"请求发出"；闭包内三类增量帧（有效模型增量）刷新；
                // select! 与流式调用竞争，停滞胜出丢弃在飞 HTTP 流（select
                // 丢弃 stream future 即关流）并按 E_RUN_STALLED 终止。工具
                // 执行/审批/子代理等待不在本生命周期（那些阶段 drive_run 已
                // 退出）；刷新点在流循环回调内、不经帧枢纽——前端帧/重连
                // 不能续命。
                let stall = StallGuard::armed(stall_timeout);
                let mut on_frame = |frame: ChatFrame| {
                    if is_model_delta(&frame) {
                        stall.refresh();
                        if matches!(frame, ChatFrame::TextDelta { .. }) {
                            turn_had_text = true;
                        }
                    }
                    ingest_delta(&mut throttler, sink, &frame);
                };
                let turn_started = Instant::now();
                let outcome = tokio::select! {
                    biased;
                    _ = stall.expired() => {
                        let message = format!(
                            "模型流停滞：请求发出后 {} 秒内未收到有效模型增量（{E_RUN_STALLED}）；\
                             已输出内容与工具结果均已保留",
                            stall_timeout.as_secs().max(1)
                        );
                        return Ok(finish_stalled(sink, started_at, &state, message));
                    }
                    outcome = driver
                        .stream_turn_cancellable(
                            &mut run,
                            prompt,
                            history,
                            turn as u32,
                            &token,
                            &mut bridge,
                            &mut on_frame,
                        ) => outcome?,
                };
                // turn 收尾 flush：窗口里的增量尾巴放行（取消路径也要收齐）
                throttler.flush(sink);
                match outcome.exit {
                    StreamExit::Cancelled => {
                        // 模型流已停：无工具进行中，直接取消终结
                        return Ok(finish_cancelled(sink, started_at, state));
                    }
                    StreamExit::Completed => {
                        // 本轮自然收口：把本轮用量记入累计并发 usage 帧；
                        // 输出速度 = 本轮输出 token 数 ÷ 本轮耗时秒数。
                        if let Some(usage) = outcome.usage {
                            let total = usage_total(&usage);
                            let elapsed_s = turn_started.elapsed().as_secs_f64();
                            let rate = (total.output > 0 && elapsed_s > 0.0)
                                .then(|| total.output as f64 / elapsed_s);
                            state.last_input = total.input;
                            state.total_usage.accumulate(&total);
                            sink.send(ChatFrame::Usage {
                                usage: state.total_usage,
                                rate,
                            });
                        }
                    }
                }
            }
            AgentRunStep::CallTools { calls } => {
                // —— 策略门（Q35/Q42，§5.4）：批内含 ask（Interactive 能力）
                // 时只执行源序首个 ask，其余调用（含其余 ask）全部跳过——
                // 不能继续执行同批预先生成的参数，由模型看到回答后重新决定。
                let parts: Vec<(String, String, String)> =
                    calls.iter().map(pending_parts).collect();
                let first_ask = parts.iter().position(|(_, name, _)| is_interactive(name));

                // —— 批级门控预判（§5.4 ①–④）：只对真正执行的调用判定 ——
                // 被跳过项不参与（挂起时不为它们申请审批，Blocked 排除逻辑
                // 也不作用于它们）；Suspend：批内零执行即挂起（恢复重放整批，
                // 策略门与门控按同参数重判，结果一致，无重复副作用）。
                let execute_indices: Vec<usize> = match first_ask {
                    Some(ask_idx) => vec![ask_idx],
                    None => (0..parts.len()).collect(),
                };
                let verdicts: Vec<(usize, GateVerdict)> = execute_indices
                    .iter()
                    .map(|&idx| {
                        let (_, name, args_json) = &parts[idx];
                        (idx, gate.judge(name, args_json))
                    })
                    .collect();
                if let Some((
                    idx,
                    GateVerdict::Suspend {
                        fence_reason,
                        external_dir,
                        plan_scope,
                    },
                )) = verdicts
                    .iter()
                    .find(|(_, v)| matches!(v, GateVerdict::Suspend { .. }))
                    .map(|(i, v)| (*i, v.clone()))
                {
                    let (model_call_id, name, args_json) = parts[idx].clone();
                    // 挂起前把调用 id 映射带进驱动状态（恢复后的 toolResult
                    // 才能与挂起前的 toolStarted/toolArgs 帧同源）。
                    state.model_call_ids = bridge.model_call_ids().clone();
                    let run_json = suspend_run(&run).map_err(serde_json_error_to_prompt)?;
                    return Ok(RunExit::Suspended(Box::new(SuspendedInfo {
                        run_json,
                        pending: PendingCall {
                            model_call_id,
                            name,
                            args_json,
                            fence_reason: fence_reason.clone(),
                            external_dir: external_dir.clone(),
                            plan_scope: plan_scope.clone(),
                        },
                        state,
                    })));
                }

                // —— 组装批次：Allow 项进批次执行器；Blocked 项直接补错误
                // 结果（硬拦不执行、不中断 run，§5.4 ①/③）。 ——
                let mut batch: Vec<BatchCall> = Vec::new();
                let mut batch_positions: Vec<usize> = Vec::new();
                let mut prefilled: Vec<(usize, ToolExecution)> = Vec::new();
                for (idx, verdict) in verdicts {
                    let (call_id, name, args_json) = &parts[idx];
                    match verdict {
                        GateVerdict::Blocked { code, message } => {
                            state.tool_count += 1;
                            prefilled.push((
                                idx,
                                ToolExecution::Completed {
                                    ok: false,
                                    ui_summary: format!("已拦截：{name}"),
                                    error: Some(FrameError::new(code, message.clone())),
                                    model_text: format!("{code} {message}"),
                                },
                            ));
                        }
                        _ => {
                            batch.push(BatchCall {
                                call_id: call_id.clone(),
                                name: name.clone(),
                                args_json: args_json.clone(),
                            });
                            batch_positions.push(idx);
                        }
                    }
                }

                // —— 受控并发执行（§5.4 ⑤）：ToolsetExecutor 覆写为
                // FileWrite 全局互斥串行 + 其余信号量并发；测试假执行器走
                // trait 默认串行实现。取消收齐由实现保证（未派发补
                // Cancelled{executed:false}，在飞 join 后才返回）。 ——
                let executed: Vec<ToolExecution> = executor.execute_batch(&batch, &token).await;

                // —— 源序拼装（FR-03-11）：帧按源序发出、answers 按源序
                // 回喂——完成顺序不改序。 ——
                let mut outcomes: Vec<Option<ToolExecution>> = vec![None; parts.len()];
                // 真正进批次执行的位置集（Blocked 预填项不在此列——计划提醒
                // 计数只认已执行的工具，硬拦结果不算"变更执行"，25 号票 §8.6）。
                let executed_positions: HashSet<usize> = batch_positions.iter().copied().collect();
                for (pos, exec) in batch_positions.into_iter().zip(executed) {
                    outcomes[pos] = Some(exec);
                }
                for (idx, exec) in prefilled {
                    outcomes[idx] = Some(exec);
                }
                if let Some(ask_idx) = first_ask {
                    for (idx, (_, name, _)) in parts.iter().enumerate() {
                        if idx != ask_idx {
                            outcomes[idx] = Some(skipped_by_ask(name));
                        }
                    }
                }

                let mut answers = Vec::new();
                let mut cancel_seen = false;
                // 看门狗入窗记录（§5.7，37 号票）：本批真正收到的结果
                // （Completed 无论成败；取消补标记不入窗——run 即将终结）。
                let mut records = Vec::new();
                for (idx, call) in calls.into_iter().enumerate() {
                    let Some(exec) = outcomes[idx].take() else {
                        unreachable!("批内每个位置都已拼装结果");
                    };
                    let (call_id, name, args_json) = pending_parts(&call);
                    state.tool_count += 1;
                    match exec {
                        ToolExecution::Completed {
                            ok,
                            ui_summary,
                            error,
                            model_text,
                        } => {
                            records.push(super::watchdog::make_record(&name, &args_json, ok));
                            // 计划提醒计数（FR-06-04，25 号票，§8.6）：仅根
                            // 会话开关开启时计数；已执行（进批次）的 Completed
                            // 才算——plan 成功更新清零（同批伴随清零），变更
                            // 工具（FileWrite/Network 能力）累加，达 8 注入
                            // 一次性提醒并清零重新累计。
                            if state.plan_reminder_enabled && executed_positions.contains(&idx) {
                                if name == "plan" && ok {
                                    state.plan_change_count = 0;
                                } else if is_change_tool(&name) {
                                    state.plan_change_count += 1;
                                    if state.plan_change_count >= PLAN_REMINDER_EVERY {
                                        pending_plan_reminder =
                                            Some(plan_reminder_text(state.plan_change_count));
                                        state.plan_change_count = 0;
                                    }
                                }
                            }
                            sink.send(ChatFrame::ToolResult {
                                call_id: bridge.frame_call_id_for_model(&call_id),
                                ok,
                                ui_summary,
                                error,
                            });
                            answers.push(super::turn::answer_one(call, model_text));
                        }
                        ToolExecution::Cancelled { executed, message } => {
                            cancel_seen = true;
                            let summary = if executed {
                                format!("{E_CANCELLED}: 取消：{message}")
                            } else {
                                format!("{E_CANCELLED}: 取消：工具未执行")
                            };
                            sink.send(cancelled_result_frame(
                                &bridge.frame_call_id_for_model(&call_id),
                                &summary,
                                executed,
                            ));
                        }
                    }
                }
                if cancel_seen {
                    // 收齐后才发终态帧（§5.3 不变量：取消中工具收齐 → runEnded(cancelled)）
                    return Ok(finish_cancelled(sink, started_at, state));
                }
                // —— 工具窗口看门狗（§5.7，37 号票）：turn 边界评估（结果
                // 回喂前）——纠偏留待下一拍 CallModel 注入；终止按
                // E_RUN_STALLED 收尾：结果帧已发（保留不撤回）、本批不再
                // 回喂模型，已执行的工具不重放副作用。
                state.watchdog.record_step(records, turn_had_text);
                match state.watchdog.evaluate() {
                    WatchdogAction::None => {}
                    WatchdogAction::Corrective(text) => pending_corrective = Some(text),
                    WatchdogAction::Terminate(message) => {
                        return Ok(finish_stalled(sink, started_at, &state, message));
                    }
                }
                // 整批应答（按源序）回喂 run，驱动进入下一轮模型调用
                run.answer_all(answers)?;
            }
            AgentRunStep::Done(response) => {
                // 自然终态：runEnded(done) 为最后一帧
                sink.send(ChatFrame::RunEnded {
                    reason: EndReason::Done,
                    duration_ms: started_at.elapsed().as_millis() as u64,
                    tool_count: state.tool_count,
                    turn: state.last_turn,
                    error: None,
                });
                return Ok(RunExit::Done(Box::new(response)));
            }
        }
    }
}

/// 流式增量的节流入口：三型增量进节流器（满窗即 flush，首帧立即——§3.4 节流
/// 语义），其余帧直通 sink（保证 toolResult 必晚于同 callId 全部 toolArgs）。
fn ingest_delta(throttler: &mut DeltaThrottler, sink: &mut dyn FrameSink, frame: &ChatFrame) {
    match frame {
        ChatFrame::TextDelta { turn, delta } => throttler.push_text(*turn, delta),
        ChatFrame::ReasoningDelta { turn, delta } => throttler.push_reasoning(*turn, delta),
        ChatFrame::ToolArgs { call_id, chunk } => throttler.push_tool_args(call_id, chunk),
        other => {
            sink.send(other.clone());
            return;
        }
    }
    if throttler.should_flush() {
        throttler.flush(sink);
    }
}

/// 发出取消终态帧并返回取消退出态（runEnded(cancelled) 必为最后一帧）。
fn finish_cancelled(sink: &mut dyn FrameSink, started_at: Instant, state: DriveState) -> RunExit {
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Cancelled,
        duration_ms: started_at.elapsed().as_millis() as u64,
        tool_count: state.tool_count,
        turn: state.last_turn,
        error: None,
    });
    RunExit::Cancelled
}

/// 发出看门狗终止终态帧并返回停滞退出态（§5.7，37 号票）：
/// runEnded(error, { code: E_RUN_STALLED, message }) 只发一次；终止前已发出
/// 的文本增量与工具结果帧原样保留（不撤回），触发批已收齐的工具不重放。
fn finish_stalled(
    sink: &mut dyn FrameSink,
    started_at: Instant,
    state: &DriveState,
    message: String,
) -> RunExit {
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Error,
        duration_ms: started_at.elapsed().as_millis() as u64,
        tool_count: state.tool_count,
        turn: state.last_turn,
        error: Some(FrameError::new(E_RUN_STALLED, message.clone())),
    });
    RunExit::Stalled { message }
}

/// 被取消工具的 toolResult 帧：ok=false + error.code=E_CANCELLED（§3.4 error 面）。
fn cancelled_result_frame(call_id: &str, ui_summary: &str, executed: bool) -> ChatFrame {
    ChatFrame::ToolResult {
        call_id: call_id.to_owned(),
        ok: false,
        ui_summary: ui_summary.to_owned(),
        error: Some(FrameError::new(
            E_CANCELLED,
            if executed {
                "执行中被取消（已产生副作用不回滚）"
            } else {
                "取消：工具未执行"
            },
        )),
    }
}

/// ask（Interactive 能力）判定（Q35 策略门）：查工具注册表的能力分级。
/// 未知工具名视为非 ask（由执行器回喂 E_INVALID_ARG）；suggest 属 run 尾
/// 后台调用、不在注册表，天然不进本批。
fn is_interactive(name: &str) -> bool {
    crate::tools::lookup_spec(name)
        .is_some_and(|spec| spec.capability == crate::safety::tier::Capability::Interactive)
}

/// 变更工具判定（FR-06-04 计划提醒的计数口径，25 号票 §8.6）：FileWrite /
/// Network 能力的工具执行一次计一次（write/edit/command/web_*/http_request
/// 等）；ReadOnly/Interactive/Meta 不计。未知工具名不计（执行器会回喂
/// E_INVALID_ARG，没有产生变更）。
fn is_change_tool(name: &str) -> bool {
    crate::tools::lookup_spec(name).is_some_and(|spec| {
        matches!(
            spec.capability,
            crate::safety::tier::Capability::FileWrite | crate::safety::tier::Capability::Network
        )
    })
}

/// 计划提醒文本（一次性 hidden system 注入，§8.6）：固定中文话术，提示
/// 模型同步计划条目状态。N 为触达阈值的变更次数（恒为
/// [`PLAN_REMINDER_EVERY`]，入参便于测试断言与文案自洽）。
fn plan_reminder_text(n: u32) -> String {
    format!(
        "计划纪律提醒：已连续执行 {n} 次变更类工具而未更新计划。\
         请用 plan 工具同步当前进度（完成的条目标记 completed、进行中的标 in_progress），\
         保持计划与实际执行一致后再继续。"
    )
}

/// ask 混批（Q35/Q42）被跳过调用的统一"未执行"产物：明确告知模型该调用
/// 未执行、等待回答后重新决定——无副作用，也不申请审批（§5.4）。
fn skipped_by_ask(name: &str) -> ToolExecution {
    ToolExecution::Completed {
        ok: false,
        ui_summary: "未执行：等待用户回答后重新决定".to_owned(),
        error: Some(FrameError::new(
            E_TOOL_SKIPPED,
            "同批包含用户询问（ask），本调用未执行，不申请审批",
        )),
        model_text: format!(
            "{E_TOOL_SKIPPED} 该调用未执行：同批包含用户询问（ask），不能继续执行同批预先生成的参数；\
             请等待用户回答后重新决定是否再次调用 {name}"
        ),
    }
}

/// run 序列化失败归入 PromptError（挂起机制的面内错误，正常不会发生）。
fn serde_json_error_to_prompt(e: serde_json::Error) -> PromptError {
    PromptError::from(rig::error::ProviderError::request(format!(
        "审批挂起序列化失败：{e}"
    )))
}

/// 从待执行的调用中提取（模型调用 id, 工具名, 参数 JSON）三元组。
fn pending_parts(call: &PendingToolCall) -> (String, String, String) {
    match call {
        PendingToolCall::Execute(exec) => (
            exec.id().to_string(),
            exec.name().to_string(),
            exec.arguments().to_string(),
        ),
        PendingToolCall::Malformed(malformed) => (
            malformed.id().to_string(),
            malformed.name().to_string(),
            malformed.raw_arguments().to_owned(),
        ),
        // #[non_exhaustive]：未来新增变体按未执行补取消标记
        other => (
            String::new(),
            String::new(),
            format!("unhandled pending call variant: {other:?}"),
        ),
    }
}
