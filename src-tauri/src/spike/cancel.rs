//! 【S5 Spike，用后即弃】取消传播全链路原型（技术设计文档 §5.3 / 附录 A-S5）。
//!
//! 在 S2+S3 钉死的 backend 形态（`AgentRun` sans-IO 步进 + 手动工具循环）上，
//! 验证 §5.3 的取消传播：`cancel_run` 置 `CancellationToken` → 模型流 select! 退出
//! （流 drop）/ 进行中命令进程组收割 → 收齐工具结果并补 `E_CANCELLED` 标记 →
//! `runEnded(cancelled)` 帧。全链时延由 tests/spike_s5_cancel.rs 实测（<1s 预算）。
//!
//! 正式实现应落在 backend（RunLoop）与 tools（command 执行器）——本模块只回答
//! 「取消语义在 S2+S3 形态上是否成立、收割路径是否够快」。
//!
//! 平台分支：Unix 进程组（`process_group(0)` + `kill(-pgid)`）；Windows Job Object
//! （`AssignProcessToJobObject` + `TerminateJobObject`，纸面确认见票 Answer）。
//! 两分支各自 cfg 隔离，保证 Windows 目标可编译（CI / cargo check --target）。

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use super::frames::ChatFrame;
use super::turn::TurnDriver;
use rig::run::PromptError;
use rig::run::{AgentRun, AgentRunStep};

/// 被取消工具结果补挂的错误码（§3.4 toolResult.error.code / §3.6 错误码注册表）。
pub const E_CANCELLED: &str = "E_CANCELLED";

/// 取消收割的宽限期：SIGTERM 后等待子进程自行退出，超时 SIGKILL。
/// 预算约束：全链 <1s，宽限只允许占小头（§8.8 命令治理）。
pub const REAP_GRACE: Duration = Duration::from_millis(200);

/// 工具执行结果（§5.3「取消中工具收齐」的语义载体）。
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutcome {
    /// 已完成：结果原样保留（已写文件不回滚），ok=true，无取消标记。
    Completed(String),
    /// 取消终止：补 `E_CANCELLED` 标记，ok=false；message 注明收割/未执行。
    Cancelled(String),
}

/// 取消可感知的工具执行器（生产 = tools 模块的 command/文件执行器，§5.4）。
pub trait CancellableExecutor: Send + Sync {
    /// 执行一个工具调用。实现内部以 `token` 感知取消（如命令工具在
    /// select! 里收割进程组）；完成与否都以 ToolOutcome 收齐。
    fn execute<'a>(
        &'a self,
        call_id: &'a str,
        name: &'a str,
        args_json: &'a str,
        token: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ToolOutcome> + Send + 'a>>;
}

/// 手动循环的退出态：正常完成（Done）或取消终结（Cancelled）。
#[derive(Debug)]
pub enum RunExit {
    /// 自然终态（§5.3 Streaming → Done）。装箱以压枚举尺寸（clippy large_enum_variant）。
    Done(Box<rig::run::response::PromptResponse>),
    /// 取消终态：工具结果已收齐并补标记，runEnded(cancelled) 已发出。
    Cancelled,
}

/// 取消可感知的手动工具循环（S2+S3 `drive_manual_tool_loop` 的取消版）：
///
/// - `CallModel`：流式调用内部 select token，取消即流 drop（`StreamExit::Cancelled`）；
/// - `CallTools`：逐个执行，任一工具报 `Cancelled` 后，本批剩余工具不再执行、
///   一律补 `E_CANCELLED`（收齐语义），已完成的保留原结果（不回滚）；
/// - 终态：统一在此发 `runEnded`（reason=done/cancelled），保证它是最后一帧。
pub async fn drive_cancellable_run(
    driver: &TurnDriver,
    mut run: AgentRun,
    token: CancellationToken,
    executor: std::sync::Arc<dyn CancellableExecutor>,
    on_frame: &mut (dyn FnMut(ChatFrame) + Send),
) -> Result<RunExit, PromptError> {
    let started_at = Instant::now();
    let mut last_turn = 1u32;
    let mut tool_count = 0u32;
    loop {
        match run.next_step()? {
            AgentRunStep::CallModel {
                prompt,
                history,
                turn,
            } => {
                last_turn = turn as u32;
                if let super::turn::StreamExit::Cancelled = driver
                    .stream_turn_cancellable(
                        &mut run,
                        prompt,
                        history,
                        turn as u32,
                        &token,
                        on_frame,
                    )
                    .await?
                {
                    // 模型流已停：无工具进行中，直接取消终结
                    return Ok(finish_cancelled(
                        on_frame, started_at, tool_count, last_turn,
                    ));
                }
            }
            AgentRunStep::CallTools { calls } => {
                let mut answers = Vec::new();
                let mut cancel_seen = false;
                for call in calls {
                    let (call_id, name, args_json) = pending_parts(&call);
                    if cancel_seen || token.is_cancelled() {
                        // 收齐语义：批内未执行的工具也补 E_CANCELLED，不留悬空调用
                        on_frame(cancelled_result(&call_id, "取消：工具未执行"));
                        tool_count += 1;
                        continue;
                    }
                    tool_count += 1;
                    match executor.execute(&call_id, &name, &args_json, &token).await {
                        ToolOutcome::Completed(output) => {
                            // 已完成的结果原样保留（已写文件不回滚），ok=true
                            on_frame(ChatFrame::ToolResult {
                                call_id: call_id.clone(),
                                ok: true,
                                ui_summary: output.clone(),
                                error: None,
                            });
                            answers.push(super::turn::answer_one(call, output));
                        }
                        ToolOutcome::Cancelled(message) => {
                            cancel_seen = true;
                            on_frame(cancelled_result(&call_id, &message));
                        }
                    }
                }
                if cancel_seen {
                    // 收齐后才发终态帧（§5.3 不变量：取消中工具收齐 → runEnded(cancelled)）
                    return Ok(finish_cancelled(
                        on_frame, started_at, tool_count, last_turn,
                    ));
                }
                run.answer_all(answers)?;
            }
            AgentRunStep::Done(response) => {
                on_frame(ChatFrame::RunEnded {
                    reason: super::frames::EndReason::Done,
                    duration_ms: started_at.elapsed().as_millis() as u64,
                    tool_count,
                    turn: last_turn,
                });
                return Ok(RunExit::Done(Box::new(response)));
            }
        }
    }
}

/// 发出取消终态帧并返回取消退出态（runEnded(cancelled) 必为最后一帧）。
fn finish_cancelled(
    on_frame: &mut dyn FnMut(ChatFrame),
    started_at: Instant,
    tool_count: u32,
    turn: u32,
) -> RunExit {
    on_frame(ChatFrame::RunEnded {
        reason: super::frames::EndReason::Cancelled,
        duration_ms: started_at.elapsed().as_millis() as u64,
        tool_count,
        turn,
    });
    RunExit::Cancelled
}

/// 被取消工具的 toolResult 帧：ok=false + error.code=E_CANCELLED（§3.4 error 面）。
fn cancelled_result(call_id: &str, message: &str) -> ChatFrame {
    ChatFrame::ToolResult {
        call_id: call_id.to_owned(),
        ok: false,
        ui_summary: format!("{E_CANCELLED}: {message}"),
        error: Some(super::frames::FrameError {
            code: E_CANCELLED.to_owned(),
            message: message.to_owned(),
        }),
    }
}

/// 从 pending call 提取 (callId, name, argsJson)（与 S2+S3 桥接同构）。
fn pending_parts(call: &rig::run::PendingToolCall) -> (String, String, String) {
    match call {
        rig::run::PendingToolCall::Execute(exec) => (
            exec.id().to_string(),
            exec.name().to_string(),
            exec.arguments().to_string(),
        ),
        rig::run::PendingToolCall::Malformed(malformed) => (
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

// —— Unix：进程组收割（macOS 实测路径） ——

#[cfg(unix)]
mod unix_reap {
    use std::time::{Duration, Instant};

    /// 子进程落在独立进程组（组长，pgid = 子 pid）：
    /// cancel/超时时 `kill(-pgid)` 一并收割其派生的整棵进程树（§8.8）。
    pub fn enable_process_group(cmd: &mut tokio::process::Command) {
        cmd.process_group(0);
    }

    /// 收割整个进程组：SIGTERM → 宽限轮询 → SIGKILL，并等待直接子进程退出。
    /// 组内孙进程随组消亡（由测试用真实进程树验证）。
    pub async fn reap_process_group(
        child: &mut tokio::process::Child,
        grace: Duration,
    ) -> Result<std::process::ExitStatus, String> {
        let pid = child
            .id()
            .ok_or_else(|| "子进程已退出，pid 不可得（无需收割）".to_string())?
            as i32;
        // 负 pid = 对整组发信号（kill(2)）；组已不存在时返回 ESRCH，忽略即可
        let _ = unsafe { libc::kill(-pid, libc::SIGTERM) };
        let deadline = Instant::now() + grace;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(status),
                Ok(None) if Instant::now() >= deadline => break,
                Ok(None) => tokio::time::sleep(Duration::from_millis(5)).await,
                Err(e) => return Err(e.to_string()),
            }
        }
        let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
        child.wait().await.map_err(|e| e.to_string())
    }

    /// 进程是否存活（kill(pid, 0) 探测）。
    pub fn pid_alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }
}

#[cfg(unix)]
pub use unix_reap::{enable_process_group, pid_alive, reap_process_group};

// —— Windows：Job Object 整树收割（纸面方案，cfg 隔离保证可编译） ——

#[cfg(windows)]
mod win_job {
    use std::time::Duration;

    /// Job Object 守护句柄：KILL_ON_JOB_CLOSE 兜底 + TerminateJobObject 整树收割。
    ///
    /// Rust 侧落地要点（依据 Microsoft Learn《Job Objects》/ jobapi2 API 文档）：
    /// 1. CreateJobObjectW 建 Job；SetInformationJobObject(JobObjectExtendedLimit-
    ///    Information) 设 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`——RunHandle 意外 drop
    ///    时句柄关闭即整树终止，杜绝孤儿进程；
    /// 2. 子进程 spawn 后 `AssignProcessToJobObject(child.raw_handle())`：此后子进程
    ///    派生的全部后代自动入 Job（除非显式 CREATE_BREAKAWAY_FROM_JOB）；
    /// 3. 取消/超时：`TerminateJobObject(job, code)` 同步整树终止（对应 Unix 的
    ///    `kill(-pgid, SIGKILL)`），随后 child.wait() 收尸；
    /// 4. spawn→assign 之间存在后代逃逸窗口（shell 类命令启动即 fork），生产应
    ///    以 `creation_flags(CREATE_SUSPENDED)` 起进程、assign 后 ResumeThread 主线程
    ///    归零窗口——std 不暴露子进程主线程句柄，届时需 windows-sys 全 FFI 的
    ///    CreateProcessW（spike 骨架先用 std spawn + 立即 assign 表达语义）。
    pub struct JobGuard {
        handle: windows_sys::Win32::Foundation::HANDLE,
    }

    impl JobGuard {
        pub fn new() -> Result<Self, String> {
            use windows_sys::Win32::Foundation::CloseHandle;
            use windows_sys::Win32::System::JobObjects::*;
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err("CreateJobObjectW 失败".into());
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    CloseHandle(handle);
                    return Err("SetInformationJobObject 失败".into());
                }
                Ok(Self { handle })
            }
        }

        /// 把已 spawn 的子进程指派进 Job（后代自动入组）。
        pub fn assign(&self, child: &tokio::process::Child) -> Result<(), String> {
            use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
            let handle = child
                .raw_handle()
                .ok_or_else(|| "子进程句柄不可得".to_string())?;
            unsafe {
                if AssignProcessToJobObject(self.handle, handle) == 0 {
                    return Err("AssignProcessToJobObject 失败".into());
                }
            }
            Ok(())
        }

        /// 整树收割：同步终止 Job 内全部进程（取消/超时路径）。
        pub fn terminate(&self, exit_code: u32) -> Result<(), String> {
            use windows_sys::Win32::System::JobObjects::TerminateJobObject;
            unsafe {
                if TerminateJobObject(self.handle, exit_code) == 0 {
                    return Err("TerminateJobObject 失败".into());
                }
            }
            Ok(())
        }

        /// 宽限等待（与 Unix reap 的 grace 语义对齐；terminate 已同步生效）。
        pub async fn reap(
            &self,
            child: &mut tokio::process::Child,
            grace: Duration,
        ) -> Result<std::process::ExitStatus, String> {
            self.terminate(130)?;
            let deadline = std::time::Instant::now() + grace;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => return Ok(status),
                    Ok(None) if std::time::Instant::now() >= deadline => {
                        return Err("Job 终止后子进程仍未退出".into());
                    }
                    Ok(None) => tokio::time::sleep(Duration::from_millis(5)).await,
                    Err(e) => return Err(e.to_string()),
                }
            }
        }
    }

    impl Drop for JobGuard {
        fn drop(&mut self) {
            use windows_sys::Win32::Foundation::CloseHandle;
            unsafe { CloseHandle(self.handle) };
            // KILL_ON_JOB_CLOSE：句柄关闭时 Job 内残余进程整树终止（兜底）
        }
    }
}

#[cfg(windows)]
pub use win_job::JobGuard;
