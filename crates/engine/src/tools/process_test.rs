//! 进程组收割原语单测（macOS 路径）：enable + 收割真实子进程。
//! 整树收割（孙进程随组消亡）的实证由集成测试覆盖。

use std::process::Stdio;
use std::time::Duration;

use super::{REAP_GRACE, enable_process_group, reap_process_group};

// 断言：收割矩阵——SIGTERM 即终结的进程远小于宽限上界被收割；
// 忽略 SIGTERM 的进程耗满宽限期后由 SIGKILL 兜底终止。
#[tokio::test]
async fn reap_grace_and_sigkill_fallback_matrix() {
    // 普通 sleep 进程：SIGTERM 即退出，耗时远小于宽限上界。
    let mut cmd = tokio::process::Command::new("sleep");
    cmd.arg("30").stdin(Stdio::null());
    enable_process_group(&mut cmd);
    let mut child = cmd.spawn().expect("spawn");
    let started = std::time::Instant::now();
    let status = reap_process_group(&mut child, REAP_GRACE)
        .await
        .expect("收割应成功");
    let elapsed = started.elapsed();
    // SIGTERM 即终结：远小于宽限上界（5ms 轮询粒度 + 信号处理）
    assert!(!status.success(), "SIGTERM 终止的 sleep 非零退出");
    assert!(
        elapsed < Duration::from_millis(500),
        "收割耗时 {elapsed:?} 应远小于 500ms（实测 6–8ms 量级）"
    );

    // 忽略 SIGTERM 的 shell：耗满宽限期后 SIGKILL 兜底。
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg("trap '' TERM; echo ready; sleep 30")
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    enable_process_group(&mut cmd);
    let mut child = cmd.spawn().expect("spawn");
    // 等 shell 打出 ready（trap 已生效），确保 SIGTERM 路径确实被忽略
    let stdout = child.stdout.take().expect("piped");
    let mut line = String::new();
    tokio::io::AsyncBufReadExt::read_line(&mut tokio::io::BufReader::new(stdout), &mut line)
        .await
        .expect("read ready");
    let started = std::time::Instant::now();
    let status = reap_process_group(&mut child, REAP_GRACE)
        .await
        .expect("收割应成功");
    let elapsed = started.elapsed();
    assert!(!status.success(), "SIGKILL 兜底终止非零退出");
    // TERM 被忽略 → 必须耗满宽限期后才 SIGKILL
    assert!(elapsed >= REAP_GRACE, "宽限期应被用满（实际 {elapsed:?}）");
}
