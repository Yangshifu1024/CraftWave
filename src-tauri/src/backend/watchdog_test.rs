//! 监督看门狗测试（37 号票，§5.7）：阈值上下界、纠偏与终止次数、重置条件、
//! 容量滑动、停滞计时与 drive_run 全链终态（ScriptedBackend = rig 剧本假模型
//! + 假执行器直驱，不依赖真实 300 秒等待——超时以毫秒级小值注入）。

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures::StreamExt;

use rig::completion::Message;
use rig::completion::request::{CompletionRequest, ToolDefinition};
use rig::driver::{Exchange, Model, Opened, Opening, Transport};
use rig::message::ToolName;
use rig::run::AgentRun;
use rig::run::spec::RunSpec;
use rig::test_utils::{MockCompletionModel, MockScript, MockStreamEvent};
use tokio_util::sync::CancellationToken;

use crate::backend::frames::{ChatFrame, CollectingSink, EndReason, FrameError};
use crate::backend::run_loop::SteerSource;
use crate::backend::run_loop::{
    AllowAllGate, CancellableExecutor, RunExit, ToolExecution, drive_run,
};
use crate::backend::turn::TurnDriver;
use crate::backend::watchdog::{
    NO_PROGRESS_CORRECT, NO_PROGRESS_TERMINATE, READONLY_REPEAT_CORRECT, RunWatchdog,
    SAME_SIGNATURE_FAIL_CORRECT, SAME_SIGNATURE_FAIL_TERMINATE, SEGMENTED_READ_CORRECT, StallGuard,
    WINDOW_CAPACITY, WatchdogAction, is_model_delta, make_record, tool_signature,
};

// ---- 便捷构造 ----

/// 失败记录（make_record 走真实注册表能力分级——grep 是只读工具）。
fn fail(name: &str, args: &str) -> super::ToolRecord {
    make_record(name, args, false)
}

/// 成功记录。
fn ok(name: &str, args: &str) -> super::ToolRecord {
    make_record(name, args, true)
}

/// §5.7 阈值表钉死（常量防漂移）。
#[test]
fn thresholds_match_spec_table() {
    assert_eq!(SAME_SIGNATURE_FAIL_CORRECT, 3);
    assert_eq!(SAME_SIGNATURE_FAIL_TERMINATE, 5);
    assert_eq!(NO_PROGRESS_CORRECT, 8);
    assert_eq!(NO_PROGRESS_TERMINATE, 14);
    assert_eq!(READONLY_REPEAT_CORRECT, 4);
    assert_eq!(SEGMENTED_READ_CORRECT, 10);
    assert_eq!(WINDOW_CAPACITY, 32);
}

/// 信号①阈值上下界：2 次无动作、3 次纠偏、4 次不重复纠偏、5 次终止。
#[test]
fn same_signature_fail_2_none_3_corrective_5_terminate() {
    let args = r#"{"pattern":"x","path":"a"}"#;
    let mut w = RunWatchdog::default();
    w.record_step([fail("grep", args)], false);
    assert_eq!(w.evaluate(), WatchdogAction::None);
    w.record_step([fail("grep", args)], false);
    assert_eq!(w.evaluate(), WatchdogAction::None);
    // 第 3 次：纠偏
    w.record_step([fail("grep", args)], false);
    assert!(matches!(w.evaluate(), WatchdogAction::Corrective(_)));
    // 第 4 次：段内已纠偏，不重复
    w.record_step([fail("grep", args)], false);
    assert_eq!(w.evaluate(), WatchdogAction::None);
    // 第 5 次：终止
    w.record_step([fail("grep", args)], false);
    let action = w.evaluate();
    assert!(
        matches!(action, WatchdogAction::Terminate(_)),
        "第 5 次应终止：{action:?}"
    );
    if let WatchdogAction::Terminate(message) = action {
        assert!(
            message.contains("连续失败"),
            "终止消息说明重复失败原因：{message}"
        );
    }
}

/// 信号①重置条件：同签名成功打断连续段——重开后重新累计，满 3 再纠偏。
#[test]
fn same_signature_fail_resets_on_success() {
    let args = r#"{"pattern":"x","path":"a"}"#;
    let mut w = RunWatchdog::default();
    w.record_step([fail("grep", args)], false);
    w.record_step([fail("grep", args)], false);
    assert_eq!(w.evaluate(), WatchdogAction::None);
    // 同签名成功：连续段打断
    w.record_step([ok("grep", args)], false);
    assert_eq!(w.evaluate(), WatchdogAction::None);
    // 重开后累计 2 次：不纠偏
    w.record_step([fail("grep", args)], false);
    w.record_step([fail("grep", args)], false);
    assert_eq!(w.evaluate(), WatchdogAction::None);
    // 第 3 次（新段）：再纠偏
    w.record_step([fail("grep", args)], false);
    assert!(matches!(w.evaluate(), WatchdogAction::Corrective(_)));
}

/// 信号①连续段语义：不同签名调用打断——每段各自累计，互不叠加。
#[test]
fn different_signature_interrupts_fail_run() {
    let a = r#"{"pattern":"x","path":"a"}"#;
    let b = r#"{"pattern":"y","path":"a"}"#;
    let mut w = RunWatchdog::default();
    w.record_step([fail("grep", a)], false);
    w.record_step([fail("grep", a)], false);
    w.record_step([fail("grep", b)], false);
    w.record_step([fail("grep", a)], false);
    w.record_step([fail("grep", a)], false);
    // A 段 2 + B 段 1 + A 段 2：任何一段都不满 3，无动作
    assert_eq!(w.evaluate(), WatchdogAction::None);
}

/// 信号②阈值上下界：7 步无动作、8 步纠偏、继续空转不重复纠偏、14 步终止。
/// 每步失败用不同参数（签名互异）——隔离无进展信号，不与同签名失败信号耦合。
#[test]
fn no_progress_thresholds() {
    let mut w = RunWatchdog::default();
    // 空转步 = 无正文 + 全部失败（或无工具）
    for step in 1..=7 {
        let args = format!(r#"{{"pattern":"x{step}","path":"a"}}"#);
        w.record_step([fail("grep", &args)], false);
        assert_eq!(w.evaluate(), WatchdogAction::None, "第 {step} 步应无动作");
    }
    // 第 8 步：纠偏
    w.record_step([fail("grep", r#"{"pattern":"x8","path":"a"}"#)], false);
    assert!(matches!(w.evaluate(), WatchdogAction::Corrective(_)));
    // 9–13 步：不再重复纠偏（无工具的空转步）
    for step in 9..14 {
        w.record_step(std::iter::empty(), false);
        assert_eq!(
            w.evaluate(),
            WatchdogAction::None,
            "第 {step} 步不应重复纠偏"
        );
    }
    // 第 14 步：终止
    w.record_step(std::iter::empty(), false);
    assert!(matches!(w.evaluate(), WatchdogAction::Terminate(_)));
}

/// 信号②重置：有文本的步（或工具成功的步）清零计数，重转空转后满 8 再纠偏。
#[test]
fn no_progress_resets_on_productive_step() {
    let mut w = RunWatchdog::default();
    for step in 0..7 {
        let args = format!(r#"{{"pattern":"x{step}","path":"a"}}"#);
        w.record_step([fail("grep", &args)], false);
        assert_eq!(w.evaluate(), WatchdogAction::None);
    }
    // 有进展步（模型输出了正文）：清零并复位纠偏水位
    w.record_step([fail("grep", r#"{"pattern":"zz","path":"a"}"#)], true);
    assert_eq!(w.evaluate(), WatchdogAction::None);
    // 重新累计 7 步仍无动作（空转步与轮换参数失败交替）
    for step in 0..7 {
        let args = format!(r#"{{"pattern":"y{step}","path":"a"}}"#);
        w.record_step([fail("grep", &args)], false);
        assert_eq!(w.evaluate(), WatchdogAction::None);
    }
    // 第 8 步：纠偏（水位已复位）
    w.record_step(std::iter::empty(), false);
    assert!(matches!(w.evaluate(), WatchdogAction::Corrective(_)));
}

/// 信号③阈值：同参数只读成功重复 3 次无动作、4 次纠偏、不重复、8 次按级再纠偏。
#[test]
fn readonly_repeat_thresholds() {
    let args = r#"{"path":"a.txt"}"#;
    let mut w = RunWatchdog::default();
    for _ in 0..3 {
        w.record_step([ok("read", args)], true);
        assert_eq!(w.evaluate(), WatchdogAction::None);
    }
    // 第 4 次：纠偏
    w.record_step([ok("read", args)], true);
    assert!(matches!(w.evaluate(), WatchdogAction::Corrective(_)));
    // 5–7 次：不重复纠偏
    for _ in 5..8 {
        w.record_step([ok("read", args)], true);
        assert_eq!(w.evaluate(), WatchdogAction::None);
    }
    // 第 8 次：level=2 > 1，按级再纠偏
    w.record_step([ok("read", args)], true);
    assert!(matches!(w.evaluate(), WatchdogAction::Corrective(_)));
}

/// 信号③只对只读工具生效：非只读工具同参数成功重复不触发。
#[test]
fn readonly_repeat_ignores_non_readonly_tools() {
    let args = r#"{"path":"a.txt","content":"x"}"#;
    let mut w = RunWatchdog::default();
    for _ in 0..6 {
        w.record_step([ok("write", args)], true);
        assert_eq!(w.evaluate(), WatchdogAction::None);
    }
}

/// 信号④阈值：同文件连续分段读 9 次无动作、10 次纠偏；读别的文件打断重开。
#[test]
fn segmented_read_thresholds_and_reset() {
    let mut w = RunWatchdog::default();
    for offset in 1..=9u64 {
        let args = format!(r#"{{"path":"big.txt","offset":{offset}}}"#);
        w.record_step([ok("read", &args)], true);
        assert_eq!(
            w.evaluate(),
            WatchdogAction::None,
            "第 {offset} 次分段读应无动作"
        );
    }
    // 第 10 次：纠偏（提示避免碎片化读取）
    w.record_step([ok("read", r#"{"path":"big.txt","offset":10}"#)], true);
    let action = w.evaluate();
    assert!(matches!(action, WatchdogAction::Corrective(_)));
    if let WatchdogAction::Corrective(text) = action {
        assert!(text.contains("big.txt"), "纠偏提示带文件路径：{text}");
    }
    // 读别的文件：段打断——重开累计
    w.record_step([ok("read", r#"{"path":"other.txt"}"#)], true);
    for offset in 11..=19u64 {
        let args = format!(r#"{{"path":"big.txt","offset":{offset}}}"#);
        w.record_step([ok("read", &args)], true);
        assert_eq!(
            w.evaluate(),
            WatchdogAction::None,
            "重开后第 {} 次应无动作",
            offset - 10
        );
    }
}

/// 窗口容量 32：满后丢最旧——只读重复计数随滑动下降，不再重复纠偏。
/// 填充用不同签名的成功记录（隔离滑动语义，不触发其他信号）。
#[test]
fn window_capacity_slides_and_reduces_count() {
    let target = r#"{"path":"t.txt"}"#;
    let mut w = RunWatchdog::default();
    // 同签名成功 ×4：纠偏一次（count=4，level=1）
    for _ in 0..4 {
        w.record_step([ok("read", target)], true);
    }
    assert!(matches!(w.evaluate(), WatchdogAction::Corrective(_)));
    // 用 30 条不同签名的成功记录挤掉 target 的最旧 2 条（4 + 30 = 32 满 + 溢出 2 条）
    for i in 0..30usize {
        let args = format!(r#"{{"path":"f{i}.txt"}}"#);
        w.record_step([ok("grep", &args)], true);
    }
    // target 记录只剩 2 条：count=2 不再触发
    assert_eq!(w.evaluate(), WatchdogAction::None);
    // 再成功 ×2：count 回到 4，level=1 不大于已纠偏水位 1——不重复纠偏
    w.record_step([ok("read", target)], true);
    w.record_step([ok("read", target)], true);
    assert_eq!(w.evaluate(), WatchdogAction::None);
}

/// 签名稳定性：同工具同参数同签名；参数不同签名不同；工具名不同签名不同。
#[test]
fn signature_stability() {
    let a = tool_signature("grep", r#"{"pattern":"x"}"#);
    let a_again = tool_signature("grep", r#"{"pattern":"x"}"#);
    let b = tool_signature("grep", r#"{"pattern":"y"}"#);
    let c = tool_signature("read", r#"{"pattern":"x"}"#);
    assert_eq!(a, a_again);
    assert_ne!(a, b);
    assert_ne!(a, c);
}

/// make_record 的能力分级与 read 路径提取：read/grep/list_files 只读；
/// write/command 非只读；read 的 path 字段提取（畸形 JSON 返回 None）。
#[test]
fn make_record_classifies_readonly_and_read_path() {
    let read = make_record("read", r#"{"path":"src/main.rs","offset":5}"#, true);
    assert!(read.read_only);
    assert_eq!(read.read_path.as_deref(), Some("src/main.rs"));

    assert!(make_record("grep", "{}", true).read_only);
    assert!(make_record("list_files", "{}", true).read_only);
    assert!(!make_record("write", "{}", true).read_only);
    assert!(!make_record("command", "{}", true).read_only);

    // 非 read 工具不提路径
    assert_eq!(make_record("grep", r#"{"path":"x"}"#, true).read_path, None);
    // 畸形 JSON：路径为 None（不是分段读信号的数据源）
    assert_eq!(make_record("read", "{oops", true).read_path, None);
}

/// 有效模型增量判定：三类增量帧刷新；其余帧（含 UI 面/工具进度/重连摘要）
/// 不刷新——前端帧与重连不能给停滞计时续命。
#[test]
fn is_model_delta_only_matches_three_delta_frames() {
    assert!(is_model_delta(&ChatFrame::TextDelta {
        turn: 1,
        delta: "a".into()
    }));
    assert!(is_model_delta(&ChatFrame::ReasoningDelta {
        turn: 1,
        delta: "r".into()
    }));
    assert!(is_model_delta(&ChatFrame::ToolArgs {
        call_id: "c".into(),
        chunk: "{}".into()
    }));
    assert!(!is_model_delta(&ChatFrame::RunStarted {
        run_id: "r".into(),
        model: "m".into()
    }));
    assert!(!is_model_delta(&ChatFrame::StepStarted { turn: 1 }));
    assert!(!is_model_delta(&ChatFrame::ToolStarted {
        call_id: "c".into(),
        name: "grep".into()
    }));
    assert!(!is_model_delta(&ChatFrame::ToolResult {
        call_id: "c".into(),
        ok: true,
        ui_summary: "s".into(),
        error: None
    }));
    assert!(!is_model_delta(&ChatFrame::Usage {
        usage: Default::default(),
        rate: None
    }));
    assert!(!is_model_delta(&ChatFrame::RunEnded {
        reason: EndReason::Done,
        duration_ms: 0,
        tool_count: 0,
        turn: 1,
        error: None
    }));
    assert!(!is_model_delta(&ChatFrame::CompactDone {
        cut_entry_id: "e".into(),
        tokens_before: 0,
        tokens_after: 0
    }));
}

// ---- 停滞计时（tokio 定时器 + 毫秒级注入超时，不依赖真实 300 秒）----

/// 无增量：停滞截止按注入超时触发。
#[tokio::test]
async fn stall_guard_fires_without_refresh() {
    let guard = StallGuard::armed(Duration::from_millis(40));
    let started = std::time::Instant::now();
    guard.expired().await;
    assert!(
        started.elapsed() >= Duration::from_millis(35),
        "应等待注入超时后才触发，实际 {:?}",
        started.elapsed()
    );
}

/// 有效增量刷新：周期性刷新顺延截止，停止刷新后才触发。
#[tokio::test]
async fn stall_guard_refresh_extends_deadline() {
    let guard = Arc::new(StallGuard::armed(Duration::from_millis(50)));
    let refresher = Arc::clone(&guard);
    // 每 20ms 刷新一次 ×5（共 100ms）：截止被顺延 5 次
    tokio::spawn(async move {
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            refresher.refresh();
        }
    });
    let started = std::time::Instant::now();
    guard.expired().await;
    assert!(
        started.elapsed() >= Duration::from_millis(140),
        "持续刷新期间不应触发（最后一次刷新 100ms + 超时 50ms），实际 {:?}",
        started.elapsed()
    );
}

// ---- drive_run 全链（ScriptedBackend：rig 剧本假模型 + 假执行器）----

/// 恒失败的假执行器：记录执行次数（副作用不重放的断言面）。
struct FailingExecutor {
    executed: AtomicUsize,
}

impl CancellableExecutor for FailingExecutor {
    fn execute<'a>(
        &'a self,
        _call_id: &'a str,
        _name: &'a str,
        _args_json: &'a str,
        _token: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>> {
        self.executed.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            ToolExecution::Completed {
                ok: false,
                ui_summary: "测试失败".to_owned(),
                error: Some(FrameError::new("E_TEST", "假执行器恒失败")),
                model_text: "E_TEST 假执行器恒失败".to_owned(),
            }
        })
    }
}

/// 慢执行器：执行耗时超过停滞超时（工具执行等待不计入停滞计时的断言面）。
struct SlowExecutor {
    delay: Duration,
    executed: AtomicUsize,
}

impl CancellableExecutor for SlowExecutor {
    fn execute<'a>(
        &'a self,
        _call_id: &'a str,
        _name: &'a str,
        _args_json: &'a str,
        _token: &'a CancellationToken,
    ) -> Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>> {
        self.executed.fetch_add(1, Ordering::SeqCst);
        let delay = self.delay;
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            ToolExecution::Completed {
                ok: true,
                ui_summary: "慢但成功".to_owned(),
                error: None,
                model_text: "ok".to_owned(),
            }
        })
    }
}

/// 剧本流一轮：一个完整工具调用事件 + 收尾（usage 缺省）。
fn tool_call_turn(id: &str, args: serde_json::Value) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::tool_call(id, "grep", args),
        MockStreamEvent::final_response_with_default_usage(),
    ]
}

/// 文本收尾一轮：一段正文 + 收尾。
fn text_turn(text: &str) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::text(text),
        MockStreamEvent::final_response_with_default_usage(),
    ]
}

/// drive_run 装配：剧本模型 + 假执行器 + 全放行门，直驱到终态。
async fn run_scripted<W, T>(
    driver: &TurnDriver<W, T>,
    executor: Arc<dyn CancellableExecutor>,
    sink: &mut CollectingSink,
    stall_timeout: Duration,
) -> Result<RunExit, rig::run::PromptError>
where
    W: rig::wire::Wire<Op = rig::operation::Completion>,
    T: rig::driver::Transport<W>,
{
    let run = AgentRun::from_spec(
        &RunSpec {
            max_turns: Some(16),
            ..RunSpec::new()
        },
        "触发看门狗",
        Some(Vec::new()),
    );
    drive_run(
        driver,
        run,
        "run-test",
        "scripted-model",
        CancellationToken::new(),
        executor,
        Arc::new(AllowAllGate),
        sink,
        None,
        None::<&dyn SteerSource>,
        None::<&dyn crate::backend::run_loop::RequestRefresher>,
        stall_timeout,
    )
    .await
}

/// 剧本模型的工具表：一个 grep 定义（AgentRun 校验模型调用必须在本拍
/// advertised tools 内——空表会 UnknownToolCall 拒绝回喂）。
fn grep_tool_table() -> Vec<Arc<ToolDefinition>> {
    vec![Arc::new(ToolDefinition::new(
        ToolName::new("grep").expect("合法工具名"),
        "grep 搜索（剧本测试）".to_owned(),
        serde_json::json!({ "type": "object" }),
    ))]
}

/// 剧本假模型（每轮一个完整工具调用事件 + 收尾）。克隆共享剧本状态——
/// 驱动后用 `model.requests()` 取回捕获的请求（纠偏注入断言面）。
fn scripted_model(turns: Vec<Vec<MockStreamEvent>>) -> MockCompletionModel {
    MockCompletionModel::from_stream_turns(turns)
}

/// sink 里 runEnded 帧的收集（终态唯一性断言面）。
fn ended_frames(sink: &CollectingSink) -> Vec<(EndReason, Option<FrameError>)> {
    sink.frames
        .iter()
        .filter_map(|f| match &f.frame {
            ChatFrame::RunEnded { reason, error, .. } => Some((*reason, error.clone())),
            _ => None,
        })
        .collect()
}

/// 同签名连续失败：第 3 次纠偏注入 hidden system 提示（第 4 拍请求携带），
/// 第 5 次终止——E_RUN_STALLED 收尾唯一、结果保留、副作用不重放。
#[tokio::test]
async fn drive_run_repeated_failures_corrective_then_terminate() {
    let args = serde_json::json!({"pattern": "x", "path": "a"});
    // 6 轮剧本（第 6 轮不应被消费——第 5 次失败即终止）
    let model = scripted_model(
        (1..=6)
            .map(|i| tool_call_turn(&format!("call-{i}"), args.clone()))
            .collect(),
    );
    let driver = TurnDriver::new(model.clone(), RunSpec::new(), grep_tool_table());
    let executor = Arc::new(FailingExecutor {
        executed: AtomicUsize::new(0),
    });
    let mut sink = CollectingSink::default();
    let exit = run_scripted(
        &driver,
        executor.clone(),
        &mut sink,
        Duration::from_secs(30),
    )
    .await
    .expect("看门狗终止是 Ok 退出态");

    // 终态：Stalled（message 说明重复失败）
    let RunExit::Stalled { message } = &exit else {
        panic!("应为看门狗终止，实际 {exit:?}");
    };
    assert!(message.contains("连续失败"), "终止消息说明原因：{message}");

    // runEnded(error, E_RUN_STALLED) 恰好一条（终态唯一）
    let ended = ended_frames(&sink);
    assert_eq!(ended.len(), 1, "终态帧唯一：{ended:?}");
    let (reason, error) = &ended[0];
    assert_eq!(*reason, EndReason::Error);
    let error = error.as_ref().expect("错误终态带 error 面");
    assert_eq!(error.code, "E_RUN_STALLED");

    // 结果保留：5 次失败的 toolResult 帧都在（不撤回）
    let tool_results = sink
        .frames
        .iter()
        .filter(|f| matches!(f.frame, ChatFrame::ToolResult { .. }))
        .count();
    assert_eq!(tool_results, 5, "第 5 次失败触发终止，前 5 批结果帧保留");

    // 副作用不重放：恰好执行 5 次（第 6 轮模型调用不发生）
    assert_eq!(executor.executed.load(Ordering::SeqCst), 5);

    // 纠偏注入：模型共收到 5 拍请求；纠偏在第 3 次失败评估（turn 3 收口）
    // 产生、第 4 拍（下标 3）注入一次——段内已纠偏不重复，其余拍不带。
    let requests = model.requests();
    assert_eq!(requests.len(), 5, "5 拍模型请求");
    for (idx, request) in requests.iter().enumerate() {
        let has_corrective = request.chat_history.iter().any(
            |m| matches!(m.as_ref(), Message::System { content } if content.contains("watchdog")),
        );
        if idx == 3 {
            assert!(has_corrective, "第 4 拍请求应携带纠偏提示");
        } else {
            assert!(!has_corrective, "第 {} 拍请求不应携带纠偏提示", idx + 1);
        }
    }
}

/// 悬空流（请求发出后一直无增量）：停滞超时触发 E_RUN_STALLED，runEnded
/// 唯一终态，无工具执行。
#[tokio::test]
async fn drive_run_stall_on_silent_stream() {
    let driver = TurnDriver::new(
        Model::new(MockScript::default(), HangingTransport::hanging(Vec::new())),
        RunSpec {
            max_turns: Some(16),
            ..RunSpec::new()
        },
        grep_tool_table(),
    );
    let executor = Arc::new(FailingExecutor {
        executed: AtomicUsize::new(0),
    });
    let mut sink = CollectingSink::default();
    let exit = run_scripted(
        &driver,
        executor.clone(),
        &mut sink,
        Duration::from_millis(60),
    )
    .await
    .expect("停滞终止是 Ok 退出态");
    assert!(
        matches!(exit, RunExit::Stalled { .. }),
        "悬空流应停滞终止：{exit:?}"
    );

    let ended = ended_frames(&sink);
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].0, EndReason::Error);
    assert_eq!(ended[0].1.as_ref().expect("error 面").code, "E_RUN_STALLED");
    // 无工具执行（停滞发生在模型请求阶段）
    assert_eq!(executor.executed.load(Ordering::SeqCst), 0);
}

/// 有效增量刷新：慢速增量流（间隔 20ms 短于超时 80ms）持续喂增量 → 不误杀，
/// 正常完成。
#[tokio::test]
async fn drive_run_delta_refresh_prevents_false_stall() {
    let mut events: Vec<MockStreamEvent> = Vec::new();
    for _ in 0..10 {
        events.push(MockStreamEvent::text("x"));
    }
    let driver = TurnDriver::new(
        Model::new(
            MockScript::default(),
            HangingTransport::finishing(events, Some(20)),
        ),
        RunSpec {
            max_turns: Some(16),
            ..RunSpec::new()
        },
        grep_tool_table(),
    );
    let executor = Arc::new(FailingExecutor {
        executed: AtomicUsize::new(0),
    });
    let mut sink = CollectingSink::default();
    let exit = run_scripted(&driver, executor, &mut sink, Duration::from_millis(80))
        .await
        .expect("正常完成");
    assert!(
        matches!(exit, RunExit::Done(_)),
        "持续增量不应误杀：{exit:?}"
    );
    let ended = ended_frames(&sink);
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].0, EndReason::Done);
}

/// 增量停发后停滞：先来一个增量（刷新计时），随后悬空 → 超时终止；
/// 首个增量已产出的帧保留（结果不撤回）。
#[tokio::test]
async fn drive_run_stall_after_delta_stops() {
    let driver = TurnDriver::new(
        Model::new(
            MockScript::default(),
            HangingTransport::hanging(vec![MockStreamEvent::text("先来一个增量")]),
        ),
        RunSpec {
            max_turns: Some(16),
            ..RunSpec::new()
        },
        grep_tool_table(),
    );
    let executor = Arc::new(FailingExecutor {
        executed: AtomicUsize::new(0),
    });
    let mut sink = CollectingSink::default();
    let exit = run_scripted(&driver, executor, &mut sink, Duration::from_millis(60))
        .await
        .expect("停滞终止");
    assert!(
        matches!(exit, RunExit::Stalled { .. }),
        "增量停发后应停滞：{exit:?}"
    );
    assert!(
        sink.frames
            .iter()
            .any(|f| matches!(&f.frame, ChatFrame::TextDelta { .. })),
        "停滞前已产出的增量保留"
    );
}

/// 工具执行等待不计入停滞计时：执行耗时（120ms）超过停滞超时（40ms）但
/// 不触发停滞——慢工具正常完成、run 正常收尾。
#[tokio::test]
async fn drive_run_tool_wait_not_counted_as_stall() {
    let driver = TurnDriver::new(
        scripted_model(vec![
            tool_call_turn("call-1", serde_json::json!({"pattern": "x", "path": "a"})),
            text_turn("完成"),
        ]),
        RunSpec {
            max_turns: Some(16),
            ..RunSpec::new()
        },
        grep_tool_table(),
    );
    let executor = Arc::new(SlowExecutor {
        delay: Duration::from_millis(120),
        executed: AtomicUsize::new(0),
    });
    let mut sink = CollectingSink::default();
    let exit = run_scripted(
        &driver,
        executor.clone(),
        &mut sink,
        Duration::from_millis(40),
    )
    .await
    .expect("正常完成");
    assert!(
        matches!(exit, RunExit::Done(_)),
        "工具执行等待不应误触发停滞：{exit:?}"
    );
    assert_eq!(executor.executed.load(Ordering::SeqCst), 1);
    let ended = ended_frames(&sink);
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].0, EndReason::Done);
}

/// 审批等待不误杀由结构保证（挂起时 drive_run 已退出、计时器随请求生灭）；
/// 队列收尾动作纯函数钉住 E_RUN_STALLED 的失败暂停归属。
#[test]
fn stalled_exit_maps_to_queue_pause() {
    use crate::host::commands::chat::{QueueDisposition, RunFailure, followup_disposition};
    let stalled = RunExit::Stalled {
        message: "停滞".to_owned(),
    };
    let failure = Some(RunFailure {
        code: "E_RUN_STALLED".to_owned(),
        message: "停滞".to_owned(),
    });
    assert_eq!(
        followup_disposition(&stalled, &failure),
        QueueDisposition::PauseKeep,
        "E_RUN_STALLED 收尾队列按失败暂停"
    );
    // 对照：取消终态清空返还
    assert_eq!(
        followup_disposition(&RunExit::Cancelled, &None),
        QueueDisposition::ClearReturn
    );
}

// ---- 悬空/慢速流 transport（停滞与持续刷新场景的假模型侧）----

/// MockScript wire 的帧类型（transport 关联面）。
type MockFrame = <MockScript as rig::wire::Wire>::Frame;

/// 悬空/慢速流 transport（stagnation 场景的假模型侧）：
///
/// - [`Self::hanging`]：前缀事件立即发出，之后**永不产出**——"请求发出后
///   一直无增量"（停滞触发面）；
/// - [`Self::finishing`]：前缀事件按可选间隔逐个发出（gap 即慢速增量——
///   有效增量刷新面），之后发收尾事件正常结束（finish 折叠）。
///
/// 前缀耗尽后悬空的实现 = `stream::pending()`（永 Pending，在飞请求由
/// 停滞分支丢弃流 future 即关闭）。
#[derive(Clone)]
struct HangingTransport {
    /// 前缀事件（立即或按间隔发出）。
    prefix: Vec<MockStreamEvent>,
    /// 每个事件发出前的等待毫秒数（None = 立即）。
    gap_ms: Option<u64>,
    /// 前缀耗尽后是否正常收尾（true = 发收尾事件并结束；false = 悬空）。
    finish: bool,
}

impl HangingTransport {
    /// 悬空流：前缀（可空）发完即永 Pending。
    fn hanging(prefix: Vec<MockStreamEvent>) -> Self {
        Self {
            prefix,
            gap_ms: None,
            finish: false,
        }
    }

    /// 慢速增量流：前缀按间隔发出后正常收尾。
    fn finishing(prefix: Vec<MockStreamEvent>, gap_ms: Option<u64>) -> Self {
        Self {
            prefix,
            gap_ms,
            finish: true,
        }
    }
}

impl Transport<MockScript> for HangingTransport {
    fn send(&self, _payload: CompletionRequest, _exchange: Exchange) -> Opening<MockFrame> {
        let head: Vec<Result<MockFrame, rig::error::ProviderError>> = self
            .prefix
            .iter()
            .cloned()
            .map(MockFrame::Event)
            .map(Ok)
            .collect();
        let gap = self.gap_ms;
        // 按可选间隔逐个发出前缀（`stream::iter().then()` 把间隔放 await——
        // 模拟真实流的增量节奏；None 间隔则立即产出）
        let paced = futures::stream::iter(head).then(move |frame| async move {
            if let Some(g) = gap {
                tokio::time::sleep(Duration::from_millis(g)).await;
            }
            frame
        });
        if self.finish {
            // 收尾事件（usage 缺省）+ 流结束（Streamed::finish 折叠出响应）
            let tail = futures::stream::iter([Ok(MockFrame::Event(
                MockStreamEvent::final_response_with_default_usage(),
            ))]);
            Opening::ready(Opened::new(paced.chain(tail)))
        } else {
            // 悬空：前缀后接永 Pending 流（在飞请求由停滞/取消分支丢弃）
            Opening::ready(Opened::new(paced.chain(futures::stream::pending())))
        }
    }
}

/// 未直接使用但保留完整导入面的占位（防 unused import 漂移的显式位置）。
#[allow(dead_code)]
type UnusedAliases = MockCompletionModel;
