//! 【S5 Spike】取消传播全链路集成测试：
//! `cancel_run`（置 CancellationToken）→ 模型流停止 / 命令进程组收割（macOS 实测）
//! → 收齐工具结果并补 E_CANCELLED → runEnded(cancelled)，全链时延实测 <1s。
//! Windows Job Object 分支只验可编译（cfg 隔离），语义确认见票 Answer。

mod common;

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::mock_openai::{
    MockOpenAI, content_delta, done_marker, finish_chunk, tool_args_delta, tool_open,
};
use common::{TOOL_COMMAND, TOOL_READ, TimedFrame, TimedSink, turn_driver, two_tool_driver};
use craftwave_lib::spike::cancel::{
    CancellableExecutor, REAP_GRACE, RunExit, ToolOutcome, enable_process_group, pid_alive,
    reap_process_group,
};
use craftwave_lib::spike::frames::{ChatFrame, EndReason, FrameSink};
use tokio::io::AsyncBufReadExt;
use tokio_util::sync::CancellationToken;

const FULL_CHAIN_BUDGET: Duration = Duration::from_millis(1000);

/// 从帧记录里按谓词计数。
fn count_kind(frames: &[TimedFrame], pred: impl Fn(&ChatFrame) -> bool) -> usize {
    frames.iter().filter(|t| pred(&t.frame.frame)).count()
}

fn is_text_delta(f: &ChatFrame) -> bool {
    matches!(f, ChatFrame::TextDelta { .. })
}

/// 场景 1（附录 A-S5 模型流停止）：取消打断流中的模型调用。
/// mock 以长流脚本模拟「答不完」的补全；取消后流 drop、runEnded(cancelled) 收尾。
#[tokio::test]
async fn s5_模型流中取消_流停止_run_ended_cancelled_全链时延() {
    // 长流：150 段 × 30ms ≈ 4.5s，测试在出帧 3 段后取消（~100ms 处）
    let mut script: Vec<(&'static str, String)> = Vec::new();
    for i in 0..150 {
        script.push(content_delta(&format!("第{i}段长输出。")));
    }
    script.push(finish_chunk("stop"));
    script.push(done_marker());
    let mock = MockOpenAI::start_with_turn1(script).await;

    let token = CancellationToken::new();
    let sink = TimedSink::default();

    let mut sink_for_run = sink.clone();
    let token_for_run = token.clone();
    let driver = turn_driver(&mock);
    let run_task = tokio::spawn(async move {
        let mut on_frame = |frame: ChatFrame| sink_for_run.send(frame);
        craftwave_lib::spike::cancel::drive_cancellable_run(
            &driver,
            common::new_run(),
            token_for_run,
            Arc::new(NoToolExecutor),
            &mut on_frame,
        )
        .await
    });

    // 等流真正跑起来（3 段 delta 出帧），再模拟前端 abort → invoke("cancel_run")
    // （§3.2 命令 19：置位 token 即返回 true）
    loop {
        let reached = { count_kind(&sink.0.lock().unwrap(), is_text_delta) >= 3 };
        if reached {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let t_cancel = Instant::now();
    token.cancel();

    let exit = run_task
        .await
        .expect("run 任务完成")
        .expect("无 PromptError");
    assert!(
        matches!(exit, RunExit::Cancelled),
        "期望取消退出态，实际 {exit:?}"
    );

    // runEnded(cancelled) 是最后一帧
    let frames = sink.0.lock().unwrap().clone();
    let last = frames.last().expect("有帧").frame.frame.clone();
    let ChatFrame::RunEnded { reason, .. } = &last else {
        panic!("最后一帧应为 runEnded，实际 {last:?}");
    };
    assert_eq!(*reason, EndReason::Cancelled, "终态 reason=cancelled");

    // 全链时延：cancel_run 置位 → runEnded(cancelled) 帧出槽 <1s
    let ended_at = frames.last().expect("有帧").at;
    let elapsed_ms = ended_at.duration_since(t_cancel).as_secs_f64() * 1000.0;
    println!("S5[模型流] 全链时延 cancel→runEnded(cancelled) = {elapsed_ms:.2}ms");
    assert!(
        ended_at.duration_since(t_cancel) < FULL_CHAIN_BUDGET,
        "全链时延 {elapsed_ms:.1}ms 超出 <1s 预算"
    );

    // 流确实停止：runEnded 之后 300ms 无任何新帧（流未停会继续出 delta）
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = sink.0.lock().unwrap().len();
    assert_eq!(after, frames.len(), "runEnded 后流已停止，无新帧");
}

/// 场景 2（附录 A-S5 进程组收割 + 标记语义）：工具批 [read 快工具, command 慢命令]，
/// 取消打断 command 的进程组（sh + 孙 sleep 整树，macOS 实测），read 结果原样保留
/// （不回滚），command 结果补 E_CANCELLED；收齐后才发 runEnded(cancelled)。
#[cfg(unix)]
#[tokio::test]
async fn s5_命令工具取消_进程组整树收割_cancelled标记收齐() {
    // 双工具脚本：call_1=read（立即完成），call_2=command（sleep 900 长命令）
    let mock = MockOpenAI::start_with_turn1(vec![
        tool_open(0, "call_1", TOOL_READ),
        tool_args_delta(0, "{}"),
        tool_open(1, "call_2", TOOL_COMMAND),
        tool_args_delta(1, "{\"command\": \"sleep 900\"}"),
        finish_chunk("tool_calls"),
        done_marker(),
    ])
    .await;

    let token = CancellationToken::new();
    let sink = TimedSink::default();
    let (spawned_tx, mut spawned_rx) = tokio::sync::mpsc::unbounded_channel::<(String, i32)>();

    let mut sink_for_run = sink.clone();
    let token_for_run = token.clone();
    let driver = two_tool_driver(&mock);
    let run_task = tokio::spawn(async move {
        let mut on_frame = |frame: ChatFrame| sink_for_run.send(frame);
        craftwave_lib::spike::cancel::drive_cancellable_run(
            &driver,
            common::new_run(),
            token_for_run,
            Arc::new(CommandTreeExecutor {
                spawned: spawned_tx,
            }),
            &mut on_frame,
        )
        .await
    });

    // 等命令进程树就绪（孙 pid 广播：此刻 read 已完成、command 执行中）再取消
    let (_call_id, grandchild_pid) = spawned_rx.recv().await.expect("spawn 通知");
    let t_cancel = Instant::now();
    token.cancel();

    let exit = run_task
        .await
        .expect("run 任务完成")
        .expect("无 PromptError");
    assert!(
        matches!(exit, RunExit::Cancelled),
        "期望取消退出态，实际 {exit:?}"
    );

    // —— 帧序列：read 结果保留 → command 补 E_CANCELLED → runEnded(cancelled) 收齐 ——
    let frames = sink.0.lock().unwrap().clone();
    let tool_results: Vec<&ChatFrame> = frames
        .iter()
        .map(|t| &t.frame.frame)
        .filter(|f| matches!(f, ChatFrame::ToolResult { .. }))
        .collect();
    assert_eq!(
        tool_results.len(),
        2,
        "批内两个工具结果都收齐（不丢不悬空）"
    );
    // read：已完成的结果原样保留（ok=true、无 error 面，已写文件不回滚语义）
    let ChatFrame::ToolResult {
        call_id: call_1,
        ok: ok_1,
        error: error_1,
        ..
    } = tool_results[0]
    else {
        unreachable!()
    };
    assert_eq!(call_1, "call_1");
    assert!(ok_1, "已完成工具结果保留原样");
    assert!(error_1.is_none(), "已完成结果不挂取消标记");

    // command：补 E_CANCELLED（ok=false + error.code）
    let ChatFrame::ToolResult {
        call_id: call_2,
        ok: ok_2,
        ui_summary: summary_2,
        error: error_2,
    } = tool_results[1]
    else {
        unreachable!()
    };
    assert_eq!(call_2, "call_2");
    assert!(!ok_2, "被收割命令 ok=false");
    let err = error_2.as_ref().expect("被收割命令带 error 面");
    assert_eq!(err.code, craftwave_lib::spike::cancel::E_CANCELLED);
    assert!(
        summary_2.contains("收割"),
        "ui_summary 注明收割：{summary_2}"
    );

    // 终态帧在全部工具结果之后，reason=cancelled
    let last = frames.last().expect("有帧").frame.frame.clone();
    let ChatFrame::RunEnded { reason, .. } = &last else {
        panic!("最后一帧应为 runEnded，实际 {last:?}");
    };
    assert_eq!(*reason, EndReason::Cancelled);
    let last_result_idx = frames
        .iter()
        .rposition(|t| matches!(t.frame.frame, ChatFrame::ToolResult { .. }))
        .expect("有 toolResult");
    assert!(
        last_result_idx < frames.len() - 1,
        "runEnded 是工具收齐后的最后一帧"
    );

    // 全链时延：cancel_run 置位 → runEnded(cancelled) 帧出槽 <1s（含进程组收割）
    let ended_at = frames.last().expect("有帧").at;
    let elapsed_ms = ended_at.duration_since(t_cancel).as_secs_f64() * 1000.0;
    println!(
        "S5[进程组收割] 全链时延 cancel→runEnded(cancelled) = {elapsed_ms:.2}ms（含 SIGTERM+{:?} 宽限）",
        REAP_GRACE
    );
    assert!(
        ended_at.duration_since(t_cancel) < FULL_CHAIN_BUDGET,
        "全链时延（含收割）{elapsed_ms:.1}ms 超出 <1s 预算"
    );

    // 整树收割实证：孙进程（sleep 900）在收割后消亡
    let deadline = Instant::now() + Duration::from_secs(2);
    while pid_alive(grandchild_pid) {
        assert!(
            Instant::now() < deadline,
            "孙进程 {grandchild_pid} 收割后 2s 内未消亡（整树收割失效）"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    println!("S5[进程组收割] 孙进程 {grandchild_pid}（sleep 900）已随组消亡");
}

/// 场景 2 的执行器：read 立即完成；command spawn 进程组进程树并感知取消。
struct CommandTreeExecutor {
    spawned: tokio::sync::mpsc::UnboundedSender<(String, i32)>,
}

impl CancellableExecutor for CommandTreeExecutor {
    fn execute<'a>(
        &'a self,
        call_id: &'a str,
        name: &'a str,
        _args_json: &'a str,
        token: &'a CancellationToken,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolOutcome> + Send + 'a>> {
        match name {
            TOOL_READ => Box::pin(async {
                ToolOutcome::Completed("[read] 共 12 行：port = 8087".to_owned())
            }),
            TOOL_COMMAND => Box::pin(async move {
                let mut tree = spawn_command_tree().await;
                let _ = self.spawned.send((call_id.to_owned(), tree.grandchild_pid));
                let outcome = tokio::select! {
                    biased;
                    _ = token.cancelled() => None,
                    status = tree.child.wait() => Some(status),
                };
                match outcome {
                    // 取消收割整组：SIGTERM → 宽限 → SIGKILL（sh + 孙 sleep 一并）
                    None => {
                        let status = reap_process_group(&mut tree.child, REAP_GRACE)
                            .await
                            .expect("进程组收割应成功");
                        ToolOutcome::Cancelled(format!(
                            "取消：进程组已收割（sh 退出 {status}，孙进程随组终止）"
                        ))
                    }
                    Some(status) => {
                        ToolOutcome::Completed(format!("命令意外完成（未取消）: {status:?}"))
                    }
                }
            }),
            other => panic!("未预期的工具调用 {other}"),
        }
    }
}

/// 真实进程树（macOS 实测用）：sh 在独立进程组里 fork 一个孙 sleep。
/// `sh -c 'sleep 900 & echo CHILD_PID=$!; wait'` —— echo 的 $! 即孙 pid。
#[cfg(unix)]
struct CommandTree {
    child: tokio::process::Child,
    grandchild_pid: i32,
}

#[cfg(unix)]
async fn spawn_command_tree() -> CommandTree {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg("sleep 900 & echo CHILD_PID=$!; wait")
        .stdin(Stdio::null())
        .stdout(Stdio::piped());
    enable_process_group(&mut cmd);
    let mut child = cmd.spawn().expect("spawn 进程组进程树");
    let stdout = child.stdout.take().expect("stdout 已 piped");
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let line = lines
        .next_line()
        .await
        .expect("读孙 pid 行")
        .expect("有 CHILD_PID 行");
    let grandchild_pid: i32 = line
        .strip_prefix("CHILD_PID=")
        .unwrap_or_else(|| panic!("CHILD_PID 行格式异常: {line}"))
        .parse()
        .expect("孙 pid 数字");
    CommandTree {
        child,
        grandchild_pid,
    }
}

/// 场景 1 的执行器：长流脚本无工具调用；若被调用即脚本错误。
struct NoToolExecutor;

impl CancellableExecutor for NoToolExecutor {
    fn execute<'a>(
        &'a self,
        call_id: &'a str,
        name: &'a str,
        _args_json: &'a str,
        _token: &'a CancellationToken,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolOutcome> + Send + 'a>> {
        Box::pin(async move {
            panic!("长流脚本不应有工具调用（call_id={call_id}, name={name}）");
        })
    }
}
