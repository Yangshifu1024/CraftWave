//! command 工具单测：退出码/合流输出/ANSI 清洗/超时收割/
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

/// 测试辅助：执行并取 Done 形态（测试路径不应取消）。
async fn done(
    ws: &TestWorkspace,
    call_id: &str,
    args: &CommandArgs,
    token: &CancellationToken,
) -> crate::tools::ToolOutcome {
    let ctx = ws.ctx();
    match execute(&ctx, call_id, args, token).await {
        CommandExit::Done(outcome) => *outcome,
        CommandExit::Cancelled => panic!("应 Done"),
    }
}

// 断言：执行输出矩阵——普通命令退出码 0、输出可见、timedOut=false；
// stdout 与 stderr 合流、非零退出码 ok=false；ANSI/OSC 转义清洗（正文保留、
// 控制字符不残留）。
#[tokio::test]
async fn execution_output_matrix() {
    let token = CancellationToken::new();
    let ws = TestWorkspace::new();

    // 普通命令。
    std::fs::write(ws.project_file("a.txt"), "hello").unwrap();
    let outcome = done(&ws, "c1", &args("cat a.txt"), &token).await;
    assert!(outcome.ok);
    let data = outcome.data.unwrap();
    assert_eq!(data["exitCode"], 0);
    assert!(data["output"].as_str().unwrap().contains("hello"));
    assert_eq!(data["timedOut"], false);

    // 合流 + 非零退出码（两个 shell 的等价写法：Unix sh / Windows powershell）。
    let command = if cfg!(windows) {
        "Write-Output OUT; [Console]::Error.WriteLine('ERR'); exit 3"
    } else {
        "echo OUT; echo ERR >&2; exit 3"
    };
    let outcome = done(&ws, "c2", &args(command), &token).await;
    assert!(!outcome.ok);
    let data = outcome.data.unwrap();
    assert_eq!(data["exitCode"], 3);
    let output = data["output"].as_str().unwrap();
    assert!(
        output.contains("OUT") && output.contains("ERR"),
        "合流：{output}"
    );

    // ANSI/OSC 清洗。
    let outcome = done(
        &ws,
        "c3",
        &args(r#"printf '\033[31m红\033[0m|\x1b]0;title\x07'"#),
        &token,
    )
    .await;
    let output = outcome.data.unwrap()["output"].as_str().unwrap().to_owned();
    assert!(output.contains("红"), "正文保留：{output:?}");
    assert!(!output.contains('\u{1b}'), "CSI/OSC 全清洗：{output:?}");
}

// 断言：治理矩阵——超时命令及时收割（E_COMMAND_TIMEOUT）且已收集输出保留、
// 取消令牌触发 Cancelled 且进程组收割、超 96KB 输出全量落盘（结果只带尾部
// 16KB 与落盘路径）、cwd 越界拒绝/roots 内放行、空命令与超范围 timeoutSec 拒绝。
#[tokio::test]
async fn timeout_cancel_spill_and_validation_matrix() {
    let token = CancellationToken::new();
    let ws = TestWorkspace::new();

    // 超时收割：1s 超时应及时（<5s），输出保留。
    let mut a = args("echo STARTED; sleep 30");
    a.timeout_sec = Some(1);
    let t = Instant::now();
    let outcome = done(&ws, "c4", &a, &token).await;
    assert!(!outcome.ok);
    let err = outcome.error.unwrap();
    assert_eq!(err.code, "E_COMMAND_TIMEOUT");
    let data = outcome.data.unwrap();
    assert_eq!(data["timedOut"], true);
    assert!(data["output"].as_str().unwrap().contains("STARTED"));
    assert!(t.elapsed() < Duration::from_secs(5), "1s 超时应及时收割");

    // 取消收割。
    let token2 = CancellationToken::new();
    let ctx = ws.ctx();
    let token_for_task = token2.clone();
    let task = tokio::spawn(async move {
        execute(&ctx, "c5", &args("echo A; sleep 30"), &token_for_task).await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    token2.cancel();
    assert!(matches!(task.await.unwrap(), CommandExit::Cancelled));

    // spill 治理：>96KB 全量落盘，结果只带尾部 16KB（约 120KB 产出的两个
    // shell 等价写法）。
    let command = if cfg!(windows) {
        "Write-Output ('x' * 120000)"
    } else {
        "yes 0123456789abcdefghijklmnopqrstuvwxyz | head -c 120000"
    };
    let outcome = done(&ws, "c6", &args(command), &token).await;
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

    // cwd 边界：越界拒绝，roots 内相对路径放行。
    let outside = tempfile::tempdir().unwrap();
    let mut a = args("pwd");
    a.cwd = Some(outside.path().display().to_string());
    let outcome = done(&ws, "c7", &a, &token).await;
    assert!(!outcome.ok);
    assert_eq!(outcome.error.unwrap().code, "E_PATH_OUTSIDE_ROOTS");
    let mut b = args("pwd");
    b.cwd = Some(".".to_owned());
    let outcome = done(&ws, "c8", &b, &token).await;
    assert!(outcome.ok, "cwd 在 roots 内应放行");

    // 参数校验：空命令与超范围 timeoutSec 均以 E_INVALID_ARG 拒绝。
    let outcome = done(&ws, "c9", &args("  "), &token).await;
    assert_eq!(outcome.error.unwrap().code, "E_INVALID_ARG");
    let mut a = args("true");
    a.timeout_sec = Some(MAX_TIMEOUT_SEC + 1);
    let outcome = done(&ws, "c10", &a, &token).await;
    assert_eq!(outcome.error.unwrap().code, "E_INVALID_ARG");
}
