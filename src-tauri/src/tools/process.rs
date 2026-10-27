//! 进程组 / Job Object 收割原语（§8.8 命令输出治理 + §5.3 取消传播）。
//!
//! 产品化自 S5 spike（`spike/cancel.rs` 的收割段，原型冻结保留）；本票先把
//! 原语落位——command 工具属工具系统后续票，落地时直接接入：
//! - Unix：spawn 即 `process_group(0)`（子进程为组长），取消/超时
//!   SIGTERM → 宽限轮询 → SIGKILL 整组（孙进程随组消亡，S5 实测 6–8ms）；
//! - Windows：Job Object（`KILL_ON_JOB_CLOSE` 兜底 + `TerminateJobObject`
//!   整树），MSVC target 可编译性已验（S5），运行行为由三平台 CI 覆盖。

use std::time::Duration;

/// 收割宽限期：SIGTERM 后等待子进程自行退出，超时 SIGKILL。
/// 预算约束：取消全链 <1s，宽限只允许占小头（§8.8 命令治理）。
pub const REAP_GRACE: Duration = Duration::from_millis(200);

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
    /// 组内孙进程随组消亡（由 S5 真实进程树测试验证）。
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

// —— Windows：Job Object 整树收割（cfg 隔离保证可编译；语义注释同 S5 票） ——

#[cfg(windows)]
mod win_job {
    use std::time::Duration;

    /// Job Object 守护句柄：KILL_ON_JOB_CLOSE 兜底 + TerminateJobObject 整树收割。
    /// 落地要点（spawn→assign 后代逃逸窗口以 CREATE_SUSPENDED 归零等）见
    /// 08 号票 Answer 第 4 节；command 工具落地时接入。
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

#[cfg(all(test, unix))]
#[path = "process_test.rs"]
mod process_test;
