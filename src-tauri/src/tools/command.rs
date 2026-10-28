//! command 工具（§8.3 Network* 行 + §8.8 命令输出治理）。
//!
//! - 承载：Unix `sh -c` / Windows `powershell -NoProfile -Command`；
//! - 收割：spawn 即入独立进程组（Unix）/Job Object（Windows），超时与取消
//!   时整树收割（`tools::process` 原语，S5 实证 6–8ms）；
//! - 输出治理：stdout+stderr 合流 → ANSI/OSC 清洗 → >96KB 全量 spill 至
//!   `<data>/logs/spill/<callId>.txt`，工具结果只带尾部 16KB + spill 路径
//!   （24h lazy 清理，best-effort）；
//! - 超时：默认 120s（1–600）→ 收割 → ok=false E_COMMAND_TIMEOUT，
//!   已收集输出保留在 data；
//! - 取消：token 触发 → 收割 → Cancelled（已产生副作用不回滚）。
//!
//! 能力标 Network（矩阵语义），围栏判定在 ApprovalGate（Block 硬拦 /
//! Confirm 挂起），本文件只管执行。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::safety::tier::Capability;
use crate::tools::process::REAP_GRACE;
#[cfg(unix)]
use crate::tools::process::enable_process_group;
use crate::util::errors::ToolError;

use super::{ToolAnnotations, ToolOutcome, ToolSpec, ToolsContext};

/// 超时默认与区间（§8.3：1–600s 默认 120）。
pub const DEFAULT_TIMEOUT_SEC: u32 = 120;
pub const MIN_TIMEOUT_SEC: u32 = 1;
pub const MAX_TIMEOUT_SEC: u32 = 600;

/// 全量输出 spill 阈值与结果保留尾部（§8.8：>96KB spill，尾部 16KB）。
pub const SPILL_THRESHOLD: usize = 96 * 1024;
pub const TAIL_BYTES: usize = 16 * 1024;

/// spill 保留时长（§8.8：24h 清理；每次执行 lazy 清理过期文件）。
pub const SPILL_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// command 工具参数（§8.3）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandArgs {
    /// 待执行的命令文本（整行交给 shell 解释）。
    pub command: String,
    /// 超时秒数（1–600，缺省 120）。
    #[serde(default)]
    pub timeout_sec: Option<u32>,
    /// 工作目录（roots 内的相对或绝对路径，缺省项目主目录）。
    #[serde(default)]
    pub cwd: Option<String>,
}

/// 取消可感知的执行退出态（executor 映射 ToolExecution）。
pub enum CommandExit {
    /// 正常结束（含超时/启动失败等工具级失败，结果在 ToolOutcome 里）。
    Done(Box<ToolOutcome>),
    /// 取消：executed=true（进程已启动被收割，副作用不回滚）。
    Cancelled,
}

/// 执行一条命令（门控放行后调用）。`call_id` 为模型 call id（spill 文件名）。
pub async fn execute(
    ctx: &ToolsContext,
    call_id: &str,
    args: &CommandArgs,
    token: &CancellationToken,
) -> CommandExit {
    match prepare(ctx, args) {
        Err(err) => CommandExit::Done(Box::new(ToolOutcome::from(err))),
        Ok((program, argv, cwd, timeout)) => {
            run_prepared(
                ctx,
                call_id,
                &args.command,
                program,
                argv,
                cwd,
                timeout,
                token,
            )
            .await
        }
    }
}

/// prepare 的产物：shell 程序名、参数、工作目录（已校验）与超时时长。
type Prepared = (String, Vec<String>, Option<PathBuf>, Duration);

/// 参数校验 + shell 形态决议。
fn prepare(ctx: &ToolsContext, args: &CommandArgs) -> Result<Prepared, ToolError> {
    if args.command.trim().is_empty() {
        return Err(ToolError::invalid_arg("command 不能为空"));
    }
    let timeout_sec = args.timeout_sec.unwrap_or(DEFAULT_TIMEOUT_SEC);
    if !(MIN_TIMEOUT_SEC..=MAX_TIMEOUT_SEC).contains(&timeout_sec) {
        return Err(ToolError::invalid_arg(format!(
            "timeoutSec 须在 {MIN_TIMEOUT_SEC}–{MAX_TIMEOUT_SEC}s（当前 {timeout_sec}）"
        )));
    }
    let cwd = match &args.cwd {
        Some(raw) => {
            let guard = ctx.guard()?;
            let dir = guard.resolve(raw)?;
            if !dir.is_dir() {
                return Err(ToolError::invalid_arg(format!("cwd 不是目录：{raw}")));
            }
            Some(dir)
        }
        None => None,
    };
    #[cfg(unix)]
    let (program, argv) = (
        "sh".to_string(),
        vec!["-c".to_string(), args.command.clone()],
    );
    #[cfg(windows)]
    let (program, argv) = (
        "powershell".to_string(),
        vec![
            "-NoProfile".to_string(),
            "-Command".to_string(),
            args.command.clone(),
        ],
    );
    Ok((program, argv, cwd, Duration::from_secs(timeout_sec as u64)))
}

/// 已校验命令的执行主体：合流读取 + 超时/取消收割 + 输出治理。
#[allow(clippy::too_many_arguments)]
async fn run_prepared(
    ctx: &ToolsContext,
    call_id: &str,
    raw_command: &str,
    program: String,
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    timeout: Duration,
    token: &CancellationToken,
) -> CommandExit {
    cleanup_spill(ctx, Instant::now());

    let mut cmd = tokio::process::Command::new(&program);
    // stdin 接 /dev/null：防止命令等待键盘输入把会话挂死；
    // stdout/stderr 接管为管道，由本循环合流读取。
    cmd.args(&argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(cwd.unwrap_or_else(|| ctx.project_root.clone()));
    #[cfg(unix)]
    enable_process_group(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CommandExit::Done(Box::new(ToolOutcome::failure(
                ToolError::new(
                    "E_INTERNAL_STATE",
                    format!("命令启动失败（{program}）：{e}"),
                ),
                "命令启动失败",
            )));
        }
    };
    #[cfg(windows)]
    let job = match crate::tools::process::JobGuard::new().and_then(|g| {
        g.assign(&child)?;
        Ok(g)
    }) {
        Ok(g) => g,
        Err(e) => {
            return CommandExit::Done(Box::new(ToolOutcome::failure(
                ToolError::new("E_INTERNAL_STATE", format!("Job Object 指派失败：{e}")),
                "命令启动失败",
            )));
        }
    };

    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut merged: Vec<u8> = Vec::new();
    let mut out_buf = [0u8; 8 * 1024];
    let mut err_buf = [0u8; 8 * 1024];
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let mut cancelled = false;

    // 主循环：同时监听取消信号、超时与两路输出。
    // biased 保证先查取消再查超时（用户取消优先于超时报错口径）。
    loop {
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                cancelled = true;
                break;
            }
            _ = tokio::time::sleep_until(deadline.into()) => {
                timed_out = true;
                break;
            }
            read = stdout.read(&mut out_buf) => match read {
                Ok(0) | Err(_) => {
                    // stdout 关闭后只剩 stderr 与计时（内层去掉已关闭的流，
                    // 避免对同一管道反复读到 EOF 空转）。
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => { cancelled = true; break; }
                        _ = tokio::time::sleep_until(deadline.into()) => { timed_out = true; break; }
                        read = stderr.read(&mut err_buf) => match read {
                            Ok(0) | Err(_) => break,
                            Ok(n) => merged.extend_from_slice(&err_buf[..n]),
                        }
                    }
                }
                Ok(n) => merged.extend_from_slice(&out_buf[..n]),
            },
            read = stderr.read(&mut err_buf) => match read {
                Ok(0) | Err(_) => {
                    // stderr 关闭后只剩 stdout 与计时（对称处理）。
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => { cancelled = true; break; }
                        _ = tokio::time::sleep_until(deadline.into()) => { timed_out = true; break; }
                        read = stdout.read(&mut out_buf) => match read {
                            Ok(0) | Err(_) => break,
                            Ok(n) => merged.extend_from_slice(&out_buf[..n]),
                        }
                    }
                }
                Ok(n) => merged.extend_from_slice(&err_buf[..n]),
            },
        }
    }

    if cancelled || timed_out {
        // 整树收割（超时/取消同路径；Unix SIGTERM→宽限→SIGKILL）。
        // 防的是：shell 只是父进程，孙进程不随父退出而残留占资源，
        // 故对整个进程组/Job 发信号而非只 kill 直接子进程。
        #[cfg(unix)]
        let reaped = crate::tools::process::reap_process_group(&mut child, REAP_GRACE).await;
        #[cfg(windows)]
        let reaped = job.reap(&mut child, REAP_GRACE).await;
        let _ = reaped; // 收割失败也已尽力（句柄 drop 兜底）
        if cancelled {
            return CommandExit::Cancelled;
        }
    }

    let status = child.wait().await;
    let cleaned = strip_ansi(&String::from_utf8_lossy(&merged));
    let exit_code = status.ok().and_then(|s| s.code());
    let line_count = cleaned.lines().count();

    let mut data = serde_json::json!({
        "exitCode": exit_code,
        "timedOut": timed_out,
        "output": tail_of(&cleaned, TAIL_BYTES),
    });
    let mut ui_summary = if timed_out {
        format!("命令超时（{}s，已收割）", timeout.as_secs())
    } else {
        match exit_code {
            Some(0) => format!("命令完成（{line_count} 行输出）"),
            Some(code) => format!("命令退出码 {code}（{line_count} 行输出）"),
            None => format!("命令异常终止（{line_count} 行输出）"),
        }
    };

    if cleaned.len() > SPILL_THRESHOLD {
        match spill(ctx, call_id, &cleaned) {
            Ok(path) => {
                data["spillPath"] = serde_json::Value::String(path.clone());
                ui_summary.push_str(&format!("（全量已落盘 {path}）"));
            }
            Err(_) => {
                // spill 失败不阻断：结果仍带尾部，模型侧可再次分段获取
                ui_summary.push_str("（全量落盘失败，仅尾部）");
            }
        }
    }

    if timed_out {
        return CommandExit::Done(Box::new(ToolOutcome {
            ok: false,
            data: Some(data),
            error: Some(ToolError::command_timeout(format!(
                "命令超时（{}s，进程组已收割）：{raw_command}",
                timeout.as_secs()
            ))),
            ui_summary,
            detail: None,
        }));
    }
    let ok = exit_code == Some(0);
    CommandExit::Done(Box::new(ToolOutcome {
        ok,
        data: Some(data),
        error: if ok {
            None
        } else {
            Some(ToolError::new(
                "E_INTERNAL_STATE",
                format!("命令未成功退出（exitCode={exit_code:?}）"),
            ))
        },
        ui_summary,
        detail: None,
    }))
}

/// ANSI/OSC 转义清洗（§8.8）。anstream 的 strip 适配器（纯 Rust）。
fn strip_ansi(s: &str) -> String {
    anstream::adapter::strip_str(s).to_string()
}

/// 尾部 `tail` 字节（UTF-8 字符边界安全）。
fn tail_of(s: &str, tail: usize) -> &str {
    if s.len() <= tail {
        return s;
    }
    let mut start = s.len() - tail;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// spill 目录（§7.1：`<data>/logs/spill`）。
fn spill_dir(ctx: &ToolsContext) -> PathBuf {
    ctx.data_root.join("logs").join("spill")
}

/// 全量输出落盘；返回落盘路径字符串。
fn spill(ctx: &ToolsContext, call_id: &str, full: &str) -> Result<String, std::io::Error> {
    let dir = spill_dir(ctx);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{call_id}.txt"));
    std::fs::write(&path, full)?;
    Ok(path.display().to_string())
}

/// 24h lazy 清理（§8.8）：删除过期 spill 文件，best-effort。
fn cleanup_spill(ctx: &ToolsContext, now: Instant) {
    let dir = spill_dir(ctx);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let expired = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > SPILL_TTL);
        if expired {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let _ = now; // 保留形参锚点（清理基准=文件 mtime，非当前 Instant）
}

/// command 工具规格（能力 Network：plan 档过滤、非 full_access 档审批）。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "command",
        description: "在工作区执行 shell 命令并返回合流输出。stdout/stderr 合并、\
自动清理控制字符；超过 96KB 时全量落盘并只返回尾部。危险命令会被拦截或要求审批。\
超时默认 120 秒（1–600），超时或用户取消时整个进程组被终止。"
            .to_string(),
        capability: Capability::Network,
        params_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "timeoutSec": { "type": "integer", "minimum": 1, "maximum": 600 },
                "cwd": { "type": "string" }
            },
            "required": ["command"],
            "additionalProperties": false
        }),
        annotations: ToolAnnotations {
            read_only_hint: false,
            destructive_hint: true,
            idempotent_hint: false,
            open_world_hint: false,
        },
    }
}
