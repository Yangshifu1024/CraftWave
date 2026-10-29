//! backend 产品化链路端到端（mock OpenAI 兼容 SSE，离线确定性）：
//!
//! 1. 帧保真与 callId 同源：文本/思考/工具参数流 → RunLoop 生产路径（64ms
//!    节流内聚）→ 帧序列（runStarted/stepStarted/delta/toolStarted/toolArgs/
//!    toolResult/usage/runEnded），帧面三帧 callId 同源、seq 严格递增、
//!    usage 累计、NFR-02（TTFT <100ms）与节流间隔断言；
//! 2. 取消全链：流中取消（<1s，runEnded(cancelled) 收齐后最后一帧）；
//! 3. 取消收齐语义：工具批内已完成保留、未完成补 E_CANCELLED；
//! 4. 审批挂起机制层：gate=Suspend 序列化挂起 → resume_run → 跑到 Done
//!    （14 号票增量：批级预判零执行、DriveState 延续、callId 跨挂起同源）；
//! 5. start_chat_core 命令面：模型解析（config+keyring 注入）、同会话互斥
//!    E_SESSION_RUNNING、RunManager 取消；
//! 6. 审批门接线（14 号票）：confirm_each 挂起 → ask:opened → resolve(allow)
//!    → 恢复 Done；resolve(deny) → 拒绝终态；围栏黑名单 Block 回喂不中断；
//! 7. plan/ask/suggest（14 号票）：plan:update 事件 + approve_plan、
//!    ask 问卷 resolve 回喂、suggest 后台小调用 run:suggestions + tokens:update。

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::mock_openai::{
    MockOpenAI, TURN1_REASONING, TURN1_TEXT, TURN1_TOOL_ARG_PARTS, TURN2_TEXT, content_delta,
    done_marker, finish_chunk, tool_args_delta, tool_open, usage_event,
};
use craftwave_lib::agent::RunManager;
use craftwave_lib::backend::frames::{
    ChatFrame, EndReason, FrameSink, RunSnapshotStatus, SequencedFrame, SequencedSink,
};
use craftwave_lib::backend::run_loop::{
    AllowAllGate, CancellableExecutor, GateVerdict, RunExit, ToolExecution, ToolGate, drive_run,
    resume_run,
};
use craftwave_lib::backend::turn::TurnDriver;
use craftwave_lib::host::AppState;
use rig::completion::Message;
use rig::run::AgentRun;
use rig::run::spec::RunSpec;
use tokio_util::sync::CancellationToken;

const TOOL_READ: &str = "read";
const TOOL_COMMAND: &str = "command";
const FULL_CHAIN_BUDGET: Duration = Duration::from_millis(1000);

// —— 测试装配（与 spike common 解耦，产品化链路自包含） ——

/// 指向 mock 的 rig Chat 模型。
fn mock_model(mock: &MockOpenAI) -> rig::driver::Model<rig::providers::openai::wire::Chat> {
    let config = rig::providers::openai::wire::OpenAIConfig::new("mock-key".to_owned())
        .with_base_url(mock.base_url.clone());
    config.client().chat("gpt-4o-mock")
}

/// 工具定义（read：mock 脚本点名；command：S5 长命令载体）。
fn tool_def(name: &str) -> Arc<rig::completion::request::ToolDefinition> {
    Arc::new(rig::completion::request::ToolDefinition::new(
        rig::message::ToolName::new(name.to_owned()).expect("tool name"),
        format!("{name} 测试工具"),
        serde_json::json!({"type": "object", "properties": {}}),
    ))
}

/// 带到达时刻的帧记录（时延断言用）。
#[derive(Debug, Clone)]
struct TimedFrame {
    at: Instant,
    frame: SequencedFrame,
}

/// 共享收集 sink。双实现：
/// - `FrameSink`：RunLoop 直驱路径（`drive_run`）——出口填 seq（本实现按
///   到达序自填）；
/// - `SequencedSink`：`start_chat_core` 命令面路径——seq 由 `FrameHub`
///   统一递增后下发（§5.8，05 号票），本实现原样收帧。
#[derive(Default, Clone)]
struct SharedSink(Arc<Mutex<Vec<TimedFrame>>>);

impl FrameSink for SharedSink {
    fn send(&mut self, frame: ChatFrame) {
        let mut frames = self.0.lock().unwrap();
        let seq = frames.len() as u64;
        frames.push(TimedFrame {
            at: Instant::now(),
            frame: SequencedFrame {
                seq,
                ts: None,
                frame,
            },
        });
    }
}

impl SequencedSink for SharedSink {
    fn send_seq(&mut self, frame: SequencedFrame) {
        self.0.lock().unwrap().push(TimedFrame {
            at: Instant::now(),
            frame,
        });
    }
}

impl SharedSink {
    fn frames(&self) -> Vec<TimedFrame> {
        self.0.lock().unwrap().clone()
    }

    fn count_kind(&self, pred: impl Fn(&ChatFrame) -> bool) -> usize {
        self.frames()
            .iter()
            .filter(|t| pred(&t.frame.frame))
            .count()
    }
}

fn is_text(f: &ChatFrame) -> bool {
    matches!(f, ChatFrame::TextDelta { .. })
}

fn join_frames(frames: &[TimedFrame], pred: impl Fn(&ChatFrame) -> bool) -> String {
    frames
        .iter()
        .filter(|t| pred(&t.frame.frame))
        .map(|t| match &t.frame.frame {
            ChatFrame::TextDelta { delta, .. }
            | ChatFrame::ReasoningDelta { delta, .. }
            | ChatFrame::ToolArgs { chunk: delta, .. } => delta.clone(),
            _ => String::new(),
        })
        .collect()
}

/// 假执行器：一律返回固定文本的 Completed。
struct FakeExecutor(String);

impl CancellableExecutor for FakeExecutor {
    fn execute<'a>(
        &'a self,
        _call_id: &'a str,
        _name: &'a str,
        _args_json: &'a str,
        _token: &'a CancellationToken,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
        let output = self.0.clone();
        Box::pin(async move {
            ToolExecution::Completed {
                ok: true,
                ui_summary: format!("[read] {output}"),
                error: None,
                model_text: format!("[read] {output}"),
            }
        })
    }
}

fn fake_executor_text(output: &str) -> Arc<dyn CancellableExecutor> {
    Arc::new(FakeExecutor(output.to_owned()))
}

fn new_run(prompt: &str) -> AgentRun {
    AgentRun::new(Message::user(prompt)).max_turns(4)
}

// —— 1. 帧保真 + callId 同源 + 节流 + NFR-02 ——

#[tokio::test]
async fn e2e_frame_fidelity_call_id_same_origin_throttle_nfr02() {
    let mock = MockOpenAI::start().await; // chunk 间隔 30ms
    let driver = TurnDriver::new(mock_model(&mock), RunSpec::new(), vec![tool_def(TOOL_READ)]);
    let sink = SharedSink::default();
    let mut sink_handle = sink.clone();
    let exit = drive_run(
        &driver,
        new_run("帮我读配置文件"),
        "run-e2e-1",
        "gpt-4o-mock",
        CancellationToken::new(),
        fake_executor_text("共 12 行：port = 8087"),
        Arc::new(AllowAllGate),
        &mut sink_handle,
        None,
        None,
    )
    .await
    .expect("RunLoop 应正常跑完");

    let frames = sink.frames();
    assert!(
        matches!(exit, RunExit::Done(_)),
        "退出态应为 Done，实际 {exit:?}"
    );

    // —— runStarted 起手帧 ——
    let first = &frames[0].frame.frame;
    let ChatFrame::RunStarted { run_id, model } = first else {
        panic!("首帧应为 runStarted，实际 {first:?}");
    };
    assert_eq!(run_id, "run-e2e-1");
    assert_eq!(model, "gpt-4o-mock");

    // —— stepStarted 两拍（两个模型调用） ——
    let turns: Vec<u32> = frames
        .iter()
        .filter_map(|t| match &t.frame.frame {
            ChatFrame::StepStarted { turn } => Some(*turn),
            _ => None,
        })
        .collect();
    assert_eq!(turns, vec![1, 2], "两次模型调用各一拍 stepStarted");

    // —— 保真：拼接与 mock 脚本一致（无丢失；节流合并无损） ——
    let text = join_frames(&frames, is_text);
    let reasoning = join_frames(&frames, |f| matches!(f, ChatFrame::ReasoningDelta { .. }));
    let args = join_frames(&frames, |f| matches!(f, ChatFrame::ToolArgs { .. }));
    assert_eq!(text, format!("{TURN1_TEXT}{TURN2_TEXT}"));
    assert_eq!(reasoning, TURN1_REASONING);
    assert_eq!(args, TURN1_TOOL_ARG_PARTS.concat());

    // —— callId 同源（§3.4）：toolStarted/toolArgs/toolResult 帧面同一 id ——
    let started_ids: Vec<String> = frames
        .iter()
        .filter_map(|t| match &t.frame.frame {
            ChatFrame::ToolStarted { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    let args_ids: Vec<String> = frames
        .iter()
        .filter_map(|t| match &t.frame.frame {
            ChatFrame::ToolArgs { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    let result_ids: Vec<String> = frames
        .iter()
        .filter_map(|t| match &t.frame.frame {
            ChatFrame::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    assert!(!started_ids.is_empty(), "应有 toolStarted 帧");
    // 同源 = 帧面 callId 集合一致（节流会合并多条 toolArgs，帧数不必相等）
    let started_set: std::collections::BTreeSet<_> = started_ids.into_iter().collect();
    let args_set: std::collections::BTreeSet<_> = args_ids.into_iter().collect();
    let result_set: std::collections::BTreeSet<_> = result_ids.into_iter().collect();
    assert_eq!(started_set, args_set, "toolArgs 与 toolStarted 同源");
    assert_eq!(started_set, result_set, "toolResult 与流式帧同源");

    // —— seq 严格递增；runEnded(done) 收尾 ——
    for (i, t) in frames.iter().enumerate() {
        assert_eq!(t.frame.seq, i as u64);
    }
    let Some(last) = frames.last() else {
        panic!("有帧")
    };
    let ChatFrame::RunEnded {
        reason,
        tool_count,
        turn,
        ..
    } = &last.frame.frame
    else {
        panic!("末帧应为 runEnded");
    };
    assert_eq!(*reason, EndReason::Done);
    assert_eq!(*tool_count, 1);
    assert_eq!(*turn, 2);

    // —— usage 帧两轮累计（mock 每轮 120 in / 48 out） ——
    let usages: Vec<(u64, u64)> = frames
        .iter()
        .filter_map(|t| match &t.frame.frame {
            ChatFrame::Usage { usage, .. } => Some((usage.input, usage.output)),
            _ => None,
        })
        .collect();
    assert_eq!(
        usages,
        vec![(120, 48), (240, 96)],
        "usage 帧为运行累计口径（§10.5）"
    );

    // —— NFR-02：TTFT（mock 首 chunk → 首个 delta 帧出槽）<100ms ——
    let sent = mock.sent_chunks().await;
    let first_sent = sent.first().expect("mock 有发送记录").at;
    let first_delta = frames
        .iter()
        .find(|t| {
            is_text(&t.frame.frame) || matches!(t.frame.frame, ChatFrame::ReasoningDelta { .. })
        })
        .expect("有增量帧");
    let ttft_ms = first_delta.at.duration_since(first_sent).as_secs_f64() * 1000.0;
    println!("E2E TTFT = {ttft_ms:.2}ms");
    assert!(ttft_ms < 100.0, "TTFT {ttft_ms:.1}ms 超出 NFR-02（<100ms）");

    // —— 节流：相邻同型增量帧间隔 ≥64ms（55ms 容忍；上限放宽到轮次往返） ——
    let text_frames: Vec<&TimedFrame> = frames.iter().filter(|t| is_text(&t.frame.frame)).collect();
    let gaps: Vec<f64> = text_frames
        .windows(2)
        .map(|w| (w[1].at - w[0].at).as_secs_f64() * 1000.0)
        .collect();
    println!("E2E 文本帧间隔（ms）：{gaps:?}");
    for gap in &gaps {
        assert!(
            (55.0..=400.0).contains(gap),
            "帧间隔 {gap}ms 异常：低于 64ms 窗口即节流失效"
        );
    }
}

// —— 2. 流中取消 ——

#[tokio::test]
async fn e2e_cancel_midstream_run_ended_last_frame_latency() {
    // 长流：150 段 × 30ms ≈ 4.5s，出帧 3 段后取消
    let mut script: Vec<(&'static str, String)> = Vec::new();
    for i in 0..150 {
        script.push(content_delta(&format!("第{i}段长输出。")));
    }
    script.push(finish_chunk("stop"));
    script.push(usage_event());
    script.push(done_marker());
    let mock = MockOpenAI::start_with_turn1(script).await;
    let driver = TurnDriver::new(mock_model(&mock), RunSpec::new(), vec![tool_def(TOOL_READ)]);
    let sink = SharedSink::default();
    let token = CancellationToken::new();

    let sink_for_run = sink.clone();
    let token_for_run = token.clone();
    let task = tokio::spawn(async move {
        let mut sink_box: Box<dyn FrameSink> = Box::new(sink_for_run);
        drive_run(
            &driver,
            new_run("答不完的问题"),
            "run-cancel",
            "gpt-4o-mock",
            token_for_run,
            fake_executor_text("不该被调用"),
            Arc::new(AllowAllGate),
            &mut *sink_box,
            None,
            None,
        )
        .await
    });

    while sink.count_kind(is_text) < 3 {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let t_cancel = Instant::now();
    token.cancel();

    let exit = task.await.expect("run 任务完成").expect("无 PromptError");
    assert!(matches!(exit, RunExit::Cancelled), "取消退出态：{exit:?}");

    let frames = sink.frames();
    let last = frames.last().expect("有帧");
    let ChatFrame::RunEnded { reason, .. } = &last.frame.frame else {
        panic!("末帧应为 runEnded，实际 {:?}", last.frame.frame);
    };
    assert_eq!(*reason, EndReason::Cancelled);
    let elapsed = last.at.duration_since(t_cancel);
    println!(
        "E2E[流中取消] cancel→runEnded(cancelled) = {:.2}ms",
        elapsed.as_secs_f64() * 1000.0
    );
    assert!(elapsed < FULL_CHAIN_BUDGET, "全链 <1s 预算");

    // 流确实停止：300ms 内无新帧
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(sink.frames().len(), frames.len(), "runEnded 后无新帧");
}

// —— 3. 工具批取消收齐语义 ——

/// 双工具执行器：read 立即完成；command spawn 真实进程树、select token 收割。
/// 进程树与收割按平台分派：Unix `sh` + 进程组（SIGTERM→宽限→SIGKILL）、
/// Windows PowerShell + Job Object（TerminateJobObject 整树）——同一 e2e
/// 在三平台 CI 各自真实执行取消传播（19 号票：Windows 不再跳过）。
struct ReadThenCommandExecutor {
    spawned: tokio::sync::mpsc::UnboundedSender<u64>,
}

impl CancellableExecutor for ReadThenCommandExecutor {
    fn execute<'a>(
        &'a self,
        call_id: &'a str,
        name: &'a str,
        _args_json: &'a str,
        token: &'a CancellationToken,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolExecution> + Send + 'a>> {
        match name {
            TOOL_READ => Box::pin(async {
                ToolExecution::Completed {
                    ok: true,
                    ui_summary: "[read] 完成".to_owned(),
                    error: None,
                    model_text: "[read] 完成".to_owned(),
                }
            }),
            TOOL_COMMAND => Box::pin(async move {
                let mut tree = spawn_command_tree().await;
                let _ = self.spawned.send(tree.grandchild_pid as u64);
                let _ = call_id;
                let outcome = tokio::select! {
                    biased;
                    _ = token.cancelled() => None,
                    status = tree.child.wait() => Some(status.expect("wait")),
                };
                match outcome {
                    None => {
                        #[cfg(unix)]
                        let status = craftwave_lib::tools::process::reap_process_group(
                            &mut tree.child,
                            craftwave_lib::tools::process::REAP_GRACE,
                        )
                        .await
                        .expect("进程组收割");
                        #[cfg(windows)]
                        let status = tree
                            .job
                            .reap(&mut tree.child, craftwave_lib::tools::process::REAP_GRACE)
                            .await
                            .expect("Job Object 收割");
                        ToolExecution::Cancelled {
                            executed: true,
                            message: format!(
                                "进程树已收割（shell 退出 {status:?}，孙进程随组/随 Job 终止）"
                            ),
                        }
                    }
                    Some(status) => ToolExecution::Completed {
                        ok: true,
                        ui_summary: format!("命令意外完成：{status:?}"),
                        error: None,
                        model_text: String::new(),
                    },
                }
            }),
            other => panic!("未预期工具 {other}"),
        }
    }
}

/// 真实进程树载体：直接子进程（shell）+ 已知 pid 的孙进程（sleep 载体）。
/// 孙 pid 经 stdout 广播给测试侧，收割后据此断言「整树消亡」。
#[cfg(unix)]
struct CommandTree {
    child: tokio::process::Child,
    grandchild_pid: i32,
}

/// Unix 真实进程树：sh 在独立进程组里 fork 一个孙 sleep。
/// `sh -c 'sleep 900 & echo CHILD_PID=$!; wait'` —— echo 的 $! 即孙 pid。
#[cfg(unix)]
async fn spawn_command_tree() -> CommandTree {
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg("sleep 900 & echo CHILD_PID=$!; wait")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped());
    craftwave_lib::tools::process::enable_process_group(&mut cmd);
    let mut child = cmd.spawn().expect("spawn 进程树");
    let stdout = child.stdout.take().expect("piped");
    let mut lines = tokio::io::BufReader::new(stdout);
    use tokio::io::AsyncBufReadExt;
    let mut line = String::new();
    lines.read_line(&mut line).await.expect("读孙 pid");
    let grandchild: i32 = line
        .trim()
        .strip_prefix("CHILD_PID=")
        .unwrap_or("")
        .parse()
        .unwrap_or(0);
    CommandTree {
        child,
        grandchild_pid: grandchild,
    }
}

/// Windows 真实进程树：PowerShell 起一个孙 ping 并等待它，孙 pid 经
/// stdout 广播。spawn 后立即 assign 进 Job（后代自动入组），取消时
/// TerminateJobObject 整树收割（spawn→assign 逃逸窗口此处为测试语义，
/// 生产路径同款时序，见 tools/command.rs）。
#[cfg(windows)]
struct CommandTree {
    child: tokio::process::Child,
    job: craftwave_lib::tools::process::JobGuard,
    grandchild_pid: u32,
}

#[cfg(windows)]
async fn spawn_command_tree() -> CommandTree {
    let mut cmd = tokio::process::Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-Command",
        "$p = Start-Process -FilePath ping -ArgumentList @('-n','900','127.0.0.1') -PassThru -WindowStyle Hidden; Write-Output $p.Id; Wait-Process -Id $p.Id",
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::piped());
    let mut child = cmd.spawn().expect("spawn 进程树");
    let job = craftwave_lib::tools::process::JobGuard::new().expect("创建 Job Object");
    job.assign(&child).expect("指派进 Job");
    let stdout = child.stdout.take().expect("piped");
    let mut lines = tokio::io::BufReader::new(stdout);
    use tokio::io::AsyncBufReadExt;
    let mut line = String::new();
    lines.read_line(&mut line).await.expect("读孙 pid");
    let grandchild: u32 = line
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("孙 pid 数字，实际行：{line}"));
    CommandTree {
        child,
        job,
        grandchild_pid: grandchild,
    }
}

#[tokio::test]
async fn e2e_tool_batch_cancel_keep_completed_mark_rest() {
    let mock = MockOpenAI::start_with_turn1(vec![
        tool_open(0, "call_1", TOOL_READ),
        tool_args_delta(0, "{}"),
        tool_open(1, "call_2", TOOL_COMMAND),
        tool_args_delta(1, "{}"),
        finish_chunk("tool_calls"),
        done_marker(),
    ])
    .await;
    let driver = TurnDriver::new(
        mock_model(&mock),
        RunSpec::new(),
        vec![tool_def(TOOL_READ), tool_def(TOOL_COMMAND)],
    );
    let sink = SharedSink::default();
    let token = CancellationToken::new();
    let (spawned_tx, mut spawned_rx) = tokio::sync::mpsc::unbounded_channel::<u64>();

    let sink_for_run = sink.clone();
    let token_for_run = token.clone();
    let task = tokio::spawn(async move {
        let mut sink_box: Box<dyn FrameSink> = Box::new(sink_for_run);
        drive_run(
            &driver,
            new_run("读文件再跑命令"),
            "run-tool-cancel",
            "gpt-4o-mock",
            token_for_run,
            Arc::new(ReadThenCommandExecutor {
                spawned: spawned_tx,
            }),
            Arc::new(AllowAllGate),
            &mut *sink_box,
            None,
            None,
        )
        .await
    });

    let grandchild = spawned_rx.recv().await.expect("spawn 通知");
    let t_cancel = Instant::now();
    token.cancel();

    let exit = task.await.expect("run 任务完成").expect("无 PromptError");
    assert!(matches!(exit, RunExit::Cancelled));

    // —— 帧序列：read 保留 → command 补标记 → runEnded(cancelled) 收齐 ——
    let frames = sink.frames();
    let results: Vec<&ChatFrame> = frames
        .iter()
        .map(|t| &t.frame.frame)
        .filter(|f| matches!(f, ChatFrame::ToolResult { .. }))
        .collect();
    assert_eq!(results.len(), 2, "批内两个工具结果收齐");
    let ChatFrame::ToolResult {
        call_id: id1,
        ok: ok1,
        error: err1,
        ..
    } = results[0]
    else {
        unreachable!()
    };
    assert_eq!(
        id1, "call_part_1_0",
        "帧面 callId = 桥接派生 id（与流式帧同源，turn 前缀）"
    );
    assert!(ok1, "已完成结果原样保留（不回滚）");
    assert!(err1.is_none());
    let ChatFrame::ToolResult {
        call_id: id2,
        ok: ok2,
        ui_summary: s2,
        error: err2,
    } = results[1]
    else {
        unreachable!()
    };
    assert_eq!(
        id2, "call_part_1_1",
        "帧面 callId = 桥接派生 id（与流式帧同源，turn 前缀）"
    );
    assert!(!ok2, "被收割命令 ok=false");
    assert_eq!(
        err2.as_ref().expect("E_CANCELLED 错误面").code,
        "E_CANCELLED"
    );
    assert!(s2.contains("收割"), "uiSummary 注明收割：{s2}");
    let last = frames.last().expect("有帧");
    let ChatFrame::RunEnded { reason, .. } = &last.frame.frame else {
        panic!("末帧 runEnded");
    };
    assert_eq!(*reason, EndReason::Cancelled);
    assert!(
        last.at.duration_since(t_cancel) < FULL_CHAIN_BUDGET,
        "全链 <1s"
    );

    // 整树收割：孙进程消亡（Unix kill(pid,0) / Windows OpenProcess 探测）
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        #[cfg(unix)]
        let alive = craftwave_lib::tools::process::pid_alive(grandchild as i32);
        #[cfg(windows)]
        let alive = craftwave_lib::tools::process::pid_alive(grandchild as u32);
        if !alive {
            break;
        }
        assert!(Instant::now() < deadline, "孙进程 {grandchild} 2s 内未消亡");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// —— 4. 审批挂起机制层 ——

/// 首个工具调用判 Suspend、其后放行的门（挂起/恢复双向驱动用）。
struct SuspendOnceGate {
    fired: Mutex<bool>,
}

impl ToolGate for SuspendOnceGate {
    fn judge(&self, _name: &str, _args: &str) -> GateVerdict {
        let mut fired = self.fired.lock().unwrap();
        if *fired {
            GateVerdict::Allow
        } else {
            *fired = true;
            GateVerdict::Suspend { fence_reason: None }
        }
    }
}

#[tokio::test]
async fn e2e_approval_suspend_serialize_resume_done() {
    let mock = MockOpenAI::start().await;
    let driver = TurnDriver::new(mock_model(&mock), RunSpec::new(), vec![tool_def(TOOL_READ)]);

    // 第一段：gate 判 Suspend → 整只 run 序列化挂起
    let sink1 = SharedSink::default();
    let mut sink1_handle = sink1.clone();
    let exit = drive_run(
        &driver,
        new_run("帮我读配置文件"),
        "run-suspend",
        "gpt-4o-mock",
        CancellationToken::new(),
        fake_executor_text("共 12 行：port = 8087"),
        Arc::new(SuspendOnceGate {
            fired: Mutex::new(false),
        }),
        &mut sink1_handle,
        None,
        None,
    )
    .await
    .expect("挂起路径应正常返回");
    let RunExit::Suspended(info) = exit else {
        panic!("应为 Suspended，实际 {exit:?}");
    };
    let craftwave_lib::backend::run_loop::SuspendedInfo {
        run_json,
        pending,
        state,
    } = *info;
    assert_eq!(pending.name, TOOL_READ, "挂起点携带调用明细");
    // toolResult 未发（挂起在执行前）、runEnded 未发
    assert_eq!(
        sink1.count_kind(|f| matches!(f, ChatFrame::ToolResult { .. })),
        0
    );
    assert_eq!(
        sink1.count_kind(|f| matches!(f, ChatFrame::RunEnded { .. })),
        0
    );
    // 挂起前已有流式 toolStarted：恢复后 toolResult 须与其同源 callId
    let suspended_started_ids: Vec<String> = sink1
        .frames()
        .iter()
        .filter_map(|t| match &t.frame.frame {
            ChatFrame::ToolStarted { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(suspended_started_ids.len(), 1, "挂起前已有流式 toolStarted");
    assert_eq!(state.tool_count, 0, "挂起时批内零执行（工具计数为 0）");

    // 第二段：反序列化恢复（DriveState 延续：不重发 runStarted、callId 映射同源）
    let resumed = resume_run(&run_json).expect("反序列化恢复");
    let sink2 = SharedSink::default();
    let mut sink2_handle = sink2.clone();
    let exit = drive_run(
        &driver,
        resumed,
        "run-suspend",
        "gpt-4o-mock",
        CancellationToken::new(),
        fake_executor_text("共 12 行：port = 8087"),
        Arc::new(AllowAllGate),
        &mut sink2_handle,
        Some(state),
        None,
    )
    .await
    .expect("恢复路径应跑到完成");
    assert!(
        matches!(exit, RunExit::Done(_)),
        "恢复后跑到 Done：{exit:?}"
    );
    assert_eq!(
        sink2.count_kind(|f| matches!(f, ChatFrame::RunStarted { .. })),
        0,
        "恢复不重发 runStarted"
    );
    assert_eq!(
        sink2.count_kind(|f| matches!(f, ChatFrame::ToolResult { .. })),
        1
    );
    // callId 同源跨挂起/恢复：toolResult = 挂起前 toolStarted 的同一桥接 id
    let resumed_result_ids: Vec<String> = sink2
        .frames()
        .iter()
        .filter_map(|t| match &t.frame.frame {
            ChatFrame::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(resumed_result_ids, suspended_started_ids);
    let frames2 = sink2.frames();
    let last = frames2.last().expect("有帧");
    let ChatFrame::RunEnded { reason, .. } = &last.frame.frame else {
        panic!("恢复后末帧 runEnded");
    };
    assert_eq!(*reason, EndReason::Done);
}

// —— 5. start_chat_core 命令面 ——

mod command_face {
    use super::*;

    use craftwave_lib::providers::types::{ApiFormat, ProviderConfig, ProviderModel};

    fn provider_fixture(base_url: &str) -> ProviderConfig {
        ProviderConfig {
            id: "prov-1".to_owned(),
            name: "Mock 供应商".to_owned(),
            api_format: ApiFormat::OpenaiChat,
            base_url: Some(base_url.to_owned()),
            api_key: Some("mock-key".into()),
            models: vec![ProviderModel {
                id: "model-1".to_owned(),
                model: "gpt-4o-mock".to_owned(),
                max_tokens: 32768,
                context_window: 128000,
                reasoning_effort: None,
                vision: None,
            }],
            headers: None,
        }
    }

    fn app_fixture(mock: &MockOpenAI) -> AppState {
        app_fixture_with_events(mock).0
    }

    fn app_fixture_with_events(mock: &MockOpenAI) -> (AppState, Arc<RecordingEventSink>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let events = Arc::new(RecordingEventSink::new());
        let app = AppState::new_in(dir.path().to_path_buf(), events.clone());
        let config = craftwave_lib::providers::types::AppConfig {
            providers: vec![provider_fixture(&mock.base_url)],
            active_model_id: Some("model-1".to_owned()),
            ..craftwave_lib::providers::types::AppConfig::default()
        };
        app.config.save(config).expect("save config");
        // tempdir drop 会删目录——用 leak 保住（测试进程生命周期内可接受）
        std::mem::forget(dir);
        (app, events)
    }

    fn user_entry(text: &str) -> craftwave_lib::storage::MessageEntry {
        craftwave_lib::storage::MessageEntry {
            id: format!("u-{}", text.len()),
            parent_id: None,
            role: craftwave_lib::storage::MessageRole::User,
            parts: vec![craftwave_lib::storage::WirePart::Text {
                text: text.to_owned(),
            }],
            created_at: 1,
            run_id: None,
            status: None,
        }
    }

    fn run_config() -> craftwave_lib::host::commands::chat::RunConfig {
        run_config_tier(craftwave_lib::safety::tier::PermissionTier::FullAccess)
    }

    fn run_config_tier(
        tier: craftwave_lib::safety::tier::PermissionTier,
    ) -> craftwave_lib::host::commands::chat::RunConfig {
        craftwave_lib::host::commands::chat::RunConfig {
            model_id: None,
            tier,
            max_turns: 4,
            steer: false,
        }
    }

    /// 等待 runEnded 帧出现（命令面任务异步产帧；tokio sleep 不阻塞 worker）。
    async fn wait_ended(sink: &SharedSink, timeout: Duration) -> ChatFrame {
        let deadline = Instant::now() + timeout;
        loop {
            let frames = sink.frames();
            if let Some(last) = frames.last()
                && matches!(last.frame.frame, ChatFrame::RunEnded { .. })
            {
                return last.frame.frame.clone();
            }
            assert!(Instant::now() < deadline, "等待 runEnded 超时");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn start_chat_core_e2e_frames_model_from_config_keyring() {
        let mock = MockOpenAI::start().await;
        let app = app_fixture(&mock);
        let runs = RunManager::new();
        let sink = SharedSink::default();

        let result = craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-1".to_owned(),
            vec![user_entry("帮我读配置文件")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("start_chat_core 应成功");
        assert!(!result.run_id.is_empty());

        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        let ChatFrame::RunEnded { reason, error, .. } = &ended else {
            panic!("应为 runEnded");
        };
        assert_eq!(*reason, EndReason::Done, "error 面：{error:?}");
        let frames = sink.frames();
        // runStarted 的 model = config.activeModelId 解析出的 wire 名
        let ChatFrame::RunStarted { model, .. } = &frames[0].frame.frame else {
            panic!("首帧 runStarted");
        };
        assert_eq!(model, "gpt-4o-mock");
        // RunManager 收尾摘除
        let deadline = Instant::now() + Duration::from_secs(2);
        while runs.get("sess-1").is_some() {
            assert!(Instant::now() < deadline, "run 任务应收尾摘除");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// 15 号票：run 全链后磁盘有 canonical 条目（user 兜底 + turn 文本 + marker）
    /// 与 index.json 建档——NFR-06「已提交历史零丢失」的端到端锚点。
    #[tokio::test]
    async fn run_persists_canonical_entries_and_index() {
        let mock = MockOpenAI::start().await;
        let app = app_fixture(&mock);
        let runs = RunManager::new();
        let sink = SharedSink::default();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-p".to_owned(),
            vec![user_entry("持久化我")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("start_chat_core 应成功");
        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        assert!(matches!(
            ended,
            ChatFrame::RunEnded {
                reason: EndReason::Done,
                ..
            }
        ));

        let store = app.store.clone();
        let tree = store.load_session_tree("sess-p").expect("史册应存在");
        let ids: Vec<&str> = tree.messages.iter().map(|m| m.id.as_str()).collect();
        // 默认 mock 脚本 = 工具调用 turn + 文本 turn（§mock_openai）：
        // user 兜底、turn1 文本、turn1 工具、turn2 文本、marker，共 5 条
        assert_eq!(ids.len(), 5, "实际条目：{ids:?}");
        assert!(ids[0].starts_with("u-"), "user 兜底条目");
        assert!(ids[1].ends_with(":1"), "turn1 文本条目");
        assert!(ids[2].ends_with(":1.tools"), "turn1 工具条目");
        assert!(ids[3].ends_with(":2"), "turn2 文本条目");
        assert!(ids[4].ends_with(":marker"), "runMarker 收口");
        assert_eq!(tree.active_leaf_id.as_deref(), Some(ids[4]));

        let meta = store
            .index()
            .list()
            .into_iter()
            .find(|m| m.id == "sess-p")
            .expect("index.json 应建档");
        assert_eq!(meta.message_count, 5);
        assert_eq!(meta.revision, tree.revision);
    }

    #[tokio::test]
    async fn second_start_same_session_rejected_session_running() {
        // 长流让首个 run 保持 running
        let mut script: Vec<(&'static str, String)> = Vec::new();
        for i in 0..200 {
            script.push(content_delta(&format!("第{i}段。")));
        }
        script.push(finish_chunk("stop"));
        script.push(done_marker());
        let mock = MockOpenAI::start_with_turn1(script).await;
        let app = app_fixture(&mock);
        let runs = RunManager::new();
        let sink = SharedSink::default();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-2".to_owned(),
            vec![user_entry("长问题")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("首个 run 成功");
        while runs.get("sess-2").is_none() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }

        let err = craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-2".to_owned(),
            vec![user_entry("第二个")],
            run_config(),
            Box::new(SharedSink::default()),
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_SESSION_RUNNING", "§3.3 #18 同会话互斥");

        // 取消收尾：置位后 run 终结摘除
        assert!(craftwave_lib::host::commands::chat::cancel_run_core(&runs, "sess-2").await);
        let ended = wait_ended(&sink, Duration::from_secs(5)).await;
        let ChatFrame::RunEnded { reason, .. } = &ended else {
            panic!("取消后 runEnded");
        };
        assert_eq!(*reason, EndReason::Cancelled);
    }

    // —— 6/7. 审批门接线 + plan/ask/suggest（14 号票） ——

    use craftwave_lib::backend::ask::AskAnswer;
    use craftwave_lib::util::events::RecordingEventSink;

    /// 等待某事件键的首个载荷。
    async fn wait_event(
        events: &Arc<RecordingEventSink>,
        key: &str,
        timeout: Duration,
    ) -> serde_json::Value {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(v) = events.payloads_of(key).first() {
                return v.clone();
            }
            assert!(Instant::now() < deadline, "等待事件 {key} 超时");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// turn1 脚本：文本 + 单工具调用（name/args 由调用方给）。
    fn tool_turn1(text: &str, tool: &str, args_json: &str) -> Vec<(&'static str, String)> {
        vec![
            content_delta(text),
            tool_open(0, "call_1", tool),
            tool_args_delta(0, args_json),
            finish_chunk("tool_calls"),
            usage_event(),
            done_marker(),
        ]
    }

    #[tokio::test]
    async fn approval_confirm_each_suspend_allow_resumes_done() {
        let mock = MockOpenAI::start_with_turn1(tool_turn1(
            "需要跑个命令。",
            "command",
            r#"{"command":"echo approved-ok"}"#,
        ))
        .await;
        let (app, events) = app_fixture_with_events(&mock);
        let runs = RunManager::new();
        let sink = SharedSink::default();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-appr".to_owned(),
            vec![user_entry("跑个命令")],
            run_config_tier(craftwave_lib::safety::tier::PermissionTier::ConfirmEach),
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("启动成功");

        // 挂起 → ask:opened（approval 载荷：command + 允许/拒绝）
        let opened = wait_event(&events, "ask:opened", Duration::from_secs(5)).await;
        assert_eq!(opened["payload"]["kind"], "approval");
        assert_eq!(opened["payload"]["command"], "echo approved-ok");
        assert_eq!(
            opened["payload"]["options"][0]["id"], "allow_once",
            "推荐项 = 允许一次"
        );
        let ask_id = opened["askId"].as_str().unwrap().to_owned();

        // 挂起中：无 toolResult / 无 runEnded；会话仍被活跃 run 占用
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            sink.count_kind(|f| matches!(f, ChatFrame::ToolResult { .. })),
            0,
            "挂起时批内零执行"
        );
        assert_eq!(
            sink.count_kind(|f| matches!(f, ChatFrame::RunEnded { .. })),
            0
        );
        assert!(runs.get("sess-appr").is_some(), "挂起期间 run 仍注册");

        // 批准 → 恢复 → 命令执行 → turn2 答复 → Done
        craftwave_lib::host::commands::chat::resolve_ask_core(
            &runs,
            &ask_id,
            AskAnswer::Approval {
                option_id: "allow_once".to_owned(),
                edited_command: None,
            },
        )
        .await
        .expect("resolve_ask 应命中");
        let closed = wait_event(&events, "ask:closed", Duration::from_secs(5)).await;
        assert_eq!(closed["askId"], ask_id.as_str());
        assert_eq!(closed["answered"], true);

        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        let ChatFrame::RunEnded { reason, .. } = &ended else {
            panic!("应为 runEnded")
        };
        assert_eq!(*reason, EndReason::Done);
        // 命令真执行过：toolResult ok + 输出含 approved-ok（帧序：toolResult ×1）
        let results: Vec<ChatFrame> = sink
            .frames()
            .iter()
            .map(|t| t.frame.frame.clone())
            .filter(|f| matches!(f, ChatFrame::ToolResult { .. }))
            .collect();
        assert_eq!(results.len(), 1);
        let ChatFrame::ToolResult { ok, .. } = &results[0] else {
            unreachable!()
        };
        assert!(ok, "批准后命令执行成功");
        // 已答 ask 再 resolve → E_NOT_FOUND（§3.3 #22）
        let err = craftwave_lib::host::commands::chat::resolve_ask_core(
            &runs,
            &ask_id,
            AskAnswer::Approval {
                option_id: "deny".to_owned(),
                edited_command: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_NOT_FOUND");
    }

    #[tokio::test]
    async fn approval_deny_ends_cancelled() {
        let mock = MockOpenAI::start_with_turn1(tool_turn1(
            "再跑一个。",
            "command",
            r#"{"command":"echo denied-ok"}"#,
        ))
        .await;
        let (app, events) = app_fixture_with_events(&mock);
        let runs = RunManager::new();
        let sink = SharedSink::default();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-deny".to_owned(),
            vec![user_entry("再跑")],
            run_config_tier(craftwave_lib::safety::tier::PermissionTier::ConfirmEach),
            Box::new(sink.clone()),
            false,
        )
        .await
        .unwrap();

        let opened = wait_event(&events, "ask:opened", Duration::from_secs(5)).await;
        let ask_id = opened["askId"].as_str().unwrap().to_owned();
        craftwave_lib::host::commands::chat::resolve_ask_core(
            &runs,
            &ask_id,
            AskAnswer::Approval {
                option_id: "deny".to_owned(),
                edited_command: None,
            },
        )
        .await
        .unwrap();

        // 拒绝 → 挂起调用补拒绝 toolResult + runEnded(cancelled)（§5.3）
        let ended = wait_ended(&sink, Duration::from_secs(5)).await;
        let ChatFrame::RunEnded { reason, .. } = &ended else {
            panic!("应为 runEnded")
        };
        assert_eq!(*reason, EndReason::Cancelled);
        let results: Vec<ChatFrame> = sink
            .frames()
            .iter()
            .map(|t| t.frame.frame.clone())
            .filter(|f| matches!(f, ChatFrame::ToolResult { .. }))
            .collect();
        assert_eq!(results.len(), 1, "拒绝结果帧补齐（前端工具卡不悬空）");
        let ChatFrame::ToolResult { ok, error, .. } = &results[0] else {
            unreachable!()
        };
        assert!(!ok);
        assert_eq!(error.as_ref().unwrap().code, "E_CANCELLED");
        // run 收尾摘除
        let deadline = Instant::now() + Duration::from_secs(2);
        while runs.get("sess-deny").is_some() {
            assert!(Instant::now() < deadline, "拒绝后 run 应摘除");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn fence_blacklist_blocked_result_run_continues() {
        let mock = MockOpenAI::start_with_turn1(tool_turn1(
            "清理一下。",
            "command",
            r#"{"command":"rm -rf /tmp/cw-e2e-fence"}"#,
        ))
        .await;
        let (app, events) = app_fixture_with_events(&mock);
        let runs = RunManager::new();
        let sink = SharedSink::default();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-fence".to_owned(),
            vec![user_entry("清理")],
            // full_access 也拦（§8.5 灾难级任何档位不豁免）
            run_config_tier(craftwave_lib::safety::tier::PermissionTier::FullAccess),
            Box::new(sink.clone()),
            false,
        )
        .await
        .unwrap();

        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        let ChatFrame::RunEnded { reason, .. } = &ended else {
            panic!("应为 runEnded")
        };
        assert_eq!(*reason, EndReason::Done, "硬拦不中断 run：{ended:?}");
        let results: Vec<ChatFrame> = sink
            .frames()
            .iter()
            .map(|t| t.frame.frame.clone())
            .filter(|f| matches!(f, ChatFrame::ToolResult { .. }))
            .collect();
        let ChatFrame::ToolResult { ok, error, .. } = &results[0] else {
            unreachable!()
        };
        assert!(!ok, "黑名单命令不执行");
        assert_eq!(error.as_ref().unwrap().code, "E_FENCE_BLOCKED");
        // 无 ask:opened（Block 不走审批）
        assert!(events.payloads_of("ask:opened").is_empty());
    }

    #[tokio::test]
    async fn plan_tool_event_and_approve_plan_command() {
        let mock = MockOpenAI::start_with_turn1(tool_turn1(
            "先建计划。",
            "plan",
            r#"{"action":"set","items":[{"id":"p1","text":"读配置","status":"in_progress"},{"id":"p2","text":"写总结","status":"pending"}]}"#,
        ))
        .await;
        let (app, events) = app_fixture_with_events(&mock);
        let runs = RunManager::new();
        let sink = SharedSink::default();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-plan".to_owned(),
            vec![user_entry("建计划")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .unwrap();

        // plan:update 事件（§3.5：items 全量）
        let update = wait_event(&events, "plan:update", Duration::from_secs(5)).await;
        assert_eq!(update["sessionId"], "sess-plan");
        assert_eq!(update["items"].as_array().unwrap().len(), 2);
        assert_eq!(update["items"][0]["status"], "in_progress");

        // run 活跃期间批准 → 快照冻结（approve_plan 命令面）
        craftwave_lib::host::commands::chat::approve_plan_core(&runs, "sess-plan")
            .expect("活跃 run 上批准成功");

        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        assert!(matches!(
            ended,
            ChatFrame::RunEnded {
                reason: EndReason::Done,
                ..
            }
        ));
        // run 结束后（收尾摘除竞态前批准仍可能成功；无活跃 run 的会话报 E_NOT_FOUND）
        let empty_runs = RunManager::new();
        let err = craftwave_lib::host::commands::chat::approve_plan_core(&empty_runs, "no-run")
            .unwrap_err();
        assert_eq!(err.code, "E_NOT_FOUND");
    }

    #[tokio::test]
    async fn ask_tool_question_resolved_as_tool_result() {
        let mock = MockOpenAI::start_with_turn1(tool_turn1(
            "问一下用户。",
            "ask",
            r#"{"title":"选方案","questions":[{"id":"q1","text":"用哪个方案？","options":[{"id":"a","label":"方案A"},{"id":"b","label":"方案B"}],"allowFreeText":false}]}"#,
        ))
        .await;
        let (app, events) = app_fixture_with_events(&mock);
        let runs = RunManager::new();
        let sink = SharedSink::default();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-ask".to_owned(),
            vec![user_entry("问吧")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .unwrap();

        let opened = wait_event(&events, "ask:opened", Duration::from_secs(5)).await;
        assert_eq!(opened["payload"]["kind"], "question");
        assert_eq!(opened["payload"]["title"], "选方案");
        let ask_id = opened["askId"].as_str().unwrap().to_owned();

        let mut answers = std::collections::BTreeMap::new();
        answers.insert("q1".to_owned(), "a".to_owned());
        craftwave_lib::host::commands::chat::resolve_ask_core(
            &runs,
            &ask_id,
            AskAnswer::Question { answers },
        )
        .await
        .unwrap();

        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        assert!(matches!(
            ended,
            ChatFrame::RunEnded {
                reason: EndReason::Done,
                ..
            }
        ));
        // 回喂模型：第 2 轮请求含工具结果（mock has_tool_result 判据）→ 答复文本
        let bodies = mock.request_bodies().await;
        assert!(
            bodies.len() >= 2
                && bodies[1]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["role"] == "tool"),
            "ask 答案作为工具结果回喂第 2 轮"
        );
        let closed = events.payloads_of("ask:closed");
        assert_eq!(closed[0]["askId"], ask_id.as_str());
        assert_eq!(closed[0]["answered"], true);
    }

    #[tokio::test]
    async fn suggest_background_call_events_and_session_usage() {
        let mock = MockOpenAI::start().await; // 默认脚本：read 工具 + 文本答复
        let (app, events) = app_fixture_with_events(&mock);
        let runs = RunManager::new();
        let sink = SharedSink::default();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-sug".to_owned(),
            vec![user_entry("帮我读配置文件")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .unwrap();
        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        assert!(matches!(
            ended,
            ChatFrame::RunEnded {
                reason: EndReason::Done,
                ..
            }
        ));

        // run:suggestions（FR-06-09）：非空建议文本
        let suggestion = wait_event(&events, "run:suggestions", Duration::from_secs(10)).await;
        assert_eq!(suggestion["sessionId"], "sess-sug");
        assert!(!suggestion["text"].as_str().unwrap().is_empty());

        // tokens:update（FR-01-13 会话口径）：两轮 run 累计 240/96 + suggest 120/48
        // = 360/144（mock 每响应固定 120 in / 48 out）
        let tokens = wait_event(&events, "tokens:update", Duration::from_secs(10)).await;
        let last_tokens = events
            .payloads_of("tokens:update")
            .last()
            .cloned()
            .unwrap_or(tokens);
        assert_eq!(last_tokens["usage"]["input"], 360, "{last_tokens}");
        assert_eq!(last_tokens["usage"]["output"], 144);
        let _ = mock.sent_chunks().await; // 保持 sent 记录引用（Drop 语义锚点）
    }

    // ---- 帧重放与热重连（05 号票，§5.8/D16）：中途 attach 续流 / 终态空重放 /
    // 快照对账 / 审批挂起中 attach 与待审批恢复查询 / 窗口耗尽摘要帧 ----

    /// 等待帧数达到 n（命令面任务异步产帧）。
    async fn wait_frame_count(sink: &SharedSink, n: usize, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if sink.frames().len() >= n {
                return;
            }
            assert!(Instant::now() < deadline, "等待 {n} 帧超时");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// 断言：run 中途以第二下发端 attach → 收到全部重放帧 + 后续增量只走
    /// 新端（旧端断流），runEnded 在新端收口；重放帧与续流帧 seq 连续无重复。
    #[tokio::test]
    async fn attach_mid_run_replays_and_hot_switches_channel() {
        // 长流让 run 保持 running 足够久
        let mut script: Vec<(&'static str, String)> = Vec::new();
        for i in 0..60 {
            script.push(content_delta(&format!("第{i}段。")));
        }
        script.push(finish_chunk("stop"));
        script.push(usage_event());
        script.push(done_marker());
        let mock = MockOpenAI::start_with_turn1(script).await;
        let app = app_fixture(&mock);
        let runs = RunManager::new();
        let session = "sess-attach";
        let old_sink = SharedSink::default();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            session.to_owned(),
            vec![user_entry("长输出任务")],
            run_config(),
            Box::new(old_sink.clone()),
            false,
        )
        .await
        .expect("启动成功");
        // 等 run 注册（帧面尚未结束）
        while runs.get(session).is_none() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        wait_frame_count(&old_sink, 4, Duration::from_secs(5)).await;

        // —— 中途 attach（重载模拟：新下发端 + sinceSeq=0 全量重放）——
        let new_sink = SharedSink::default();
        let result = craftwave_lib::host::commands::chat::attach_run_core(
            &runs,
            session,
            0,
            Box::new(new_sink.clone()),
        );
        let replayed = result.replayed;
        assert!(result.run_id.is_some(), "活跃 run attach 带 runId");
        let old_before = old_sink.frames().len();
        assert_eq!(
            replayed, old_before as u64,
            "重放 = attach 时已发出的全部帧"
        );
        let replayed_seqs: Vec<u64> = new_sink.frames().iter().map(|t| t.frame.seq).collect();
        assert_eq!(
            replayed_seqs,
            (0..old_before as u64).collect::<Vec<_>>(),
            "重放帧原样携带 seq（0 起连续）"
        );

        // —— 续流：新帧只走新端，旧端断流 ——
        let ended = wait_ended(&new_sink, Duration::from_secs(10)).await;
        assert!(matches!(
            ended,
            ChatFrame::RunEnded {
                reason: EndReason::Done,
                ..
            }
        ));
        let old_after = old_sink.frames().len();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            old_sink.frames().len(),
            old_after,
            "热切换后旧端断流（§5.8）"
        );
        let new_frames = new_sink.frames();
        assert!(
            new_frames.len() > replayed as usize,
            "新端 = 重放 + 续流增量"
        );
        // 帧序严格递增无重复（§3.4 排序规则：重放帧先于新帧）
        for (i, t) in new_frames.iter().enumerate() {
            assert_eq!(t.frame.seq, i as u64, "新端帧序连续：位置 {i}");
        }
    }

    /// 断言：run 结束（收尾摘除）后 attach → 空重放且 runId 省略；
    /// session_run_snapshot → null（§3.3 #57/#20 幂等查询语义）。
    #[tokio::test]
    async fn attach_after_run_end_returns_empty_replay_and_null_snapshot() {
        let mock = MockOpenAI::start().await;
        let app = app_fixture(&mock);
        let runs = RunManager::new();
        let session = "sess-attach-end";
        let sink = SharedSink::default();
        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            session.to_owned(),
            vec![user_entry("快速任务")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .unwrap();
        wait_ended(&sink, Duration::from_secs(10)).await;
        // 等收尾摘除
        let deadline = Instant::now() + Duration::from_secs(2);
        while runs.get(session).is_some() {
            assert!(Instant::now() < deadline, "run 应收尾摘除");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let result = craftwave_lib::host::commands::chat::attach_run_core(
            &runs,
            session,
            0,
            Box::new(SharedSink::default()),
        );
        assert_eq!(result.replayed, 0, "无活跃 run → 空重放");
        assert!(result.run_id.is_none(), "runId 省略");
        assert!(
            craftwave_lib::host::commands::chat::session_run_snapshot_core(&runs, session)
                .is_none(),
            "无活跃 run → 快照 null"
        );
        // 未知 sessionId 同语义（幂等，不报 E_NOT_FOUND）
        let missing = craftwave_lib::host::commands::chat::attach_run_core(
            &runs,
            "sess-nope",
            0,
            Box::new(SharedSink::default()),
        );
        assert_eq!(missing.replayed, 0);
        assert!(missing.run_id.is_none());
    }

    /// 断言：session_run_snapshot 与帧面一致（running 态 lastSeq=已发出最大
    /// 帧序号、usage=最后 usage 帧累计、runId 对上）；审批挂起中 attach 仍可
    /// 重放且 resolve_ask 照常恢复（帧重放不改变 run 生命周期或审批决定）；
    /// session_pending_ask 恢复查询在挂起中返回原始载荷、解决后返回 null。
    #[tokio::test]
    async fn snapshot_matches_frames_and_attach_during_approval_pending() {
        let mock = MockOpenAI::start_with_turn1(tool_turn1(
            "需要跑个命令。",
            "command",
            r#"{"command":"echo attach-ok"}"#,
        ))
        .await;
        let (app, events) = app_fixture_with_events(&mock);
        let runs = RunManager::new();
        let session = "sess-attach-ask";
        let sink = SharedSink::default();
        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            session.to_owned(),
            vec![user_entry("跑个命令")],
            run_config_tier(craftwave_lib::safety::tier::PermissionTier::ConfirmEach),
            Box::new(sink.clone()),
            false,
        )
        .await
        .unwrap();
        let opened = wait_event(&events, "ask:opened", Duration::from_secs(5)).await;
        let ask_id = opened["askId"].as_str().unwrap().to_owned();

        // —— 审批挂起中：快照与帧面对账 ——
        let snapshot =
            craftwave_lib::host::commands::chat::session_run_snapshot_core(&runs, session)
                .expect("活跃 run 快照存在");
        assert_eq!(snapshot.status, RunSnapshotStatus::Running);
        let frames = sink.frames();
        let max_seq = frames.last().expect("有帧").frame.seq;
        assert_eq!(snapshot.last_seq, max_seq, "lastSeq = 已发出最大帧序号");
        let last_usage = frames.iter().rev().find_map(|t| match &t.frame.frame {
            ChatFrame::Usage { usage, .. } => Some(*usage),
            _ => None,
        });
        assert_eq!(
            snapshot.usage,
            last_usage.expect("挂起前已有 usage 帧"),
            "usage 与帧面最后 usage 帧一致"
        );
        assert_eq!(
            snapshot.run_id.as_deref(),
            runs.get(session).as_ref().map(|h| h.run_id.as_str()),
            "runId 对上注册表"
        );

        // —— 待审批恢复查询（§5.6 #59c）：原始操作及参数随行 ——
        let pending = craftwave_lib::host::commands::chat::session_pending_ask_core(&runs, session)
            .expect("挂起中的审批应可查");
        assert_eq!(pending.ask_id, ask_id);
        assert_eq!(pending.kind, "approval");
        let payload = serde_json::to_value(&pending.payload).unwrap();
        assert_eq!(payload["kind"], "approval");
        assert_eq!(payload["command"], "echo attach-ok", "原始命令参数随行");

        // —— 挂起中 attach：重放照常，run 生命周期不变 ——
        let replay_sink = SharedSink::default();
        let result = craftwave_lib::host::commands::chat::attach_run_core(
            &runs,
            session,
            0,
            Box::new(replay_sink.clone()),
        );
        assert!(result.run_id.is_some(), "挂起中的 run 仍可 attach");
        assert!(result.replayed > 0, "挂起前帧面非空可重放");
        assert!(
            runs.get(session).is_some(),
            "attach 不改变 run 生命周期（不摘除不重启）"
        );

        // —— 批准 → 恢复 Done；解决后待审批查询为 null（槽位已收口）——
        craftwave_lib::host::commands::chat::resolve_ask_core(
            &runs,
            &ask_id,
            craftwave_lib::backend::ask::AskAnswer::Approval {
                option_id: "allow_once".to_owned(),
                edited_command: None,
            },
        )
        .await
        .expect("挂起中 attach 不影响 resolve_ask");
        let ended = wait_ended(&replay_sink, Duration::from_secs(10)).await;
        assert!(matches!(
            ended,
            ChatFrame::RunEnded {
                reason: EndReason::Done,
                ..
            }
        ));
        assert!(
            craftwave_lib::host::commands::chat::session_pending_ask_core(&runs, session).is_none(),
            "已解决的审批不再返回（有效性以 ask 槽为权威）"
        );
        // 过期 askId 的 resolve 照常 E_NOT_FOUND（重连竞态拒绝面）
        let err = craftwave_lib::host::commands::chat::resolve_ask_core(
            &runs,
            &ask_id,
            craftwave_lib::backend::ask::AskAnswer::Approval {
                option_id: "deny".to_owned(),
                edited_command: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_NOT_FOUND", "过期请求操作被拒绝");
    }

    /// 断言：重放窗口耗尽（人为构造 dropped>0）→ attach_run_core 补发
    /// runSnapshot 摘要帧（末帧、占用新 seq 号、lastSeq=摘要帧之前最大序号）。
    #[tokio::test]
    async fn attach_replay_window_exhausted_sends_snapshot_frame() {
        // 人为构造溢出：直驱枢纽推 514 帧（mock 流出 514 帧耗时过长，
        // 环容量为常量 512，直驱与命令面等价——attach_run_core 走同一枢纽）
        let runs = RunManager::new();
        let session = "sess-overflow";
        let old_sink = SharedSink::default();
        let hub = Arc::new(craftwave_lib::agent::FrameHub::new(Box::new(
            old_sink.clone(),
        )));
        runs.start(session, "run-ovf".to_owned(), 1, hub.clone())
            .unwrap();
        for i in 0..(craftwave_lib::agent::FRAME_BUFFER_CAPACITY + 2) {
            hub.push(ChatFrame::TextDelta {
                turn: 1,
                delta: format!("第{i}段"),
            });
        }

        let new_sink = SharedSink::default();
        let result = craftwave_lib::host::commands::chat::attach_run_core(
            &runs,
            session,
            0,
            Box::new(new_sink.clone()),
        );
        // 重放 = 环内 512 帧（seq 2..=513）；摘要帧补发
        assert!(result.replayed == craftwave_lib::agent::FRAME_BUFFER_CAPACITY as u64);
        let frames = new_sink.frames();
        let last = frames.last().expect("有帧");
        let ChatFrame::RunSnapshot { snapshot } = &last.frame.frame else {
            panic!("末帧应为 runSnapshot 摘要帧，实际 {:?}", last.frame.frame);
        };
        assert_eq!(snapshot.status, RunSnapshotStatus::Running);
        assert_eq!(
            snapshot.run_id.as_deref(),
            Some("run-ovf"),
            "摘要帧负载对齐 §3.2 RunSnapshot（runId 随行）"
        );
        assert_eq!(
            snapshot.last_seq,
            (craftwave_lib::agent::FRAME_BUFFER_CAPACITY + 1) as u64,
            "lastSeq = 摘要帧之前的最大帧序号"
        );
        assert_eq!(
            last.frame.seq,
            (craftwave_lib::agent::FRAME_BUFFER_CAPACITY + 2) as u64,
            "摘要帧占用新 seq 号，后续帧连续"
        );
    }

    // ---- 上下文压缩（04 号票，FR-03-05）：跨阈值自动压缩 / 失败兜底 /
    // 手动命令 / 超预算暂停 ----

    /// 压缩 e2e 的配置面：把激活模型窗口改小让第二条消息必然跨 0.6 阈值
    ///（估算面含 32768 回复预留 > 0.6 × 40000）。
    fn small_window_config(base_url: &str) -> craftwave_lib::providers::types::AppConfig {
        craftwave_lib::providers::types::AppConfig {
            providers: vec![craftwave_lib::providers::types::ProviderConfig {
                models: vec![craftwave_lib::providers::types::ProviderModel {
                    context_window: 40000,
                    ..provider_fixture(base_url).models.remove(0)
                }],
                ..provider_fixture(base_url)
            }],
            active_model_id: Some("model-1".to_owned()),
            ..craftwave_lib::providers::types::AppConfig::default()
        }
    }

    /// 断言：长会话跨阈值自动压缩——compactDone 帧 + run:compacted 事件 +
    /// 史册 compaction 条目（原始消息全保留）+ 后续请求 wire 只含 summary
    /// 注入与本次输入（cutEntryId 之前的历史被裁）+ 摘要用量计入会话（C-9）。
    #[tokio::test]
    async fn auto_compaction_across_threshold_frame_event_wire_and_metering() {
        let mock = MockOpenAI::start().await;
        let (app, events) = app_fixture_with_events(&mock);
        app.config
            .save(small_window_config(&mock.base_url))
            .expect("save config");
        let runs = RunManager::new();
        let sink = SharedSink::default();
        let session = "sess-comp";

        // 第一次发送（无历史）：达阈值但无可压内容 → run:compact_failed 事件
        // + 原上下文可容纳 → run 照常完成（§6.2 失败兜底）
        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            session.to_owned(),
            vec![user_entry("第一条消息")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("第一次 start_chat 应成功");
        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        assert!(matches!(
            ended,
            ChatFrame::RunEnded {
                reason: EndReason::Done,
                ..
            }
        ));
        assert!(
            !events.payloads_of("run:compact_failed").is_empty(),
            "无可压内容的压缩尝试应发失败事件"
        );
        let tree = app.store.load_session_tree(session).unwrap();
        assert!(tree.compactions.is_empty(), "失败不得提交边界");

        // 第二次发送（历史在史册、messages 参数含旧消息 + 本次输入）：
        // 触发压缩（body=[长历史条目]）→ 摘要（非流式 mock）→ 落盘 → wire 重建。
        // 历史用长文本：压缩后摘要（固定 30 token 级）必须显著小于被裁历史
        //（tokensBefore > tokensAfter 的载荷语义成立）
        let long_text = "历史".repeat(500);
        let first = user_entry(&long_text);
        let second = user_entry("第二条内容消息"); // 字节数不同 → id 不撞车
        // 前端协议模拟：长历史条目先经 save_session_tree 落盘（挂第一次
        // run 的链尾），start_chat 只对末条 user 做幂等兜底（§7.3）
        {
            let tree = app.store.load_session_tree(session).unwrap();
            let tail = tree.active_leaf_id.clone().unwrap();
            let mut first_entry = first.clone();
            first_entry.parent_id = Some(tail);
            let rev = tree.revision;
            app.store
                .save_session_tree(session, &[first_entry], Some(&first.id), rev)
                .unwrap();
        }
        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            session.to_owned(),
            vec![first, second],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("第二次 start_chat 应成功");
        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        assert!(matches!(
            ended,
            ChatFrame::RunEnded {
                reason: EndReason::Done,
                ..
            }
        ));

        // 史册：compaction 条目落地、原始消息全保留（压缩不丢记录）
        let tree = app.store.load_session_tree(session).unwrap();
        assert_eq!(tree.compactions.len(), 1, "{:?}", tree.compactions);
        let long_id = format!("u-{}", long_text.len());
        assert_eq!(
            tree.compactions[0].cut_entry_id, long_id,
            "cut = 被压缩历史链尾"
        );
        assert_eq!(tree.compactions[0].summary, "试试让我总结配置要点。");
        let ids: Vec<&str> = tree.messages.iter().map(|m| m.id.as_str()).collect();
        assert!(
            ids.contains(&long_id.as_str()),
            "原始历史不得被裁掉：{ids:?}"
        );

        // 帧面：compactDone（本次 run 的 sink 含两次 run 的帧；至少一帧）
        assert!(
            sink.count_kind(|f| matches!(f, ChatFrame::CompactDone { .. })) >= 1,
            "应有 compactDone 帧"
        );
        // 事件面：run:compacted（tokensBefore/tokensAfter 载荷）
        let compacted = events
            .payloads_of("run:compacted")
            .last()
            .cloned()
            .expect("应有 run:compacted 事件");
        assert_eq!(compacted["sessionId"], session);
        let before = compacted["tokensBefore"].as_u64().unwrap();
        let after = compacted["tokensAfter"].as_u64().unwrap();
        assert!(before > after, "压缩后占用应下降：{compacted}");

        // wire 面：第二次 run 的流式请求 messages[0] = system 摘要注入，
        // 且不含被压缩的"第一条消息"（cutEntryId 之前被裁）
        let requests = mock.request_bodies().await;
        let streaming: Vec<&serde_json::Value> = requests
            .iter()
            .filter(|r| r["stream"] == serde_json::Value::Bool(true))
            .collect();
        let last_stream = streaming.last().expect("应有流式请求");
        let messages = last_stream["messages"].as_array().unwrap();
        assert_eq!(
            messages[0]["role"], "system",
            "摘要应注入为 system 前缀：{messages:?}"
        );
        assert_eq!(
            messages[0]["content"][0]["text"], "试试让我总结配置要点。",
            "摘要文本 = mock 非流式答复（rig system content 数组形态）"
        );
        let joined = serde_json::to_string(&messages).unwrap();
        assert!(
            !joined.contains(&long_text),
            "被压缩历史不得进入 wire：{joined}"
        );
        assert!(joined.contains("第二条内容消息"), "本次输入保留：{joined}");

        // 计量面（C-9）：摘要用量计入会话（tokens:update 照发）
        assert!(
            !events.payloads_of("tokens:update").is_empty(),
            "压缩摘要用量应计入会话累计"
        );
    }

    /// 断言：摘要失败注入——run:compact_failed 后原上下文可容纳 → run 继续
    /// 完成；边界不推进（不提交 compaction 条目）；原始历史未被静默裁剪。
    #[tokio::test]
    async fn compaction_failure_keeps_history_and_continues_when_fits() {
        let mock = MockOpenAI::start().await;
        let (app, events) = app_fixture_with_events(&mock);
        app.config
            .save(small_window_config(&mock.base_url))
            .expect("save config");
        mock.set_fail_nonstream(true); // 非流式（摘要调用）一律 500
        let runs = RunManager::new();
        let sink = SharedSink::default();
        let session = "sess-fail";

        // 先让史册有内容（第一次 run 也会尝试压缩并失败——注入已在）
        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            session.to_owned(),
            vec![user_entry("第一条消息")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .unwrap();
        wait_ended(&sink, Duration::from_secs(10)).await;

        // 第二次：有可压内容但摘要失败 → 兜底继续（可容纳）
        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            session.to_owned(),
            vec![user_entry("第一条消息"), user_entry("第二条消息")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .unwrap();
        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        assert!(matches!(
            ended,
            ChatFrame::RunEnded {
                reason: EndReason::Done,
                ..
            }
        ));
        assert!(
            !events.payloads_of("run:compact_failed").is_empty(),
            "摘要失败应发 run:compact_failed"
        );
        let tree = app.store.load_session_tree(session).unwrap();
        assert!(
            tree.compactions.is_empty(),
            "失败不得提交边界：{:?}",
            tree.compactions
        );
        // wire 未裁剪：本次流式请求含完整历史（未经摘要的历史不被静默裁掉）
        let requests = mock.request_bodies().await;
        let streaming: Vec<&serde_json::Value> = requests
            .iter()
            .filter(|r| r["stream"] == serde_json::Value::Bool(true))
            .collect();
        let joined = serde_json::to_string(streaming.last().unwrap()).unwrap();
        assert!(
            joined.contains("第一条消息"),
            "失败后 wire 不得裁历史：{joined}"
        );
    }

    /// 断言：压缩后仍超硬预算 → E_CONTEXT_OVERFLOW 暂停，输入条目已先落盘
    /// 保留（persist 兜底先于压缩），run 不启动。
    #[tokio::test]
    async fn context_overflow_after_compaction_pauses_with_input_kept() {
        let mock = MockOpenAI::start().await;
        let (app, _events) = app_fixture_with_events(&mock);
        // 窗口 30000：阈值线 18000（触发压缩）、硬预算 27000（预留 32768
        // 必然超出——压缩成功也放不下）
        let config = craftwave_lib::providers::types::AppConfig {
            providers: vec![craftwave_lib::providers::types::ProviderConfig {
                models: vec![craftwave_lib::providers::types::ProviderModel {
                    context_window: 30000,
                    ..provider_fixture(&mock.base_url).models.remove(0)
                }],
                ..provider_fixture(&mock.base_url)
            }],
            active_model_id: Some("model-1".to_owned()),
            ..craftwave_lib::providers::types::AppConfig::default()
        };
        app.config.save(config).expect("save config");
        let runs = RunManager::new();
        let sink = SharedSink::default();

        let err = craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-overflow".to_owned(),
            vec![user_entry("超大输入")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect_err("应因超硬预算被拒");
        assert_eq!(err.code, "E_CONTEXT_OVERFLOW", "{err}");
        // 输入保留：user 兜底条目已在史册（暂停不丢输入）
        let tree = app.store.load_session_tree("sess-overflow").unwrap();
        assert!(
            tree.messages.iter().any(|m| m.id == "u-12"),
            "输入条目应已落盘保留：{:?}",
            tree.messages.iter().map(|m| &m.id).collect::<Vec<_>>()
        );
        assert!(runs.get("sess-overflow").is_none(), "run 不应启动");
    }

    /// 断言：手动 compact_session——快照压缩成功（返回 cutEntryId 与前后
    /// 估算）；运行中拒绝（E_SESSION_RUNNING）。
    #[tokio::test]
    async fn manual_compact_session_snapshot_and_running_rejected() {
        let mock = MockOpenAI::start().await;
        let (app, events) = app_fixture_with_events(&mock);
        app.config
            .save(small_window_config(&mock.base_url))
            .expect("save config");
        let runs = RunManager::new();
        let session = "sess-manual";
        app.store
            .save_session_tree(session, &[user_entry("第一条消息")], Some("u-15"), 0)
            .unwrap();

        // 运行中拒绝：start_chat 启动后（run 未收口）手动压缩即拒
        {
            let sink = SharedSink::default();
            craftwave_lib::host::commands::chat::start_chat_core(
                &app,
                &runs,
                session.to_owned(),
                vec![user_entry("第一条消息")],
                run_config(),
                Box::new(sink.clone()),
                false,
            )
            .await
            .unwrap();
            let err =
                craftwave_lib::host::commands::chat::compact_session_core(&app, &runs, session)
                    .await
                    .expect_err("运行中应拒绝");
            assert_eq!(err.code, "E_SESSION_RUNNING", "{err}");
            wait_ended(&sink, Duration::from_secs(10)).await;
        }

        // 快照压缩：无条件执行（不走阈值判定）。此时史册已含第一次 run 的
        // 产物条目，压缩候选 = 全量（无"本次输入"豁免）→ cut = 史册链尾 marker
        let result =
            craftwave_lib::host::commands::chat::compact_session_core(&app, &runs, session)
                .await
                .expect("手动压缩应成功");
        let tree = app.store.load_session_tree(session).unwrap();
        let tail_id = tree.active_leaf_id.clone().expect("史册应有活动叶");
        assert_eq!(
            result.cut_entry_id.as_deref(),
            Some(tail_id.as_str()),
            "{result:?}"
        );
        assert_eq!(tree.compactions.len(), 1);
        assert_eq!(
            tree.compactions[0].id, "manual:compact:1",
            "手动 owner=manual"
        );
        assert!(tree.messages.iter().any(|m| m.id == "u-15"), "原始历史保留");
        assert!(
            events.payloads_of("tokens:update").is_empty()
                || events.payloads_of("run:compacted").is_empty(),
            "手动压缩走命令返回面（不发生自动压缩事件流）"
        );
    }

    // —— 8. steer / followUp 队列（06 号票，§5.5）——

    use craftwave_lib::host::commands::chat::{
        InjectMode, inject_run_message_core, remove_followup_core, resume_followup_queue_core,
    };
    use craftwave_lib::storage::MessageRole;

    /// run 正常完成后的两连发场景（06 号票 e2e）：
    /// 空闲时 followUp 入队 → run1 正常完成 → 队列头自动投递开 run2（走同一
    /// start_chat_core 入口、DetachedSink 帧只入环）→ 队列空 + run2 的 user
    /// 条目与产物落盘 + run:queue 事件随行。
    #[tokio::test]
    async fn followup_auto_delivery_after_done_two_runs_back_to_back() {
        let mock = MockOpenAI::start().await;
        let (app, events) = app_fixture_with_events(&mock);

        // 空闲时 Alt+Enter 排队（mode=followUp + 附着 runConfig 快照）
        let injected = inject_run_message_core(
            &app,
            "sess-fu",
            "排队第二条",
            Some(InjectMode::FollowUp),
            Some(&run_config()),
        )
        .expect("空闲入 followUp 应成功");
        assert!(injected.queued);
        assert_eq!(injected.mode, Some(InjectMode::FollowUp));
        assert_eq!(app.followups.snapshot("sess-fu").size, 1);
        // 入队即落盘（§7.7）
        assert!(
            app.store
                .root()
                .join("sessions")
                .join("sess-fu.queue.json")
                .exists(),
            "queue.json 应在入队时生成"
        );

        // run1 启动并跑完（默认脚本两轮 → Done）
        let sink = SharedSink::default();
        let first = craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &app.runs,
            "sess-fu".to_owned(),
            vec![user_entry("第一条消息")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("run1 应成功");
        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        assert!(
            matches!(
                ended,
                ChatFrame::RunEnded {
                    reason: EndReason::Done,
                    ..
                }
            ),
            "run1 应正常完成"
        );

        // run2 自动投递（收尾 spawn 异步投递）：等第二个 run:start 事件
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let starts = events.payloads_of("run:start");
            if starts.len() >= 2 {
                break;
            }
            assert!(Instant::now() < deadline, "等待 run2 自动投递超时");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let second_run_id = events.payloads_of("run:start")[1]["runId"]
            .as_str()
            .expect("run2 的 runId")
            .to_owned();
        assert_ne!(second_run_id, first.run_id, "自动投递应新开 run");

        // run2 收口（Done 事件；帧面走 DetachedSink 不进 sink1）
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if events.payloads_of("run:done").len() >= 2 {
                break;
            }
            assert!(Instant::now() < deadline, "等待 run2 收口超时");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // 队列空（run2 已消费队列头）+ canonical 含 run2 的 user 条目与产物
        let snap = app.followups.snapshot("sess-fu");
        assert_eq!(snap.size, 0, "队列头应已被投递消费");
        let tree = app.store.load_session_tree("sess-fu").unwrap();
        let auto_user = tree
            .messages
            .iter()
            .find(|m| m.id.starts_with("auto-fu-"))
            .expect("run2 的 user 条目应兜底落盘");
        assert_eq!(auto_user.role, MessageRole::User);
        let run2_ids: Vec<&str> = tree
            .messages
            .iter()
            .filter(|m| {
                m.run_id.as_deref() == Some(second_run_id.as_str())
                    || m.id == format!("{second_run_id}:marker")
            })
            .map(|m| m.id.as_str())
            .collect();
        assert!(
            run2_ids.iter().any(|id| id.ends_with(":1")),
            "run2 的 turn 文本条目应落盘：{run2_ids:?}"
        );
        assert!(
            run2_ids.iter().any(|id| id.ends_with(":marker")),
            "run2 的 marker 应落盘（marker 条目按 id 识别，run_id 字段为空）：{run2_ids:?}"
        );

        // run:queue 事件随行（入队一次 size=1 + 投递后 size=0）
        let queue_events = events.payloads_of("run:queue");
        assert!(
            queue_events
                .iter()
                .any(|p| p["size"].as_u64() == Some(1) && !p["paused"].as_bool().unwrap()),
            "入队事件：{queue_events:?}"
        );
        assert!(
            queue_events.iter().any(|p| p["size"].as_u64() == Some(0)),
            "投递后清空事件：{queue_events:?}"
        );

        // 第二次请求的 history 应含排队文本（run2 的首请求）
        let bodies = mock.request_bodies().await;
        let with_queued = bodies
            .iter()
            .filter_map(|b| b["messages"].as_array())
            .filter(|msgs| {
                msgs.iter()
                    .any(|m| m["content"].as_str() == Some("排队第二条"))
            })
            .count();
        assert!(with_queued >= 1, "自动投递请求应携带排队消息");
    }

    /// 运行中 inject → steer 在 turn 边界被消费：run:inject 回执 + steer
    /// 条目落 canonical + 下一请求 history 含插话 user 条目（run 不终止）。
    #[tokio::test]
    async fn steer_injected_midrun_consumed_at_turn_boundary() {
        let mock = MockOpenAI::start().await;
        let (app, events) = app_fixture_with_events(&mock);
        let sink = SharedSink::default();

        let first = craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &app.runs,
            "sess-steer".to_owned(),
            vec![user_entry("帮我读配置文件")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("run 应成功");

        // 等 run1 进入工具轮（toolStarted 帧出现）后插话——默认脚本
        // turn1 工具调用 → turn2 文本，插话应在 turn2 请求前被消费
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if sink
                .frames()
                .iter()
                .any(|t| matches!(t.frame.frame, ChatFrame::ToolStarted { .. }))
            {
                break;
            }
            assert!(Instant::now() < deadline, "等待 turn1 工具开始超时");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let injected = inject_run_message_core(&app, "sess-steer", "顺便看下端口", None, None)
            .expect("运行中 steer 应成功");
        assert!(injected.queued);
        assert_eq!(
            injected.mode,
            Some(InjectMode::Steer),
            "有活跃 run 应归 steer"
        );

        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        assert!(
            matches!(
                ended,
                ChatFrame::RunEnded {
                    reason: EndReason::Done,
                    ..
                }
            ),
            "插话不应终止 run"
        );

        // run:inject 回执（acceptedAtTurn = 消费时的下一轮次；entryId 规则）
        let injects = events.payloads_of("run:inject");
        assert_eq!(injects.len(), 1, "恰好消费一次：{injects:?}");
        assert_eq!(injects[0]["sessionId"], "sess-steer");
        assert_eq!(injects[0]["runId"], first.run_id);
        assert_eq!(injects[0]["text"], "顺便看下端口");
        let entry_id = injects[0]["entryId"].as_str().expect("entryId");
        assert_eq!(
            entry_id,
            format!("{}:steer:1", first.run_id),
            "id 规则 = {{runId}}:steer:{{n}}"
        );
        let accepted_turn = injects[0]["acceptedAtTurn"]
            .as_u64()
            .expect("acceptedAtTurn");
        assert!(accepted_turn >= 2, "turn 边界消费（turn>1）");

        // steer 条目已落 canonical（id 同回执）
        let tree = app.store.load_session_tree("sess-steer").unwrap();
        let steer_entry = tree
            .messages
            .iter()
            .find(|m| m.id == entry_id)
            .expect("steer 条目应落盘");
        assert_eq!(steer_entry.role, MessageRole::User);

        // 下一请求（第 2 轮，带 tool result）的 history 含插话 user 内容
        let bodies = mock.request_bodies().await;
        let tool_round = bodies
            .iter()
            .find(|b| {
                b["messages"]
                    .as_array()
                    .map(|msgs| msgs.iter().any(|m| m["role"] == "tool"))
                    .unwrap_or(false)
            })
            .expect("应有工具轮请求");
        let messages = tool_round["messages"].as_array().unwrap();
        // rig OpenAI wire 把纯文本 user 消息序列化为 content 字符串形态
        let has_steer = messages
            .iter()
            .any(|m| m["role"] == "user" && m["content"] == "顺便看下端口");
        assert!(has_steer, "第 2 轮请求应含插话 user 条目：{tool_round}");
    }

    /// cancel 后队列清空且持久化文件删除；取消前入队的待发文本随
    /// run:queue.returned 交还（FR-01-07 返还草稿）。
    #[tokio::test]
    async fn cancel_clears_queue_and_deletes_persisted_file() {
        let mock = MockOpenAI::start().await;
        let (app, events) = app_fixture_with_events(&mock);
        let sink = SharedSink::default();

        // 取消前排队两条
        inject_run_message_core(
            &app,
            "sess-cancel",
            "取消也不该丢的草稿一",
            Some(InjectMode::FollowUp),
            Some(&run_config()),
        )
        .unwrap();
        inject_run_message_core(
            &app,
            "sess-cancel",
            "取消也不该丢的草稿二",
            Some(InjectMode::FollowUp),
            Some(&run_config()),
        )
        .unwrap();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &app.runs,
            "sess-cancel".to_owned(),
            vec![user_entry("跑起来然后取消我")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("run 应成功");

        // 等首帧出现后取消（cancel_run 置位即返回）
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if !sink.frames().is_empty() {
                break;
            }
            assert!(Instant::now() < deadline, "等待首帧超时");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(
            craftwave_lib::host::commands::chat::cancel_run_core(&app.runs, "sess-cancel").await,
            "取消应命中活跃 run"
        );
        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        assert!(
            matches!(
                ended,
                ChatFrame::RunEnded {
                    reason: EndReason::Cancelled,
                    ..
                }
            ),
            "取消终态：{ended:?}"
        );

        // 队列清空 + 文件删除 + returned 交还
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if app.followups.snapshot("sess-cancel").size == 0 {
                let returned = events
                    .payloads_of("run:queue")
                    .last()
                    .and_then(|p| p["returned"].as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                if returned == 2 {
                    break;
                }
            }
            assert!(Instant::now() < deadline, "等待清空返还超时");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let returned = events.payloads_of("run:queue").last().unwrap()["returned"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            returned,
            vec![
                "取消也不该丢的草稿一".to_owned(),
                "取消也不该丢的草稿二".to_owned()
            ],
            "按排队序交还"
        );
        assert!(
            !app.store
                .root()
                .join("sessions")
                .join("sess-cancel.queue.json")
                .exists(),
            "取消后队列文件应删除"
        );
    }

    /// 失败暂停与明确继续（06 号票验收回归）：run 以 runEnded(error) 收场 →
    /// 队列暂停、待发保留；resume_followup_queue 解除后立即投递队列头开新
    /// run 并跑到收口（模型可用面由 fixture 保证）。
    #[tokio::test]
    async fn failure_pauses_queue_until_explicit_resume() {
        let mock = MockOpenAI::start().await;
        let (app, events) = app_fixture_with_events(&mock);
        let sink = SharedSink::default();

        inject_run_message_core(
            &app,
            "sess-pause",
            "失败后应保留的待发",
            Some(InjectMode::FollowUp),
            Some(&run_config()),
        )
        .unwrap();

        // 让 run 失败：maxTurns=0 → rig 预算即耗尽，run 任务内以错误终态收场
        //（start_chat 命令面本身成功——失败发生在 run 任务内，收尾暂停可测）
        let mut bad_config = run_config();
        bad_config.max_turns = 0;
        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &app.runs,
            "sess-pause".to_owned(),
            vec![user_entry("必失败")],
            bad_config,
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("start 命令面应成功（失败发生在 run 任务内）");
        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        assert!(
            matches!(
                ended,
                ChatFrame::RunEnded {
                    reason: EndReason::Error,
                    ..
                }
            ),
            "maxTurns=0 应以错误收场：{ended:?}"
        );

        // 失败收尾：队列暂停 + 待发保留
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snap = app.followups.snapshot("sess-pause");
            if snap.paused && snap.size == 1 {
                break;
            }
            assert!(Instant::now() < deadline, "等待失败暂停超时");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let last = events
            .payloads_of("run:queue")
            .last()
            .expect("应有 run:queue 事件")
            .clone();
        assert_eq!(last["paused"], true, "失败即暂停");
        assert_eq!(last["size"], 1, "待发保留");

        // 用户明确继续：解除暂停并立即投递队列头（模型可用 → 新 run 跑到收口）
        let resumed = resume_followup_queue_core(&app, "sess-pause")
            .await
            .expect("resume 应成功");
        assert!(resumed.delivered, "解除后应立即投递队列头");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if !events.payloads_of("run:done").is_empty() {
                break;
            }
            assert!(Instant::now() < deadline, "等待投递 run 收口超时");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(app.followups.snapshot("sess-pause").size, 0, "队列清空");
    }

    /// remove_followup：命中移除返回剩余长度；重复删除 E_NOT_FOUND。
    #[tokio::test]
    async fn remove_followup_entry_and_miss_reports_not_found() {
        let mock = MockOpenAI::start().await;
        let (app, _events) = app_fixture_with_events(&mock);

        inject_run_message_core(
            &app,
            "sess-rm",
            "留下的",
            Some(InjectMode::FollowUp),
            Some(&run_config()),
        )
        .unwrap();
        inject_run_message_core(
            &app,
            "sess-rm",
            "要删的",
            Some(InjectMode::FollowUp),
            Some(&run_config()),
        )
        .unwrap();
        let entry_id = app
            .followups
            .peek_head("sess-rm")
            .map(|e| e.id)
            .expect("队头");
        let result = remove_followup_core(&app, "sess-rm", &entry_id).expect("命中应成功");
        assert_eq!(result.size, 1);
        let err = remove_followup_core(&app, "sess-rm", &entry_id).expect_err("重复删除应未命中");
        assert_eq!(err.code, "E_NOT_FOUND");
        assert_eq!(app.followups.snapshot("sess-rm").size, 1);
    }
}
