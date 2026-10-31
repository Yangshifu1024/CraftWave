//! backend 产品化链路端到端（mock OpenAI 兼容 SSE，离线确定性）：
//!
//! 1. 帧保真与 callId 同源：文本/思考/工具参数流 → RunLoop 生产路径（64ms
//!    节流内聚）→ 帧序列（runStarted/stepStarted/delta/toolStarted/toolArgs/
//!    toolResult/usage/runEnded），帧面三帧 callId 同源、seq 严格递增、
//!    usage 累计、TTFT <100ms 与节流间隔断言；
//! 2. 取消全链：流中取消（<1s，runEnded(cancelled) 收齐后最后一帧）；
//! 3. 取消收齐语义：工具批内已完成保留、未完成补 E_CANCELLED；
//! 4. 审批挂起机制层：gate=Suspend 序列化挂起 → resume_run → 跑到 Done
//!    （增量：批级预判零执行、DriveState 延续、callId 跨挂起同源）；
//! 5. start_chat_core 命令面：模型解析（config+keyring 注入）、同会话互斥
//!    E_SESSION_RUNNING、RunManager 取消；
//! 6. 审批门接线：confirm_each 挂起 → ask:opened → resolve(allow)
//!    → 恢复 Done；resolve(deny) → 拒绝终态；围栏黑名单 Block 回喂不中断；
//! 7. plan/ask/suggest：plan:update 事件 + approve_plan、
//!    ask 问卷 resolve 回喂、suggest 后台小调用 run:suggestions + tokens:update；
//! 8. 并行工具批：ask 混批互斥（只执行 ask、被跳过项不申请
//!    审批）与源序回喂。

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::mock_openai::{
    MockOpenAI, TURN1_REASONING, TURN1_TEXT, TURN1_TOOL_ARG_PARTS, TURN2_TEXT, content_delta,
    done_marker, finish_chunk, tool_args_delta, tool_open, usage_event,
};
use craftwave_engine::agent::RunManager;
use craftwave_engine::backend::frames::{
    ChatFrame, EndReason, FrameSink, RunSnapshotStatus, SequencedFrame, SequencedSink,
};
use craftwave_engine::backend::run_loop::{
    AllowAllGate, CancellableExecutor, GateVerdict, RunExit, ToolExecution, ToolGate, drive_run,
    resume_run,
};
use craftwave_engine::backend::turn::TurnDriver;
use craftwave_engine::app_state::AppState;
use rig::completion::Message;
use rig::run::AgentRun;
use rig::run::spec::RunSpec;
use tokio_util::sync::CancellationToken;

const TOOL_READ: &str = "read";
const TOOL_COMMAND: &str = "command";
const FULL_CHAIN_BUDGET: Duration = Duration::from_millis(1000);

// —— 测试装配（与 tests/common 解耦，产品化链路自包含） ——

/// 指向 mock 的 rig Chat 模型。
fn mock_model(mock: &MockOpenAI) -> rig::driver::Model<rig::providers::openai::wire::Chat> {
    let config = rig::providers::openai::wire::OpenAIConfig::new("mock-key".to_owned())
        .with_base_url(mock.base_url.clone());
    config.client().chat("gpt-4o-mock".to_owned())
}

/// 工具定义（read：mock 脚本点名；command：长命令载体）。
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
///   统一递增后下发，本实现原样收帧。
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

// —— 1. 帧保真 + callId 同源 + 节流 + TTFT ——

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
        None, // 每拍请求刷新源（测试直驱路径无）
        std::time::Duration::from_secs(30),
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

    // —— callId 同源：toolStarted/toolArgs/toolResult 帧面同一 id ——
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
    assert_eq!(usages, vec![(120, 48), (240, 96)], "usage 帧为运行累计口径");

    // —— TTFT（mock 首 chunk → 首个 delta 帧出槽）<100ms ——
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
    assert!(ttft_ms < 100.0, "TTFT {ttft_ms:.1}ms 超出预算（<100ms）");

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
            None, // 每拍请求刷新源（测试直驱路径无）
            std::time::Duration::from_secs(30),
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
/// 在三平台 CI 各自真实执行取消传播（Windows 不再跳过）。
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
                        let status = craftwave_engine::tools::process::reap_process_group(
                            &mut tree.child,
                            craftwave_engine::tools::process::REAP_GRACE,
                        )
                        .await
                        .expect("进程组收割");
                        #[cfg(windows)]
                        let status = tree
                            .job
                            .reap(&mut tree.child, craftwave_engine::tools::process::REAP_GRACE)
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
    craftwave_engine::tools::process::enable_process_group(&mut cmd);
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
    job: craftwave_engine::tools::process::JobGuard,
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
    let job = craftwave_engine::tools::process::JobGuard::new().expect("创建 Job Object");
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
            None, // 每拍请求刷新源（测试直驱路径无）
            std::time::Duration::from_secs(30),
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
        let alive = craftwave_engine::tools::process::pid_alive(grandchild as i32);
        #[cfg(windows)]
        let alive = craftwave_engine::tools::process::pid_alive(grandchild as u32);
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
            GateVerdict::Suspend {
                fence_reason: None,
                external_dir: None,
                plan_scope: None,
            }
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
        None, // 每拍请求刷新源（测试直驱路径无）
        std::time::Duration::from_secs(30),
    )
    .await
    .expect("挂起路径应正常返回");
    let RunExit::Suspended(info) = exit else {
        panic!("应为 Suspended，实际 {exit:?}");
    };
    let craftwave_engine::backend::run_loop::SuspendedInfo {
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
        None, // 每拍请求刷新源（测试直驱路径无）
        std::time::Duration::from_secs(30),
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

/// 多工具批的 turn1 脚本：正文 + 按序多个工具调用（并行批驱动面）。
fn multi_tool_turn1(calls: &[(&str, &str, &str)]) -> Vec<(&'static str, String)> {
    let mut script: Vec<(&'static str, String)> = vec![content_delta("一次交代多件事。")];
    for (i, (id, name, args)) in calls.iter().enumerate() {
        script.push(tool_open(i, id, name));
        script.push(tool_args_delta(i, args));
    }
    script.push(finish_chunk("tool_calls"));
    script.push(usage_event());
    script.push(done_marker());
    script
}

/// 批内 ToolResult 帧面序列（callId/ok/error.code 三元组，源序断言用）。
fn tool_result_faces(frames: &[TimedFrame]) -> Vec<(String, bool, Option<String>)> {
    frames
        .iter()
        .filter_map(|t| match &t.frame.frame {
            ChatFrame::ToolResult {
                call_id, ok, error, ..
            } => Some((
                call_id.clone(),
                *ok,
                error.as_ref().map(|e| e.code.to_owned()),
            )),
            _ => None,
        })
        .collect()
}

mod command_face {
    use super::*;

    use craftwave_engine::providers::types::{ApiFormat, ProviderConfig, ProviderModel};

    pub(super) fn provider_fixture(base_url: &str) -> ProviderConfig {
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
            local: false,
        }
    }

    fn app_fixture(mock: &MockOpenAI) -> AppState {
        app_fixture_with_events(mock).0
    }

    pub(super) fn app_fixture_with_events(
        mock: &MockOpenAI,
    ) -> (AppState, Arc<RecordingEventSink>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let events = Arc::new(RecordingEventSink::new());
        let app = AppState::new_in(dir.path().to_path_buf(), events.clone());
        let config = craftwave_engine::providers::types::AppConfig {
            providers: vec![provider_fixture(&mock.base_url)],
            active_model_id: Some("model-1".to_owned()),
            ..craftwave_engine::providers::types::AppConfig::default()
        };
        app.config.save(config).expect("save config");
        // tempdir drop 会删目录——用 leak 保住（测试进程生命周期内可接受）
        std::mem::forget(dir);
        (app, events)
    }

    pub(super) fn user_entry(text: &str) -> craftwave_engine::storage::MessageEntry {
        craftwave_engine::storage::MessageEntry {
            id: format!("u-{}", text.len()),
            parent_id: None,
            role: craftwave_engine::storage::MessageRole::User,
            parts: vec![craftwave_engine::storage::WirePart::Text {
                text: text.to_owned(),
            }],
            created_at: 1,
            run_id: None,
            status: None,
        }
    }

    pub(super) fn run_config() -> craftwave_lib::host::commands::chat::RunConfig {
        run_config_tier(craftwave_engine::safety::tier::PermissionTier::FullAccess)
    }

    fn run_config_tier(
        tier: craftwave_engine::safety::tier::PermissionTier,
    ) -> craftwave_lib::host::commands::chat::RunConfig {
        craftwave_lib::host::commands::chat::RunConfig {
            model_id: None,
            thinking: None,
            tier,
            max_turns: 4,
            steer: false,
        }
    }

    /// 等待 runEnded 帧出现（命令面任务异步产帧；tokio sleep 不阻塞 worker）。
    pub(super) async fn wait_ended(sink: &SharedSink, timeout: Duration) -> ChatFrame {
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

        // 落盘锚：run 全链后磁盘有 canonical 条目（user 兜底 + turn
        // 文本 + marker）与 index.json 建档——「已提交历史零丢失」端到端。
        let tree = app.store.load_session_tree("sess-1").expect("史册应存在");
        let ids: Vec<&str> = tree.messages.iter().map(|m| m.id.as_str()).collect();
        // 默认 mock 脚本 = 工具调用 turn + 文本 turn（tests/common/mock_openai）：共 5 条
        assert_eq!(ids.len(), 5, "实际条目：{ids:?}");
        assert!(ids[0].starts_with("u-"), "user 兜底条目");
        assert!(ids[1].ends_with(":1"), "turn1 文本条目");
        assert!(ids[2].ends_with(":1.tools"), "turn1 工具条目");
        assert!(ids[3].ends_with(":2"), "turn2 文本条目");
        assert!(ids[4].ends_with(":marker"), "runMarker 收口");
        assert_eq!(tree.active_leaf_id.as_deref(), Some(ids[4]));
        let meta = app
            .store
            .index()
            .list()
            .into_iter()
            .find(|m| m.id == "sess-1")
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
        assert_eq!(err.code, "E_SESSION_RUNNING", "同会话互斥");

        // 取消收尾：置位后 run 终结摘除
        assert!(craftwave_lib::host::commands::chat::cancel_run_core(&runs, "sess-2").await);
        let ended = wait_ended(&sink, Duration::from_secs(5)).await;
        let ChatFrame::RunEnded { reason, .. } = &ended else {
            panic!("取消后 runEnded");
        };
        assert_eq!(*reason, EndReason::Cancelled);
    }

    // —— 6/7. 审批门接线 + plan/ask/suggest ——

    use craftwave_engine::backend::ask::AskAnswer;
    use craftwave_engine::util::events::RecordingEventSink;

    /// 等待某事件键的首个载荷。
    pub(super) async fn wait_event(
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
    pub(super) fn tool_turn1(
        text: &str,
        tool: &str,
        args_json: &str,
    ) -> Vec<(&'static str, String)> {
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
            run_config_tier(craftwave_engine::safety::tier::PermissionTier::ConfirmEach),
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
        // 已答 ask 再 resolve → E_NOT_FOUND
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
            run_config_tier(craftwave_engine::safety::tier::PermissionTier::ConfirmEach),
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

        // 拒绝 → 挂起调用补拒绝 toolResult + runEnded(cancelled)
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
            // full_access 也拦（灾难级任何档位不豁免）
            run_config_tier(craftwave_engine::safety::tier::PermissionTier::FullAccess),
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

        // plan:update 事件（items 全量）
        let update = wait_event(&events, "plan:update", Duration::from_secs(5)).await;
        assert_eq!(update["sessionId"], "sess-plan");
        assert_eq!(update["items"].as_array().unwrap().len(), 2);
        assert_eq!(update["items"][0]["status"], "in_progress");

        // run 活跃期间批准 → 快照冻结（approve_plan 命令面，同键
        // 补发 plan:update 全量计划态）
        let events_dyn: std::sync::Arc<dyn craftwave_engine::util::events::EventSink> = events.clone();
        craftwave_lib::host::commands::chat::approve_plan_core(&runs, "sess-plan", &events_dyn)
            .expect("活跃 run 上批准成功");
        // 批准后的 plan:update 全量态（第二条——首条是 plan 工具
        // 执行发的；前端计划卡/Inspector 基线标记的数据面）
        let deadline = Instant::now() + Duration::from_secs(5);
        let approved_update = loop {
            let updates = events.payloads_of("plan:update");
            if updates.len() >= 2 {
                break updates[1].clone();
            }
            assert!(Instant::now() < deadline, "等待批准后的 plan:update 超时");
            tokio::time::sleep(Duration::from_millis(5)).await;
        };
        assert_eq!(approved_update["approved"], true, "批准态随事件带出");
        let baseline = approved_update["baselinePaths"]
            .as_array()
            .expect("基线数组");
        assert_eq!(baseline.len(), 0, "模型未声明 paths 时冻结为空基线");

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
        let err = craftwave_lib::host::commands::chat::approve_plan_core(
            &empty_runs,
            "no-run",
            &events_dyn,
        )
        .unwrap_err();
        assert_eq!(err.code, "E_NOT_FOUND");
    }

    // ---- 用量统计落账与面板数据面 e2e ----

    /// 落账→查询→重启恢复→删除保留→缓存降级 全链：真实 mock SSE run +
    /// suggest 后台小调用后，三面（stats/*.json、usage.json、index.db）读面
    /// 同源一致；重启后四口径恢复；删除会话保留匿名聚合；
    /// 删库后降级直读数字不丢。
    #[tokio::test]
    async fn usage_stats_ledger_e2e_record_recover_delete_degrade() {
        let mock = MockOpenAI::start().await; // 默认脚本：工具轮 + 文本轮
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        std::mem::forget(dir); // 测试进程生命周期内保住数据目录
        let events = Arc::new(RecordingEventSink::new());
        let app = AppState::new_in(root.clone(), events.clone());
        let config = craftwave_engine::providers::types::AppConfig {
            providers: vec![provider_fixture(&mock.base_url)],
            active_model_id: Some("model-1".to_owned()),
            ..craftwave_engine::providers::types::AppConfig::default()
        };
        app.config.save(config).expect("save config");
        let runs = RunManager::new();
        let sink = SharedSink::default();

        // ① 真实 run + suggest（后台小调用）
        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-stats".to_owned(),
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
        wait_event(&events, "run:suggestions", Duration::from_secs(10)).await;

        // ② 日/模型聚合：两拍 run（240/96）+ suggest（120/48）
        //    = 360/144；runs 只计主 run 一次（子 run/辅助不计，suggest 属 aux）
        let today = craftwave_engine::usage::local_today();
        let day = app.ledger.stats.read_day(&today);
        let m1 = day.models.get("model-1").expect("model-1 聚合条目");
        assert_eq!(m1.input, 360, "run 两拍 + suggest 全部计入当日");
        assert_eq!(m1.output, 144);
        assert_eq!(m1.runs, 1, "runs 仅主 run（suggest 辅助不计次）");
        assert_eq!(m1.model_name, "gpt-4o-mock", "显示名快照随落账");

        // ③ run/会话明细：sessionTotal 与聚合同源；main/aux 分明
        let detail = app.ledger.detail.load("sess-stats");
        assert_eq!(detail.session_total.input, 360);
        assert_eq!(detail.session_total.output, 144);
        assert_eq!(detail.runs.len(), 2, "主 run + suggest 辅助条目");
        let kinds: Vec<&str> = detail.runs.values().map(|r| r.kind.as_str()).collect();
        assert!(kinds.contains(&"main"), "主 run 明细（kind=main）");
        assert!(kinds.contains(&"aux"), "suggest 明细（kind=aux）");
        let main_run = detail
            .runs
            .values()
            .find(|r| r.kind == "main")
            .expect("主 run 条目");
        assert!(
            main_run.by_model.contains_key("model-1"),
            "按实际执行模型归属"
        );
        assert!(main_run.ended_at.is_some(), "run 收口补写 endedAt");

        // ④ stats_token 与实时面同源一致（验收：聚合数字与 Inspector
        //    会话累计同源）；tokens:update 带 modelId（前端增量修正依据）
        let stats = craftwave_lib::host::commands::stats::stats_token_core(
            &app,
            &craftwave_lib::host::commands::stats::StatsTokenArgs { range: None },
            &today,
        )
        .expect("stats_token");
        assert_eq!(stats.total.input, 360);
        assert_eq!(stats.total.total, 504);
        assert_eq!(stats.total.runs, 1);
        let by_model = &stats.by_model[0];
        assert_eq!(by_model.model_id, "model-1");
        assert_eq!(by_model.runs, 1);
        let last_tokens = events
            .payloads_of("tokens:update")
            .last()
            .cloned()
            .expect("tokens:update 已发");
        assert_eq!(last_tokens["modelId"], "model-1", "事件带实际执行模型");
        assert_eq!(last_tokens["usage"]["input"], 360);

        // ⑤ 内存读面（活跃会话）
        let live = craftwave_lib::host::commands::stats::session_usage_core(&app, "sess-stats");
        assert_eq!(live.usage.input, 360);

        // ⑥ 重启恢复：新 AppState（同数据目录）从明细文件恢复四口径
        let events2 = Arc::new(RecordingEventSink::new());
        let app2 = AppState::new_in(root.clone(), events2);
        let restored =
            craftwave_lib::host::commands::stats::session_usage_core(&app2, "sess-stats");
        assert_eq!(restored.usage.input, 360, "重启后四口径恢复");
        assert_eq!(restored.usage.output, 144);
        assert_eq!(restored.legacy_total_tokens, None);
        let stats2 = craftwave_lib::host::commands::stats::stats_token_core(
            &app2,
            &craftwave_lib::host::commands::stats::StatsTokenArgs { range: None },
            &today,
        )
        .expect("stats_token 重启后");
        assert_eq!(stats2.total.input, 360, "重启后日聚合数字不丢");

        // ⑦ 删除会话：明细不可查询、日/模型匿名数字保留、缓存重建不复活
        craftwave_lib::host::commands::session::delete_session_core(&app2, &runs, "sess-stats")
            .expect("删除会话");
        assert!(
            !root.join("sessions/sess-stats.usage.json").exists(),
            "明细文件随删除清除"
        );
        let after_delete =
            craftwave_lib::host::commands::stats::session_usage_core(&app2, "sess-stats");
        assert_eq!(after_delete.usage.input, 0, "删除后明细不可查询");
        let stats3 = craftwave_lib::host::commands::stats::stats_token_core(
            &app2,
            &craftwave_lib::host::commands::stats::StatsTokenArgs { range: None },
            &today,
        )
        .expect("删除后 stats_token");
        assert_eq!(stats3.total.input, 360, "匿名聚合数字保留（无会话关联）");

        // ⑧ 删 index.db 后：新进程重建空库 → 查询缺日回填（或降级直读），
        //    面板数字仍正确（验收项；不重复增减）
        std::fs::remove_file(root.join("cache/index.db")).expect("删缓存库");
        let events3 = Arc::new(RecordingEventSink::new());
        let app3 = AppState::new_in(root.clone(), events3);
        let stats4 = craftwave_lib::host::commands::stats::stats_token_core(
            &app3,
            &craftwave_lib::host::commands::stats::StatsTokenArgs { range: None },
            &today,
        )
        .expect("stats_token 降级/回填读面");
        assert_eq!(stats4.total.input, 360, "缓存重建/降级后数字一致");
        assert_eq!(stats4.total.runs, 1, "回填不重复增减 runs");
    }

    // ---- 帧重放与热重连：中途 attach 续流 / 终态空重放 /
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
        assert_eq!(old_sink.frames().len(), old_after, "热切换后旧端断流");
        let new_frames = new_sink.frames();
        assert!(
            new_frames.len() > replayed as usize,
            "新端 = 重放 + 续流增量"
        );
        // 帧序严格递增无重复（重放帧先于新帧）
        for (i, t) in new_frames.iter().enumerate() {
            assert_eq!(t.frame.seq, i as u64, "新端帧序连续：位置 {i}");
        }

        // —— run 结束（收尾摘除）后 attach：空重放 + runId 省略 + 快照 null
        //    （幂等查询语义）；未知 sessionId 同语义 ——
        let deadline = Instant::now() + Duration::from_secs(2);
        while runs.get(session).is_some() {
            assert!(Instant::now() < deadline, "run 应收尾摘除");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let after = craftwave_lib::host::commands::chat::attach_run_core(
            &runs,
            session,
            0,
            Box::new(SharedSink::default()),
        );
        assert_eq!(after.replayed, 0, "无活跃 run → 空重放");
        assert!(after.run_id.is_none(), "runId 省略");
        assert!(
            craftwave_lib::host::commands::chat::session_run_snapshot_core(&runs, session)
                .is_none(),
            "无活跃 run → 快照 null"
        );
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
            run_config_tier(craftwave_engine::safety::tier::PermissionTier::ConfirmEach),
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

        // —— 待审批恢复查询：原始操作及参数随行 ——
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
            craftwave_engine::backend::ask::AskAnswer::Approval {
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
            craftwave_engine::backend::ask::AskAnswer::Approval {
                option_id: "deny".to_owned(),
                edited_command: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_NOT_FOUND", "过期请求操作被拒绝");
    }

    // ---- 上下文压缩：跨阈值自动压缩 / 失败兜底 /
    // 手动命令 / 超预算暂停 ----

    /// 压缩 e2e 的配置面：把激活模型窗口改小让第二条消息必然跨 0.6 阈值
    ///（估算面含 32768 回复预留 > 0.6 × 44000）。窗口取 44000：工具注册表
    /// 每增一工具都会抬高请求固定估算面，40000 的
    /// 硬预算线（×0.9=36000）已被 web_search 挤破（固定面 36058），40000
    /// 失去「第一次发送达阈值但硬预算内可容纳」的设计前提；44000 的硬预算
    /// 39600 恢复该区间，且 32768 预留 > 0.6×44000 的跨阈值前提不变。
    fn small_window_config(base_url: &str) -> craftwave_engine::providers::types::AppConfig {
        craftwave_engine::providers::types::AppConfig {
            providers: vec![craftwave_engine::providers::types::ProviderConfig {
                models: vec![craftwave_engine::providers::types::ProviderModel {
                    context_window: 44000,
                    ..provider_fixture(base_url).models.remove(0)
                }],
                ..provider_fixture(base_url)
            }],
            active_model_id: Some("model-1".to_owned()),
            ..craftwave_engine::providers::types::AppConfig::default()
        }
    }

    /// 断言：长会话跨阈值自动压缩——compactDone 帧 + run:compacted 事件 +
    /// 史册 compaction 条目（原始消息全保留）+ 后续请求 wire 只含 summary
    /// 注入与本次输入（cutEntryId 之前的历史被裁）+ 摘要用量计入会话。
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
        // + 原上下文可容纳 → run 照常完成（失败兜底）
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
        // run 的链尾），start_chat 只对末条 user 做幂等兜底
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
        // 注入次序：ai_language 层在压缩 summary 之前——
        // messages[0] = ai_language 注入行（空偏好跟随界面语言 zh-CN）、
        // messages[1] = 压缩摘要
        assert_eq!(
            messages[0]["role"], "system",
            "ai_language 应注入为 system 前缀首位：{messages:?}"
        );
        assert_eq!(
            messages[0]["content"][0]["text"], "请始终使用 简体中文 回答。",
            "ai_language 空 → 跟随界面语言 zh-CN 固定行"
        );
        assert_eq!(
            messages[1]["content"][0]["text"], "试试让我总结配置要点。",
            "摘要文本 = mock 非流式答复（rig system content 数组形态）"
        );
        let joined = serde_json::to_string(&messages).unwrap();
        assert!(
            !joined.contains(&long_text),
            "被压缩历史不得进入 wire：{joined}"
        );
        assert!(joined.contains("第二条内容消息"), "本次输入保留：{joined}");

        // 计量面：摘要用量计入会话（tokens:update 照发）
        assert!(
            !events.payloads_of("tokens:update").is_empty(),
            "压缩摘要用量应计入会话累计"
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
        let config = craftwave_engine::providers::types::AppConfig {
            providers: vec![craftwave_engine::providers::types::ProviderConfig {
                models: vec![craftwave_engine::providers::types::ProviderModel {
                    context_window: 30000,
                    ..provider_fixture(&mock.base_url).models.remove(0)
                }],
                ..provider_fixture(&mock.base_url)
            }],
            active_model_id: Some("model-1".to_owned()),
            ..craftwave_engine::providers::types::AppConfig::default()
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

    // —— 8. steer / followUp 队列 ——

    use craftwave_lib::host::commands::chat::{InjectMode, inject_run_message_core};
    use craftwave_engine::storage::MessageRole;

    /// run 正常完成后的两连发场景：
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
            &[],
        )
        .expect("空闲入 followUp 应成功");
        assert!(injected.queued);
        assert_eq!(injected.mode, Some(InjectMode::FollowUp));
        assert_eq!(app.followups.snapshot("sess-fu").size, 1);
        // 入队即落盘
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
        let injected = inject_run_message_core(&app, "sess-steer", "顺便看下端口", None, None, &[])
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

    // —— ask 混批互斥（命令面）——

    const ASK_ARGS: &str = r#"{"title":"选方案","questions":[{"id":"q1","text":"用哪个方案？","options":[{"id":"a","label":"方案A"},{"id":"b","label":"方案B"}],"allowFreeText":false}]}"#;

    /// ask 与其他工具同批——只执行 ask，其余调用明确返回未执行
    /// （E_TOOL_SKIPPED）；confirm_each 档下被跳过的命令**不申请审批**
    /// （ask:opened 恰一次且 kind=question）；回答后由模型重新决定。
    #[tokio::test]
    async fn ask_mixed_batch_executes_only_ask_others_reported_skipped() {
        let mock = MockOpenAI::start_with_turn1(multi_tool_turn1(&[
            ("call_1", "ask", ASK_ARGS),
            (
                "call_2",
                TOOL_COMMAND,
                r#"{"command":"echo should-not-run"}"#,
            ),
        ]))
        .await;
        let (app, events) = app_fixture_with_events(&mock);
        let runs = RunManager::new();
        let sink = SharedSink::default();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-askmix".to_owned(),
            vec![user_entry("问用户的同时跑个命令")],
            run_config_tier(craftwave_engine::safety::tier::PermissionTier::ConfirmEach),
            Box::new(sink.clone()),
            false,
        )
        .await
        .unwrap();

        // 只弹问卷（command 不申请审批）：ask:opened 恰一次且 kind=question
        let opened = wait_event(&events, "ask:opened", Duration::from_secs(5)).await;
        assert_eq!(opened["payload"]["kind"], "question");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            events.payloads_of("ask:opened").len(),
            1,
            "被跳过的 command 不申请审批（无 approval 弹窗）"
        );

        let ask_id = opened["askId"].as_str().unwrap().to_owned();
        let mut answers = std::collections::BTreeMap::new();
        answers.insert("q1".to_owned(), "b".to_owned());
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
        // 源序结果：ask 成功 + command 明确未执行（E_TOOL_SKIPPED）
        let faces = tool_result_faces(&sink.frames());
        assert_eq!(faces.len(), 2, "调用/结果配对完整");
        assert_eq!(
            faces[0].0, "call_part_1_1",
            "源序首个 = ask（part 1：开头正文占 part 0）"
        );
        assert!(faces[0].1, "ask 正常执行并拿到回答");
        assert_eq!(faces[1].0, "call_part_1_2", "源序第二 = command");
        assert!(!faces[1].1, "command 未执行");
        assert_eq!(
            faces[1].2.as_deref(),
            Some("E_TOOL_SKIPPED"),
            "未执行结果带 E_TOOL_SKIPPED"
        );
        // 回喂面：第 2 轮请求两条 tool 消息，第 2 条含跳过说明（模型重新决定）
        let bodies = mock.request_bodies().await;
        let tools: Vec<&serde_json::Value> = bodies[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "tool")
            .collect();
        assert_eq!(tools.len(), 2);
        assert!(
            tools[1].to_string().contains("E_TOOL_SKIPPED"),
            "模型通道含未执行说明：{}",
            tools[1]
        );
    }

    // ---- 自动起名全链（start_chat_core 触发判定 → 后台标题任务） ----

    use common::mock_openai::NONSTREAM_TEXT;

    /// 默认标题的 index 条目（模拟 create_session 产物——真实链路前端
    /// 发消息前必先 create_session，index 必有默认标题条目）。
    fn seed_default_title(app: &AppState, session_id: &str) {
        app.store
            .index()
            .upsert(craftwave_engine::storage::SessionMeta {
                id: session_id.to_owned(),
                project_id: None,
                title: "新会话".to_owned(),
                created_at: 1,
                updated_at: 1,
                revision: 0,
                message_count: 0,
                total_tokens: 0,
                authorized_external_dirs: Vec::new(),
            })
            .expect("seed index");
    }

    /// 全链：新会话首条消息 run 完成后自动得名——session:title 事件 +
    /// index/meta 双写（事件时点已落盘）+ 计量（`:title` aux 明细 +
    /// tokens:update 照发）。
    #[tokio::test]
    async fn auto_title_full_chain_first_message() {
        let mock = MockOpenAI::start().await;
        let (app, events) = app_fixture_with_events(&mock);
        seed_default_title(&app, "sess-auto-title");
        let runs = RunManager::new();
        let sink = SharedSink::default();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-auto-title".to_owned(),
            vec![user_entry("帮我读配置文件")],
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

        // 标题 = mock 非流式回复文本（归一后）
        let title = wait_event(&events, "session:title", Duration::from_secs(10)).await;
        assert_eq!(title["sessionId"], "sess-auto-title");
        assert_eq!(title["title"], NONSTREAM_TEXT);

        // 事件顺序锚：事件已到 ⇒ 落盘已完成（index + meta 行双写一致）
        let meta = app
            .store
            .index()
            .get("sess-auto-title")
            .expect("index 应有会话");
        assert_eq!(meta.title, NONSTREAM_TEXT);
        let raw = std::fs::read_to_string(
            app.store
                .root()
                .join("histories")
                .join("sess-auto-title")
                .join("0001.jsonl"),
        )
        .expect("史册文件应存在");
        assert!(
            raw.lines()
                .rev()
                .any(|l| l.contains("\"kind\":\"meta\"") && l.contains(NONSTREAM_TEXT)),
            "meta 行应带新标题"
        );

        // 计量：run 两拍（240/96）+ suggest（120/48）+ 标题（120/48）
        // = 480/192；明细面有 `:title` aux 条目
        wait_event(&events, "run:suggestions", Duration::from_secs(10)).await;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let tokens = events.payloads_of("tokens:update");
            let Some(last) = tokens.last() else {
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            };
            if last["usage"]["input"] == 480 {
                assert_eq!(last["usage"]["output"], 192, "{last}");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "tokens:update 未达 480/192；已收 {tokens:?}；session:title {} 个",
                events.payloads_of("session:title").len()
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let detail = app.ledger.detail.load("sess-auto-title");
        // runId 后缀键 = `{runId}:title`（run-1 为真实 uuid run id）；明细
        // 以 kind=aux + 输入计量存在为判据
        assert!(
            detail.runs.values().any(|r| r.kind == "aux"
                && r.by_model
                    .values()
                    .any(|m| m.input == 120 && m.output == 48)),
            "标题辅助调用应有 aux 明细：{detail:?}"
        );
    }
}

// —— 8b. 灰区命令审计：三态 + 失败转人工 + 恢复不重复审计 ——

mod command_audit {
    use super::command_face::{
        app_fixture_with_events, run_config, tool_turn1, user_entry, wait_ended, wait_event,
    };
    use super::*;
    use serde_json::Value;

    /// 审计灰区命令样例：重定向写目标在授权目录外（词法越界）→ full_access
    /// 档围栏 Confirm 挂起（唯一灰区待决）→ 进审计。
    const AUDIT_GRAY_COMMAND: &str = "echo hi > /tmp/craftwave-audit-e2e.txt";

    /// turn1 脚本：文本 + 灰区命令调用。
    fn audit_turn1() -> Vec<(&'static str, String)> {
        tool_turn1(
            "需要在授权目录外写一个文件。",
            TOOL_COMMAND,
            &format!(r#"{{"command":"{AUDIT_GRAY_COMMAND}"}}"#),
        )
    }

    /// 等待首个指定状态的 toolAudit 帧。
    async fn wait_audit_frame(sink: &SharedSink, state: &str, timeout: Duration) -> ChatFrame {
        let deadline = Instant::now() + timeout;
        loop {
            let hit = sink
                .frames()
                .into_iter()
                .find_map(|t| match &t.frame.frame {
                    ChatFrame::ToolAudit { state: s, .. } if audit_state_key(s) == state => {
                        Some(t.frame.frame.clone())
                    }
                    _ => None,
                });
            if let Some(frame) = hit {
                return frame;
            }
            assert!(Instant::now() < deadline, "等待 toolAudit({state}) 超时");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// AuditState → 线上键（snake_case）。
    fn audit_state_key(state: &craftwave_engine::backend::frames::AuditState) -> &'static str {
        use craftwave_engine::backend::frames::AuditState as S;
        match state {
            S::Running => "running",
            S::Allow => "allow",
            S::Block => "block",
            S::Ask => "ask",
            S::Manual => "manual",
        }
    }

    /// 非流式（审计）请求体清单。
    async fn nonstream_bodies(mock: &MockOpenAI) -> Vec<Value> {
        mock.request_bodies()
            .await
            .into_iter()
            .filter(|b| b["stream"] != Value::Bool(true))
            .collect()
    }

    /// 场景 1：审计 allow —— 低风险灰区自动放行（不弹审批卡），恢复重放
    /// 不重复审计，usage 计入会话（kind=aux）。
    #[tokio::test]
    async fn audit_allow_resumes_without_ask_and_records_usage() {
        let mock = MockOpenAI::start_with_turn1(audit_turn1()).await;
        mock.set_nonstream_text(Some(
            r#"{"decision":"allow","reason":"常规文件创建，风险低"}"#,
        ));
        let (app, events) = app_fixture_with_events(&mock);
        let runs = RunManager::new();
        let sink = SharedSink::default();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-audit-allow".to_owned(),
            vec![user_entry("在临时目录建个文件")],
            run_config(), // full_access：无矩阵待决，围栏 Confirm = 唯一灰区
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("启动成功");

        // 审计帧序：running → allow（含简短理由），随后无审批卡直接恢复执行。
        let running = wait_audit_frame(&sink, "running", Duration::from_secs(10)).await;
        let ChatFrame::ToolAudit { call_id, .. } = &running else {
            unreachable!()
        };
        let allow = wait_audit_frame(&sink, "allow", Duration::from_secs(10)).await;
        let ChatFrame::ToolAudit { reason, .. } = &allow else {
            unreachable!()
        };
        assert_eq!(
            reason.as_deref(),
            Some("常规文件创建，风险低"),
            "结论带简短理由"
        );
        assert!(
            events.payloads_of("ask:opened").is_empty(),
            "审计放行不弹审批卡"
        );

        // 审计模型请求断言：无工具表、单条 user 消息（提示词含待审命令，
        // 不带 turn1 的 assistant 文本——独立提示词不带全历史）。
        let bodies = nonstream_bodies(&mock).await;
        assert_eq!(bodies.len(), 1, "审计只调用一次（恢复重放不重复审计）");
        let body = &bodies[0];
        let tools_empty =
            body.get("tools").is_none() || body["tools"].as_array().is_some_and(|t| t.is_empty());
        assert!(tools_empty, "审计请求不带工具表：{body}");
        let messages = body["messages"].as_array().expect("messages");
        assert_eq!(messages.len(), 1, "审计请求单条消息（不带聊天历史）");
        assert_eq!(messages[0]["role"], "user");
        let prompt = messages[0]["content"].as_str().unwrap_or_default();
        assert!(prompt.contains(AUDIT_GRAY_COMMAND), "提示词含待审命令");
        assert!(
            !prompt.contains("需要在授权目录外写"),
            "提示词不带 assistant 历史"
        );
        assert!(prompt.contains("在临时目录建个文件"), "提示词带用户目标");
        assert_eq!(body["model"], "gpt-4o-mock", "未配审计模型跟随会话模型");

        // run 正常完成（恢复后命令真执行 → turn2 答复）。
        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        let ChatFrame::RunEnded { reason, .. } = &ended else {
            panic!("应为 runEnded");
        };
        assert_eq!(*reason, EndReason::Done, "审计放行后 run 正常完成");
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
        assert!(ok, "放行后命令执行成功");

        // usage 计入会话（审计 120/48 与主 run 叠加）且不增主轮次：
        // tokens:update 事件 ≥2 次（turn1 usage 帧 + 审计补计），末次 modelId
        // = 会话模型；审计帧在 turn1 的 usage 帧之后。
        let updates = events.payloads_of("tokens:update");
        assert!(
            updates.len() >= 2,
            "审计用量应补发 tokens:update：{}",
            updates.len()
        );
        let last = updates.last().expect("有事件");
        assert_eq!(last["modelId"], "model-1", "审计用量按实际执行模型归属");
        let _ = call_id;
    }

    /// 场景 5：审计 Block —— 模型判断高风险 → E_AUDIT_BLOCKED 拒绝收场
    /// （runEnded(error) + run:error 事件带 code），命令不执行。
    #[tokio::test]
    async fn audit_block_rejects_run_ends_error() {
        let mock = MockOpenAI::start_with_turn1(audit_turn1()).await;
        mock.set_nonstream_text(Some(r#"{"decision":"block","reason":"疑似数据外传"}"#));
        let (app, events) = app_fixture_with_events(&mock);
        let runs = RunManager::new();
        let sink = SharedSink::default();

        craftwave_lib::host::commands::chat::start_chat_core(
            &app,
            &runs,
            "sess-audit-block".to_owned(),
            vec![user_entry("审计拒绝场景")],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("启动成功");

        wait_audit_frame(&sink, "block", Duration::from_secs(10)).await;
        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        let ChatFrame::RunEnded { reason, error, .. } = &ended else {
            panic!("应为 runEnded");
        };
        assert_eq!(*reason, EndReason::Error, "审计拒绝按错误终态收场");
        assert_eq!(
            error.as_ref().map(|e| e.code.as_str()),
            Some("E_AUDIT_BLOCKED"),
            "runEnded.error.code = E_AUDIT_BLOCKED"
        );
        // run:error 事件带同一 code（followUp 队列按失败暂停的判定输入）。
        let err_event = wait_event(&events, "run:error", Duration::from_secs(5)).await;
        assert_eq!(err_event["code"], "E_AUDIT_BLOCKED");
        // 命令未执行：toolResult ok=false + 审计拒绝摘要，无二次模型轮次。
        let results: Vec<ChatFrame> = sink
            .frames()
            .iter()
            .map(|t| t.frame.frame.clone())
            .filter(|f| matches!(f, ChatFrame::ToolResult { .. }))
            .collect();
        assert_eq!(results.len(), 1);
        let ChatFrame::ToolResult { ok, ui_summary, .. } = &results[0] else {
            unreachable!()
        };
        assert!(!ok, "审计拒绝的调用不执行");
        assert!(
            ui_summary.contains("审计拒绝"),
            "摘要带审计结论：{ui_summary}"
        );
        assert!(
            events.payloads_of("ask:opened").is_empty(),
            "Block 不弹审批卡"
        );
    }
}

// ---- 图片输入与模型读图（image part 双形态端到端） ----

mod image_face {
    use super::*;
    use base64::Engine;

    use craftwave_lib::host::commands::chat::{RunConfig, start_chat_core};
    use craftwave_engine::providers::types::{ApiFormat, ProviderConfig, ProviderModel};
    use craftwave_engine::safety::tier::PermissionTier;
    use craftwave_engine::storage::{MessageEntry, MessageRole, WirePart, get_blob, put_blob};
    use craftwave_engine::util::events::RecordingEventSink;

    /// 1x1 透明 PNG（base64 内嵌，程序解码——测试样本不依赖外部文件）。
    const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

    fn png_bytes() -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .decode(PNG_1X1)
            .unwrap()
    }

    /// vision 标记可配的 provider（与 command_face::provider_fixture 同构）。
    fn provider_fixture_vision(base_url: &str, vision: Option<bool>) -> ProviderConfig {
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
                vision,
            }],
            headers: None,
            local: false,
        }
    }

    fn app_fixture_vision(mock: &MockOpenAI, vision: Option<bool>) -> AppState {
        let dir = tempfile::tempdir().expect("tempdir");
        let events = Arc::new(RecordingEventSink::new());
        let app = AppState::new_in(dir.path().to_path_buf(), events);
        let config = craftwave_engine::providers::types::AppConfig {
            providers: vec![provider_fixture_vision(&mock.base_url, vision)],
            active_model_id: Some("model-1".to_owned()),
            ..craftwave_engine::providers::types::AppConfig::default()
        };
        app.config.save(config).expect("save config");
        std::mem::forget(dir);
        app
    }

    fn run_config() -> RunConfig {
        RunConfig {
            model_id: None,
            thinking: None,
            tier: PermissionTier::FullAccess,
            max_turns: 4,
            steer: false,
        }
    }

    /// 带图 user 消息（text + image 双 part）。
    fn user_with_image(id: &str, text: &str, blob_hash: &str) -> MessageEntry {
        MessageEntry {
            id: id.to_owned(),
            parent_id: None,
            role: MessageRole::User,
            parts: vec![
                WirePart::Text {
                    text: text.to_owned(),
                },
                WirePart::Image {
                    blob_hash: blob_hash.to_owned(),
                    mime_type: "image/png".to_owned(),
                },
            ],
            created_at: 1,
            run_id: None,
            status: None,
        }
    }

    /// 等待 runEnded 帧出现（命令面任务异步产帧）。
    pub(super) async fn wait_ended(sink: &SharedSink, timeout: Duration) -> ChatFrame {
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

    /// 收集全部请求体里 user 消息的 content 数组（跨请求平铺）。
    async fn user_contents(mock: &MockOpenAI) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        for body in mock.request_bodies().await {
            for message in body["messages"].as_array().unwrap_or(&Vec::new()) {
                if message["role"] == "user" {
                    out.push(message["content"].clone());
                }
            }
        }
        out
    }

    /// vision 模型全链：blob 落盘 → 带图 start_chat → mock 请求体收多模态块
    ///（image_url，data URL 承载 base64 字节）；纯文本部分照常。
    #[tokio::test]
    async fn vision_model_consumes_image_as_multimodal_block() {
        let mock = MockOpenAI::start().await;
        let app = app_fixture_vision(&mock, Some(true));
        let runs = RunManager::new();
        let sink = SharedSink::default();

        // blob 先落盘（前端 attach_image 的等价动作）
        let blob_hash = put_blob(app.store.root(), "sess-v", &png_bytes()).unwrap();
        let (stored, mime) = get_blob(app.store.root(), "sess-v", &blob_hash).unwrap();
        assert_eq!(mime, "image/png");
        assert_eq!(stored, png_bytes());

        let result = start_chat_core(
            &app,
            &runs,
            "sess-v".to_owned(),
            vec![user_with_image("u1", "看这张图", &blob_hash)],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("vision 模型带图 start_chat 应成功");
        assert!(!result.run_id.is_empty());
        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        let ChatFrame::RunEnded { reason, error, .. } = &ended else {
            panic!("应为 runEnded");
        };
        assert_eq!(*reason, EndReason::Done, "error 面：{error:?}");

        // mock 请求体：prompt 的 content 数组含 image_url 多模态块
        //（rig openai wire 把 base64 图片编为 data URL）
        let contents = user_contents(&mock).await;
        let with_image = contents
            .iter()
            .find(|c| {
                c.as_array()
                    .is_some_and(|parts| parts.iter().any(|p| p["type"] == "image_url"))
            })
            .expect("vision 请求应携带 image_url 多模态块");
        let image_part = with_image
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["type"] == "image_url")
            .unwrap();
        let url = image_part["image_url"]["url"].as_str().unwrap();
        assert!(
            url.starts_with("data:image/png;base64,"),
            "图片应为 data URL 形态：{url}"
        );
        let expected_data_url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png_bytes())
        );
        assert_eq!(url, expected_data_url, "base64 字节应与 blob 一致");
        // 文本块同在（text + image 共存）
        assert!(
            with_image
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["type"] == "text"),
            "文本块应与图片块共存"
        );
    }

    /// 非 vision 模型：本次新附图（prompt 带图）命令面拒绝（E_INVALID_ARG，
    /// 零 run）；纯文字继续不受历史图片影响——历史图在请求中转占位文本。
    #[tokio::test]
    async fn non_vision_rejects_new_image_and_degrades_history_to_placeholder() {
        let mock = MockOpenAI::start().await;
        let app = app_fixture_vision(&mock, Some(false));
        let runs = RunManager::new();
        let blob_hash = put_blob(app.store.root(), "sess-n", &png_bytes()).unwrap();

        // ① 新附图拒绝（零副作用——mock 无请求到达）
        let sink = SharedSink::default();
        let err = start_chat_core(
            &app,
            &runs,
            "sess-n".to_owned(),
            vec![user_with_image("u1", "看这张图", &blob_hash)],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARG");
        assert!(err.message.contains("不支持图片输入"));
        assert!(mock.request_bodies().await.is_empty(), "拒绝路径零模型请求");

        // ② 历史带图 + 纯文字 prompt：照常运行，历史图在请求中是占位文本
        let history_img = user_with_image("u1", "看这张图", &blob_hash);
        let prompt = MessageEntry {
            id: "u2".to_owned(),
            parent_id: Some("u1".to_owned()),
            role: MessageRole::User,
            parts: vec![WirePart::Text {
                text: "改用文字描述".to_owned(),
            }],
            created_at: 2,
            run_id: None,
            status: None,
        };
        let sink = SharedSink::default();
        start_chat_core(
            &app,
            &runs,
            "sess-n".to_owned(),
            vec![history_img, prompt],
            run_config(),
            Box::new(sink.clone()),
            false,
        )
        .await
        .expect("历史带图不阻塞纯文字继续");
        let ended = wait_ended(&sink, Duration::from_secs(10)).await;
        let ChatFrame::RunEnded { reason, error, .. } = &ended else {
            panic!("应为 runEnded");
        };
        assert_eq!(*reason, EndReason::Done, "error 面：{error:?}");
        // 占位文本面（逐字格式）出现在请求 history
        let placeholder = craftwave_engine::backend::wire::image_placeholder(&blob_hash);
        let contents = user_contents(&mock).await;
        assert!(
            contents
                .iter()
                .any(|c| c
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(|p| p["type"] == "text"
                        && p["text"]
                            .as_str()
                            .is_some_and(|t| t.contains("当前模型无法查看此图片"))))),
            "历史图应降级为占位文本（{placeholder}）；实际：{contents:?}"
        );
        // 占位不携带图片字节
        assert!(
            !contents.iter().any(|c| c
                .as_array()
                .is_some_and(|parts| parts.iter().any(|p| p["type"] == "image_url"))),
            "非 vision 请求不应出现多模态图片块"
        );
    }
}

// —— 8. 计划范围门链路 ——

/// 范围门直驱全链：已批准计划（基线 src/a.rs）下模型写范围外路径 →
/// RunExit::Suspended 载荷带 plan_scope（run_loop 集成面：GateVerdict::
/// Suspend.plan_scope → PendingCall.plan_scope）→ 宿主语义恢复（签名登记
/// + 恢复循环同款）→ 重放放行跑到 Done。
#[tokio::test]
async fn e2e_plan_scope_suspend_resume_allow_once() {
    use craftwave_engine::backend::gate::{ApprovalGate, GateWorkspace};
    use craftwave_engine::tools::plan::PlanState;
    use std::sync::{Arc, Mutex};

    // mock：轮 1 = write 调用（范围外）；轮 2（带 tool_result）默认文本。
    let write_script = vec![
        content_delta("写个文件"),
        tool_open(0, "call_w1", "write"),
        tool_args_delta(0, r#"{"path":"src/b.rs","content":"fn b() {}\n"}"#),
        finish_chunk("tool_calls"),
        usage_event(),
        done_marker(),
    ];
    let mock = MockOpenAI::start_with_turn1(write_script).await;

    // 已批准计划：基线 src/a.rs（条目 in_progress，不过期门）
    let mut plan_state = PlanState {
        items: vec![craftwave_engine::storage::entries::PlanItem {
            id: "p1".to_owned(),
            text: "写 a".to_owned(),
            status: craftwave_engine::storage::entries::PlanItemStatus::InProgress,
        }],
        paths: vec!["src/a.rs".to_owned()],
        ..PlanState::default()
    };
    plan_state.approve(1000);
    let plan: Arc<Mutex<Option<PlanState>>> = Arc::new(Mutex::new(Some(plan_state)));

    // 临时项目目录 + 范围门（full_access：矩阵放行，隔离范围门行为）
    let dir = tempfile::tempdir().expect("临时目录");
    let project_root = dir.path().join("project");
    std::fs::create_dir_all(&project_root).expect("建项目根");
    let project = std::fs::canonicalize(&project_root).expect("canonicalize");
    let gate = ApprovalGate::new(
        craftwave_engine::safety::tier::PermissionTier::FullAccess,
        plan,
    )
    .with_workspace(GateWorkspace {
        roots: vec![project.clone()],
        base: project.clone(),
    });

    let driver = TurnDriver::new(mock_model(&mock), RunSpec::new(), vec![tool_def("write")]);
    let gate = Arc::new(gate);
    let gate_arc: Arc<dyn ToolGate> = gate.clone();
    let sink = SharedSink::default();
    let mut sink_handle = sink.clone();
    let exit = drive_run(
        &driver,
        new_run("写范围外的文件"),
        "run-plan-scope",
        "gpt-4o-mock",
        CancellationToken::new(),
        fake_executor_text("写入成功"),
        gate_arc.clone(),
        &mut sink_handle,
        None,
        None,
        None,
        std::time::Duration::from_secs(30),
    )
    .await
    .expect("挂起路径正常返回");
    let RunExit::Suspended(info) = exit else {
        panic!("范围外写应挂起：{exit:?}");
    };
    let craftwave_engine::backend::run_loop::SuspendedInfo {
        run_json,
        pending,
        state,
    } = *info;
    assert_eq!(pending.name, "write");
    assert_eq!(
        pending.plan_scope.as_deref(),
        Some("src/b.rs"),
        "挂起载荷带计划外路径"
    );
    assert_eq!(state.tool_count, 0, "挂起时批内零执行");

    // 恢复循环的 allow_once 语义：签名登记（approve）后原样重放
    gate.approve(&pending.name, &pending.args_json);
    let _ = &project;
    let resumed = resume_run(&run_json).expect("反序列化恢复");
    let sink2 = SharedSink::default();
    let mut sink2_handle = sink2.clone();
    let exit = drive_run(
        &driver,
        resumed,
        "run-plan-scope",
        "gpt-4o-mock",
        CancellationToken::new(),
        fake_executor_text("写入成功"),
        gate_arc.clone(),
        &mut sink2_handle,
        Some(state),
        None,
        None,
        std::time::Duration::from_secs(30),
    )
    .await
    .expect("恢复路径跑到完成");
    assert!(matches!(exit, RunExit::Done(_)), "恢复后 Done：{exit:?}");
    assert_eq!(
        sink2.count_kind(|f| matches!(f, ChatFrame::ToolResult { ok: true, .. })),
        1,
        "重放放行并执行"
    );
    // 范围门不因 allow_once 扩基线：同 run 内再写 src/c.rs 仍会挂起
    //（由 gate 判定面保证——gate_test 覆盖，此处签名面验证）
    let verdict = gate.judge(
        "write",
        &serde_json::json!({"path": "src/c.rs", "content": "x"}).to_string(),
    );
    let GateVerdict::Suspend { plan_scope, .. } = &verdict else {
        panic!("allow_once 不扩大基线：{verdict:?}")
    };
    assert_eq!(plan_scope.as_deref(), Some("src/c.rs"));
}
