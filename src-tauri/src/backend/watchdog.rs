//! 监督看门狗（技术设计文档 §5.7，FR-03-07，37 号票）。
//!
//! 名词解释：看门狗（watchdog）= 旁路监督 run 推进过程、在模型陷入重复失败
//! 或空转、模型流停滞时及时纠偏或终止的守护逻辑。它不参与正常的工具执行与
//! 模型调用，只在两个接缝观察：
//!
//! - **工具窗口（[`RunWatchdog`]）**：容量 32 的滑动窗口记录每次工具调用的
//!   结果（签名 = 工具名 + 参数哈希），在 turn 边界（工具批收口、结果回喂
//!   前）评估四类信号——同签名连续失败、无进展轮次、同参数只读成功重复、
//!   同文件连续分段读——命中阈值即纠偏（hidden system 提示）或终止
//!   （E_RUN_STALLED）；
//! - **停滞计时（[`StallGuard`]）**：仅模型请求阶段计时（Q33 边界）——单次
//!   模型请求自发出起，无有效模型增量达到 `stall_timeout` 即终止；有效增量
//!   （§5.7 字段级判定：三类增量帧 textDelta/reasoningDelta/toolArgs）到达
//!   即刷新。工具执行、审批等待、子代理等待不在计时生命周期内；前端帧、
//!   重连与工具进度不能刷新计时。

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant as TokioInstant;

use super::frames::ChatFrame;

// ---- 阈值（§5.7 表格钉死；验收要求全部可复现）----

/// 工具结果滑动窗口容量（§5.7：32）。
pub const WINDOW_CAPACITY: usize = 32;
/// 同签名工具连续失败——纠偏阈值（注入 hidden system 提示）。
pub const SAME_SIGNATURE_FAIL_CORRECT: usize = 3;
/// 同签名工具连续失败——终止阈值（E_RUN_STALLED）。
pub const SAME_SIGNATURE_FAIL_TERMINATE: usize = 5;
/// 无进展轮次（无文本且无工具成功）——纠偏阈值（步）。
pub const NO_PROGRESS_CORRECT: u32 = 8;
/// 无进展轮次——终止阈值（步）。
pub const NO_PROGRESS_TERMINATE: u32 = 14;
/// 同参数只读调用成功重复——纠偏阈值（次）。
pub const READONLY_REPEAT_CORRECT: usize = 4;
/// 同文件连续分段读——纠偏阈值（次）。
pub const SEGMENTED_READ_CORRECT: usize = 10;

/// 一条工具结果记录（滑动窗口的单元）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRecord {
    /// 调用签名 = 工具名 + 参数哈希（§5.7；参数哈希对参数 JSON 文本求
    /// std DefaultHasher 值——同进程内一致即可，不跨进程持久化）。
    pub signature: u64,
    /// 工具名。
    pub name: String,
    /// 执行是否成功（失败也是收齐的结果，同样入窗——连续失败信号的数据源）。
    pub ok: bool,
    /// 是否只读工具（按注册表能力分级 [`crate::safety::tier::Capability::ReadOnly`]
    /// 判定；只读成功重复信号只看只读工具）。
    pub read_only: bool,
    /// read 类工具的目标文件路径（分段读信号用；按参数 JSON 的 `path` 字段
    /// 原样提取，不做路径归一——检测的是模型行为模式，非文件同一性证明）。
    pub read_path: Option<String>,
}

/// turn 边界评估的结论（§5.7 动作列）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchdogAction {
    /// 无事发生。
    None,
    /// 纠偏：注入 hidden system 提示（不写入用户可见消息树，只在下一拍
    /// 模型请求面生效）。
    Corrective(String),
    /// 终止 run（E_RUN_STALLED，message 说明停滞或重复失败原因）。
    Terminate(String),
}

/// run 内的工具结果滑动窗口与无进展计数（§5.7）。
///
/// 挂在跨挂起延续的驱动状态（[`super::run_loop::DriveState`]）上：审批挂起
/// 恢复后窗口与计数延续，不丢历史、不重复计数。阈值评估带纠偏水位——同一
/// 信号的同一水平只纠偏一次，重复越线（计数继续增长）再次纠偏或升级终止。
#[derive(Debug, Clone, Default)]
pub struct RunWatchdog {
    /// 工具结果滑动窗口（按执行顺序追加，满容量丢最旧）。
    window: VecDeque<ToolRecord>,
    /// 连续无进展步数（有进展的步清零；步 = 一次模型调用 + 其工具批）。
    no_progress_steps: u32,
    /// 无进展纠偏水位（段内已纠偏标志）：8 步纠偏一次后，继续空转不再
    /// 重复纠偏，直至 14 步终止；有进展步清零计数时一并复位。
    no_progress_corrected: bool,
    /// 同签名失败段的锚：段起点前一条记录的签名（段从窗口头开始时为
    /// None）——锚变化即连续段重开，纠偏水位随之复位。
    fail_anchor: Option<u64>,
    /// 同签名失败纠偏水位（段内已纠偏标志）：3 次纠偏一次后，继续失败
    /// 不再重复纠偏，直至 5 次终止；段重开（锚变化）时复位。
    fail_corrected: bool,
    /// 只读重复已纠偏次数（每满 4 次重复纠偏一次；模型无视纠偏继续
    /// 重复时按级再提醒，防纠偏风暴）。
    readonly_corrected_count: usize,
    /// 分段读已纠偏次数（每满 10 次连续分段读纠偏一次；同上按级再提醒）。
    segmented_corrected_count: usize,
}

/// 签名计算：工具名 + 参数 JSON 文本的哈希（std DefaultHasher，进程内一致）。
pub fn tool_signature(name: &str, args_json: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    args_json.hash(&mut hasher);
    hasher.finish()
}

/// 从 read 类工具的参数 JSON 提取目标文件路径（`path` 字段原样；解析失败
/// 返回 None——畸形参数不是分段读信号的数据源）。
pub fn read_tool_path(args_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(args_json)
        .ok()?
        .get("path")?
        .as_str()
        .map(str::to_owned)
}

/// 从工具名与参数构造一条结果记录（只读判定查注册表能力分级；read 路径
/// 提取见 [`read_tool_path`]）。未知工具名按非只读处理（由执行器回喂错误，
/// 不会成为成功记录）。
pub fn make_record(name: &str, args_json: &str, ok: bool) -> ToolRecord {
    let read_only = crate::tools::lookup_spec(name)
        .is_some_and(|spec| spec.capability == crate::safety::tier::Capability::ReadOnly);
    let read_path = (name == "read")
        .then(|| read_tool_path(args_json))
        .flatten();
    ToolRecord {
        signature: tool_signature(name, args_json),
        name: name.to_owned(),
        ok,
        read_only,
        read_path,
    }
}

/// RunWatchdog 的记录与 turn 边界评估。
impl RunWatchdog {
    /// 记录一个工具批的结果并推进无进展计数（turn 边界、结果回喂前调用）。
    ///
    /// `records` = 本批真正收到的结果（按源序；取消补标记的调用不入窗——
    /// 取消时 run 即将终结，不参与信号统计）。`step_had_text` = 本轮模型
    /// 输出是否出现过正文增量（textDelta；推理增量不算——向用户可见的输出
    /// 才算进展，纯思考空转应被抓住）。本步无进展 = 无正文且本批无成功。
    pub fn record_step(
        &mut self,
        records: impl IntoIterator<Item = ToolRecord>,
        step_had_text: bool,
    ) {
        let records: Vec<ToolRecord> = records.into_iter().collect();
        let step_had_success = records.iter().any(|r| r.ok);
        for record in records {
            if self.window.len() >= WINDOW_CAPACITY {
                self.window.pop_front();
            }
            self.window.push_back(record);
        }
        if step_had_text || step_had_success {
            self.no_progress_steps = 0;
            self.no_progress_corrected = false;
        } else {
            self.no_progress_steps += 1;
        }
    }

    /// turn 边界评估（§5.7 四类信号，终止优先于纠偏，多信号同时命中取
    /// 优先级最高者）：内部更新纠偏水位——同一水平只纠偏一次。
    pub fn evaluate(&mut self) -> WatchdogAction {
        if let Some(action) = self.check_same_signature_failures() {
            return action;
        }
        if let Some(action) = self.check_no_progress() {
            return action;
        }
        if let Some(action) = self.check_readonly_repeat() {
            return action;
        }
        self.check_segmented_reads().unwrap_or(WatchdogAction::None)
    }

    /// 信号①：同签名工具连续失败（§5.7 行 1/2）。连续段 = 自窗口尾部起、
    /// 与末条同签名且全为失败的紧邻段（同签名成功或不同签名调用即打断）。
    /// 5 次终止；3 次纠偏一次（段内继续失败不再重复纠偏，段重开——锚变化
    /// ——则复位，新一轮失败累计到 3 再纠偏）。
    fn check_same_signature_failures(&mut self) -> Option<WatchdogAction> {
        let last = self.window.back()?;
        if last.ok {
            return None;
        }
        let signature = last.signature;
        let name = last.name.clone();
        let run_len = self
            .window
            .iter()
            .rev()
            .take_while(|r| r.signature == signature && !r.ok)
            .count();
        let start = self.window.len() - run_len;
        let anchor = if start == 0 {
            None
        } else {
            Some(self.window[start - 1].signature)
        };
        if anchor != self.fail_anchor {
            // 连续段重开：复位纠偏水位
            self.fail_anchor = anchor;
            self.fail_corrected = false;
        }
        if run_len >= SAME_SIGNATURE_FAIL_TERMINATE {
            return Some(WatchdogAction::Terminate(format!(
                "工具 {name} 以相同参数连续失败 {run_len} 次，运行已停滞（E_RUN_STALLED）；\
                 已输出内容与工具结果均已保留"
            )));
        }
        if run_len >= SAME_SIGNATURE_FAIL_CORRECT && !self.fail_corrected {
            self.fail_corrected = true;
            return Some(WatchdogAction::Corrective(format!(
                "[watchdog] 工具 {name} 以相同参数已连续失败 {run_len} 次：请停止重试同一调用，\
                 换一种方法达成目标（修改参数、改用其他工具，或向用户说明）。"
            )));
        }
        None
    }

    /// 信号②：无进展轮次（§5.7 行 3）。连续无进展步达到 14 终止、8 纠偏
    /// 一次（继续空转不再重复纠偏，直至 14 终止）；有进展的步在
    /// [`Self::record_step`] 已清零计数并复位纠偏水位。
    fn check_no_progress(&mut self) -> Option<WatchdogAction> {
        if self.no_progress_steps >= NO_PROGRESS_TERMINATE {
            return Some(WatchdogAction::Terminate(format!(
                "已连续 {} 步没有任何进展（无正文输出且无工具成功），运行已停滞（E_RUN_STALLED）；\
                 已输出内容与工具结果均已保留",
                self.no_progress_steps
            )));
        }
        if self.no_progress_steps >= NO_PROGRESS_CORRECT && !self.no_progress_corrected {
            self.no_progress_corrected = true;
            return Some(WatchdogAction::Corrective(format!(
                "[watchdog] 已连续 {} 步没有任何进展（无正文输出且无工具成功）：请停止空转，\
                 直接给出结论、换一种方法，或向用户说明当前遇到的障碍。",
                self.no_progress_steps
            )));
        }
        None
    }

    /// 信号③：同参数只读调用成功重复（§5.7 行 4）。按窗口内末条签名（须为
    /// 只读成功）的成功计数评估，每满 4 次纠偏一次（提示已获得过该结果）。
    fn check_readonly_repeat(&mut self) -> Option<WatchdogAction> {
        let last = self.window.back()?;
        if !last.ok || !last.read_only {
            return None;
        }
        let signature = last.signature;
        let name = last.name.clone();
        let count = self
            .window
            .iter()
            .filter(|r| r.signature == signature && r.ok)
            .count();
        let level = count / READONLY_REPEAT_CORRECT;
        if level > self.readonly_corrected_count && level >= 1 {
            self.readonly_corrected_count = level;
            return Some(WatchdogAction::Corrective(format!(
                "[watchdog] 只读工具 {name} 以相同参数已成功执行 {count} 次，结果相同：\
                 请勿重复调用，直接使用已获得的结果。"
            )));
        }
        None
    }

    /// 信号④：同文件连续分段读（§5.7 行 5）。连续段 = 自窗口尾部起、对同一
    /// 文件路径的 read 调用紧邻段（读别的文件或调用其他工具即打断），每满
    /// 10 次纠偏一次（提示一次性读取更大窗口或改用 grep 定位，避免碎片化
    /// 读取）。
    fn check_segmented_reads(&mut self) -> Option<WatchdogAction> {
        let last = self.window.back()?;
        let path = last.read_path.clone()?;
        let run_len = self
            .window
            .iter()
            .rev()
            .take_while(|r| r.read_path.as_deref() == Some(path.as_str()))
            .count();
        let level = run_len / SEGMENTED_READ_CORRECT;
        if level > self.segmented_corrected_count && level >= 1 {
            self.segmented_corrected_count = level;
            return Some(WatchdogAction::Corrective(format!(
                "[watchdog] 已对同一文件（{path}）连续分段读取 {run_len} 次：\
                 请一次性读取更大窗口（read 的 offset/limit）、用 batch_read 一次读取多个文件，\
                 或改用 grep 定位目标内容，避免碎片化读取。"
            )));
        }
        None
    }
}

/// §5.7 有效模型增量判定：三类增量帧（textDelta / reasoningDelta / toolArgs）
/// 到过一个即刷新停滞计时；其余帧（生命周期事件与帧面非增量帧）不刷新——
/// 前端帧、重连与工具进度天然不能给模型停滞计时续命。
pub fn is_model_delta(frame: &ChatFrame) -> bool {
    matches!(
        frame,
        ChatFrame::TextDelta { .. } | ChatFrame::ReasoningDelta { .. } | ChatFrame::ToolArgs { .. }
    )
}

/// 模型流停滞计时器（§5.7 计时边界，Q33）：随单次模型请求生灭。
///
/// - `armed` 即启动计时（等价于「请求发出」时刻——`drive_run` 在发起流式
///   调用的同一拍构造）；
/// - 刷新（`refresh`）把截止时刻顺延一个完整超时周期——只由有效模型增量
///   （[`is_model_delta`]）驱动；
/// - 等待端（`expired`）在截止时刻到达时返回；期间 deadline 被刷新则继续
///   等新值。锁只包瞬时读写、不跨 await，流循环内的刷新与 select 等待互不
///   阻塞。
///
/// 超时值可注入（生产 = `watchdog.stallTimeoutSecs`，测试 = 毫秒级小值），
/// 停滞验证不依赖真实 300 秒等待。
#[derive(Debug)]
pub struct StallGuard {
    /// 单次刷新顺延的超时周期。
    timeout: Duration,
    /// 当前截止时刻（互斥锁保护；刷新端与等待端共享）。
    deadline: Mutex<TokioInstant>,
}

/// StallGuard 的构造、刷新与停滞等待。
impl StallGuard {
    /// 启动计时（= 模型请求发出）：截止时刻 = 当前 + timeout。
    pub fn armed(timeout: Duration) -> Self {
        Self {
            timeout,
            deadline: Mutex::new(TokioInstant::now() + timeout),
        }
    }

    /// 有效模型增量到达：截止时刻顺延一个完整超时周期。
    pub fn refresh(&self) {
        let next = TokioInstant::now() + self.timeout;
        *self
            .deadline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
    }

    /// 停滞截止等待：截止时刻到达即返回；期间被刷新则改等新值。
    pub async fn expired(&self) {
        loop {
            let deadline = *self
                .deadline
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if TokioInstant::now() >= deadline {
                return;
            }
            tokio::time::sleep_until(deadline).await;
        }
    }
}

// 单元测试挂载：测试代码放在独立文件 watchdog_test.rs，仅 cargo test 时编译。
#[cfg(test)]
#[path = "watchdog_test.rs"]
mod watchdog_test;
