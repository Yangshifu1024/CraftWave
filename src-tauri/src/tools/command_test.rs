//! command 工具单测（§8.3/§8.8）：退出码/合流输出/ANSI 清洗/超时收割/
//! 取消收割/spill 治理/cwd 边界。Unix 路径（sh）；Windows 行为由三平台 CI 覆盖。

use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use super::command::{CommandArgs, CommandExit, MAX_TIMEOUT_SEC, SPILL_THRESHOLD, execute};
use crate::util::test_support::TestWorkspace;

/// 测试辅助：构造默认参数（无超时、无 cwd）的命令参数。
fn args(command: &str) -> CommandArgs {
    CommandArgs {
        command: command.to_owned(),
        timeout_sec: None,
        cwd: None,
    }
}

// 断言：普通命令正常执行，退出码 0、输出可见、timedOut=false。
#[tokio::test]
async fn simple_command_exit_code_and_output() {
    let ws = TestWorkspace::new();
    std::fs::write(ws.project_file("a.txt"), "hello").unwrap();
    let exit = execute(
        &ws.ctx(),
        "c1",
        &args("cat a.txt"),
        &CancellationToken::new(),
    )
    .await;
    let CommandExit::Done(outcome) = exit else {
        panic!("应 Done")
    };
    assert!(outcome.ok);
    let data = outcome.data.unwrap();
    assert_eq!(data["exitCode"], 0);
    assert!(data["output"].as_str().unwrap().contains("hello"));
    assert_eq!(data["timedOut"], false);
}

// 断言：stdout 与 stderr 合流进同一段输出，非零退出码 ok=false。
#[tokio::test]
async fn stdout_stderr_merged_nonzero_exit() {
    let ws = TestWorkspace::new();
    let exit = execute(
        &ws.ctx(),
        "c2",
        &args("echo OUT; echo ERR >&2; exit 3"),
        &CancellationToken::new(),
    )
    .await;
    let CommandExit::Done(outcome) = exit else {
        panic!("应 Done")
    };
    assert!(!outcome.ok);
    let data = outcome.data.unwrap();
    assert_eq!(data["exitCode"], 3);
    let output = data["output"].as_str().unwrap();
    assert!(
        output.contains("OUT") && output.contains("ERR"),
        "合流：{output}"
    );
}

// 断言：ANSI/OSC 转义序列被清洗（正文保留、控制字符不残留）。
#[tokio::test]
async fn ansi_escapes_stripped() {
    let ws = TestWorkspace::new();
    let exit = execute(
        &ws.ctx(),
        "c3",
        &args(r#"printf '\033[31m红\033[0m|\x1b]0;title\x07'"#),
        &CancellationToken::new(),
    )
    .await;
    let CommandExit::Done(outcome) = exit else {
        panic!("应 Done")
    };
    let output = outcome.data.unwrap()["output"].as_str().unwrap().to_owned();
    assert!(output.contains("红"), "正文保留：{output:?}");
    assert!(!output.contains('\u{1b}'), "CSI/OSC 全清洗：{output:?}");
}

// 断言：超时命令被及时收割（E_COMMAND_TIMEOUT），已收集的输出仍保留在 data。
#[tokio::test]
async fn timeout_reap_error_code_output_kept() {
    let ws = TestWorkspace::new();
    let mut a = args("echo STARTED; sleep 30");
    a.timeout_sec = Some(1);
    let t = Instant::now();
    let exit = execute(&ws.ctx(), "c4", &a, &CancellationToken::new()).await;
    let CommandExit::Done(outcome) = exit else {
        panic!("超时应 Done（工具级失败）")
    };
    assert!(!outcome.ok);
    let err = outcome.error.unwrap();
    assert_eq!(err.code, "E_COMMAND_TIMEOUT");
    let data = outcome.data.unwrap();
    assert_eq!(data["timedOut"], true);
    assert!(data["output"].as_str().unwrap().contains("STARTED"));
    assert!(t.elapsed() < Duration::from_secs(5), "1s 超时应及时收割");
}

// 断言：取消令牌触发后执行返回 Cancelled 且进程组被收割。
#[tokio::test]
async fn cancel_reaps_process_group() {
    let ws = TestWorkspace::new();
    let token = CancellationToken::new();
    let ctx = ws.ctx();
    let token_for_task = token.clone();
    let task = tokio::spawn(async move {
        execute(&ctx, "c5", &args("echo A; sleep 30"), &token_for_task).await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    token.cancel();
    assert!(matches!(task.await.unwrap(), CommandExit::Cancelled));
}

// 断言：超 96KB 的输出全量落盘（spill），工具结果只带尾部 16KB 与落盘路径。
#[tokio::test]
async fn large_output_spilled_tail_and_path() {
    let ws = TestWorkspace::new();
    // 产出 > 96KB：约 120KB 文本
    let exit = execute(
        &ws.ctx(),
        "c6",
        &args("yes 0123456789abcdefghijklmnopqrstuvwxyz | head -c 120000"),
        &CancellationToken::new(),
    )
    .await;
    let CommandExit::Done(outcome) = exit else {
        panic!("应 Done")
    };
    assert!(outcome.ok);
    let data = outcome.data.unwrap();
    let output = data["output"].as_str().unwrap();
    assert!(
        output.len() <= 16 * 1024 + 128,
        "结果只带尾部 16KB：{}",
        output.len()
    );
    let spill_path = data["spillPath"].as_str().expect("应有 spill 路径");
    let full = std::fs::read(spill_path).expect("spill 文件可读");
    assert!(full.len() > SPILL_THRESHOLD, "全量已落盘：{}", full.len());
}

// 断言：cwd 在 roots 外被拒（E_PATH_OUTSIDE_ROOTS），roots 内相对路径放行。
#[tokio::test]
async fn cwd_boundary_outside_rejected_inside_ok() {
    let ws = TestWorkspace::new();
    let outside = tempfile::tempdir().unwrap();
    // 越界 cwd
    let mut a = args("pwd");
    a.cwd = Some(outside.path().display().to_string());
    let exit = execute(&ws.ctx(), "c7", &a, &CancellationToken::new()).await;
    let CommandExit::Done(outcome) = exit else {
        panic!("应 Done")
    };
    assert!(!outcome.ok);
    assert_eq!(outcome.error.unwrap().code, "E_PATH_OUTSIDE_ROOTS");

    // 合法 cwd（相对）
    let mut b = args("pwd");
    b.cwd = Some(".".to_owned());
    let exit = execute(&ws.ctx(), "c8", &b, &CancellationToken::new()).await;
    let CommandExit::Done(outcome) = exit else {
        panic!("应 Done")
    };
    assert!(outcome.ok, "cwd 在 roots 内应放行");
}

// 断言：空命令与超范围 timeoutSec 均以 E_INVALID_ARG 拒绝（参数校验）。
#[tokio::test]
async fn validation_empty_command_timeout_range() {
    let ws = TestWorkspace::new();
    let exit = execute(&ws.ctx(), "c9", &args("  "), &CancellationToken::new()).await;
    let CommandExit::Done(outcome) = exit else {
        panic!("应 Done")
    };
    assert_eq!(outcome.error.unwrap().code, "E_INVALID_ARG");

    let mut a = args("true");
    a.timeout_sec = Some(MAX_TIMEOUT_SEC + 1);
    let exit = execute(&ws.ctx(), "c10", &a, &CancellationToken::new()).await;
    let CommandExit::Done(outcome) = exit else {
        panic!("应 Done")
    };
    assert_eq!(outcome.error.unwrap().code, "E_INVALID_ARG");
}
