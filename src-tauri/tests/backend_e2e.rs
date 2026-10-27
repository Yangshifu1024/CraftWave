//! backend 产品化链路端到端（mock OpenAI 兼容 SSE，离线确定性）：
//!
//! 1. 帧保真与 callId 同源：文本/思考/工具参数流 → RunLoop 生产路径（64ms
//!    节流内聚）→ 帧序列（runStarted/stepStarted/delta/toolStarted/toolArgs/
//!    toolResult/usage/runEnded），帧面三帧 callId 同源、seq 严格递增、
//!    usage 累计、NFR-02（TTFT <100ms）与节流间隔断言；
//! 2. 取消全链：流中取消（<1s，runEnded(cancelled) 收齐后最后一帧）；
//! 3. 取消收齐语义：工具批内已完成保留、未完成补 E_CANCELLED；
//! 4. 审批挂起机制层：gate=Suspend 序列化挂起 → resume_run → 跑到 Done；
//! 5. start_chat_core 命令面：模型解析（config+keyring 注入）、同会话互斥
//!    E_SESSION_RUNNING、RunManager 取消。

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::mock_openai::{
    MockOpenAI, TURN1_REASONING, TURN1_TEXT, TURN1_TOOL_ARG_PARTS, TURN2_TEXT, content_delta,
    done_marker, finish_chunk, tool_args_delta, tool_open, usage_event,
};
use craftwave_lib::agent::RunManager;
use craftwave_lib::backend::frames::{ChatFrame, EndReason, FrameSink, SequencedFrame};
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

/// 共享收集 sink（出口填 seq；时序单独记录）。
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
async fn e2e_帧保真与call_id同源_节流与nfr_02() {
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
async fn e2e_流中取消_run_ended_cancelled最后一帧_全链时延() {
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
struct ReadThenCommandExecutor {
    spawned: tokio::sync::mpsc::UnboundedSender<i32>,
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
                let _ = self.spawned.send(grandchild);
                let _ = call_id;
                let outcome = tokio::select! {
                    biased;
                    _ = token.cancelled() => None,
                    status = child.wait() => Some(status.expect("wait")),
                };
                match outcome {
                    None => {
                        let status = craftwave_lib::tools::process::reap_process_group(
                            &mut child,
                            craftwave_lib::tools::process::REAP_GRACE,
                        )
                        .await
                        .expect("进程组收割");
                        ToolExecution::Cancelled {
                            executed: true,
                            message: format!("进程组已收割（sh 退出 {status:?}，孙进程随组终止）"),
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

#[cfg(unix)]
#[tokio::test]
async fn e2e_工具批取消_已完成保留_未完成补cancelled标记() {
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
    let (spawned_tx, mut spawned_rx) = tokio::sync::mpsc::unbounded_channel::<i32>();

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
        id1, "call_part_0",
        "帧面 callId = 桥接派生 id（与流式帧同源）"
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
        id2, "call_part_1",
        "帧面 callId = 桥接派生 id（与流式帧同源）"
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

    // 整树收割：孙进程消亡
    craftwave_lib::tools::process::pid_alive(grandchild);
    let deadline = Instant::now() + Duration::from_secs(2);
    while craftwave_lib::tools::process::pid_alive(grandchild) {
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
            GateVerdict::Suspend
        }
    }
}

#[tokio::test]
async fn e2e_审批挂起机制_序列化挂起_恢复跑到done() {
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
    )
    .await
    .expect("挂起路径应正常返回");
    let RunExit::Suspended { run_json } = exit else {
        panic!("应为 Suspended，实际 {exit:?}");
    };
    // toolResult 未发（挂起在执行前）、runEnded 未发
    assert_eq!(
        sink1.count_kind(|f| matches!(f, ChatFrame::ToolResult { .. })),
        0
    );
    assert_eq!(
        sink1.count_kind(|f| matches!(f, ChatFrame::RunEnded { .. })),
        0
    );

    // 第二段：反序列化恢复 → 全放行 → 跑到 Done
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
    )
    .await
    .expect("恢复路径应跑到完成");
    assert!(
        matches!(exit, RunExit::Done(_)),
        "恢复后跑到 Done：{exit:?}"
    );
    assert_eq!(
        sink2.count_kind(|f| matches!(f, ChatFrame::ToolResult { .. })),
        1
    );
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

    use craftwave_lib::providers::keyring::{MemoryKeyring, credential_account};
    use craftwave_lib::providers::types::{ApiFormat, ProviderConfig, ProviderModel};
    use craftwave_lib::storage::config_store::ConfigStore;

    fn provider_fixture(base_url: &str) -> ProviderConfig {
        ProviderConfig {
            id: "prov-1".to_owned(),
            name: "Mock 供应商".to_owned(),
            api_format: ApiFormat::OpenaiChat,
            base_url: Some(base_url.to_owned()),
            credential_ref: Some(credential_account("prov-1")),
            credential_count: 1,
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
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("config.json");
        let intent_path = dir.path().join("intent.json");
        let store: ConfigStore<craftwave_lib::providers::types::AppConfig> =
            ConfigStore::new(config_path, intent_path);
        let config = craftwave_lib::providers::types::AppConfig {
            providers: vec![provider_fixture(&mock.base_url)],
            active_model_id: Some("model-1".to_owned()),
            ..craftwave_lib::providers::types::AppConfig::default()
        };
        store.save(config).expect("save config");
        // tempdir drop 会删目录——用 leak 保住（测试进程生命周期内可接受）
        std::mem::forget(dir);
        AppState {
            config: store,
            keys: Arc::new(MemoryKeyring::new()),
            runs: RunManager::new(),
        }
        .with_key("mock-key")
    }

    trait WithKey {
        fn with_key(self, key: &str) -> AppState;
    }

    impl WithKey for AppState {
        fn with_key(self, key: &str) -> AppState {
            let provider = self.config.load().unwrap().providers[0].clone();
            let reference = provider.credential_ref.clone().unwrap();
            self.keys.set_secret(&reference, key).unwrap();
            self
        }
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
        craftwave_lib::host::commands::chat::RunConfig {
            model_id: None,
            tier: craftwave_lib::safety::tier::PermissionTier::FullAccess,
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
    async fn start_chat_core_端到端出帧_模型解析走config与keyring() {
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

    #[tokio::test]
    async fn 同会话第二个start被拒绝_会话运行中错误() {
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
}
