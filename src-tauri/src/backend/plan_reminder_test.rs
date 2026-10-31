//! 计划提醒测试（25 号票，FR-06-04/§8.6）：N=8 阈值、plan:update 清零、
//! 开关面与跨挂起延续——剧本假模型直驱 `drive_run` 全链（37 号票直驱
//! 基建同款），不依赖真实计时。

use std::sync::Arc;
use std::time::Duration;

use rig::completion::Message;
use rig::completion::request::{CompletionRequest, ToolDefinition};
use rig::message::ToolName;
use rig::run::AgentRun;
use rig::run::spec::RunSpec;
use rig::test_utils::{MockCompletionModel, MockStreamEvent};
use tokio_util::sync::CancellationToken;

use crate::backend::frames::CollectingSink;
use crate::backend::run_loop::{
    AllowAllGate, CancellableExecutor, DriveState, PLAN_REMINDER_EVERY, RunExit, ToolExecution,
    drive_run,
};
use crate::backend::turn::TurnDriver;

/// 提醒文本的判别子串（与 §5.7 看门狗纠偏文本区分）。
const REMINDER_MARK: &str = "计划纪律提醒";

/// 剧本流一轮：一个 command 工具调用（变更工具，FileWrite/Network 能力）
/// + 收尾。args 在批内无重（看门狗同签名失败计数不受影响——工具恒成功）。
fn command_turn(i: usize) -> Vec<MockStreamEvent> {
    let args = serde_json::json!({"command": format!("echo step-{i}")});
    vec![
        MockStreamEvent::tool_call(format!("call-{i}"), "command", args),
        MockStreamEvent::final_response_with_default_usage(),
    ]
}

/// 剧本流一轮：plan 工具成功更新（计数清零的触发面）。
fn plan_turn(i: usize) -> Vec<MockStreamEvent> {
    let args = serde_json::json!({
        "action": "update",
        "items": [{"id": "p1", "text": "任务", "status": "in_progress"}]
    });
    vec![
        MockStreamEvent::tool_call(format!("call-plan-{i}"), "plan", args),
        MockStreamEvent::final_response_with_default_usage(),
    ]
}

/// 文本收尾一轮（自然终态 Done）。
fn text_turn() -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::text("完成"),
        MockStreamEvent::final_response_with_default_usage(),
    ]
}

/// 剧本模型的工具表：command + plan 定义（AgentRun 校验模型调用必须在
/// 本拍 advertised tools 内）。
fn tool_table() -> Vec<Arc<ToolDefinition>> {
    let command = ToolDefinition::new(
        ToolName::new("command").expect("合法工具名"),
        "执行 shell 命令（剧本测试）".to_owned(),
        serde_json::json!({ "type": "object" }),
    );
    let plan = ToolDefinition::new(
        ToolName::new("plan").expect("合法工具名"),
        "维护任务计划（剧本测试）".to_owned(),
        serde_json::json!({ "type": "object" }),
    );
    vec![Arc::new(command), Arc::new(plan)]
}

/// 恒成功假执行器（工具结果 ok=true；plan 更新在直驱面不经执行器真实
/// 状态——计数判定只看"plan 工具调用成功完成"）。
struct OkExecutor;

impl CancellableExecutor for OkExecutor {
    fn execute<'a>(
        &'a self,
        _call_id: &'a str,
        _name: &'a str,
        _args_json: &'a str,
        _token: &'a tokio_util::sync::CancellationToken,
    ) -> std::pin::Pin<Box<dyn Future<Output = ToolExecution> + Send + 'a>> {
        Box::pin(async {
            ToolExecution::Completed {
                ok: true,
                ui_summary: "成功".to_owned(),
                error: None,
                model_text: "ok".to_owned(),
            }
        })
    }
}

/// 直驱装配：剧本模型 + 恒成功执行器 + 全放行门，初始 DriveState 由用例
/// 给（提醒开关/计数起点），驱动到终态。
async fn drive_with(
    turns: Vec<Vec<MockStreamEvent>>,
    initial: DriveState,
) -> (CollectingSink, Vec<CompletionRequest>) {
    let model = MockCompletionModel::from_stream_turns(turns);
    let driver = TurnDriver::new(model.clone(), RunSpec::new(), tool_table());
    let mut sink = CollectingSink::default();
    let run = AgentRun::from_spec(
        &RunSpec {
            max_turns: Some(24),
            ..RunSpec::new()
        },
        "开始",
        Some(Vec::new()),
    );
    let exit = drive_run(
        &driver,
        run,
        "run-reminder",
        "scripted-model",
        CancellationToken::new(),
        Arc::new(OkExecutor),
        Arc::new(AllowAllGate),
        &mut sink,
        Some(initial),
        None::<&dyn crate::backend::run_loop::SteerSource>,
        None::<&dyn crate::backend::run_loop::RequestRefresher>,
        Duration::from_secs(30),
    )
    .await
    .expect("自然终态为 Ok");
    assert!(
        matches!(exit, RunExit::Done(_)),
        "用例均应自然终态：{exit:?}"
    );
    (sink, model.requests())
}

/// 请求 history 末尾的 system 消息文本（注入断言面：hidden system 提示都
/// 追加在末尾——取末条消息若为 system 则返回其文本，否则空，避免把更早
/// 的系统前缀误判为本拍注入）。
fn trailing_systems(request: &CompletionRequest) -> Vec<String> {
    match request.chat_history.last().map(|m| m.as_ref()) {
        Some(Message::System { content }) => vec![content.clone()],
        _ => Vec::new(),
    }
}

/// 逐拍断言提醒携带面（场景共用）。
fn assert_reminder_only_on(requests: &[CompletionRequest], only_at: Option<usize>, why: &str) {
    for (idx, request) in requests.iter().enumerate() {
        let has_reminder = trailing_systems(request)
            .iter()
            .any(|t| t.contains(REMINDER_MARK));
        match only_at {
            Some(at) if idx == at => {
                assert!(has_reminder, "第 {} 拍应携带提醒（{why}）", idx + 1);
            }
            _ => {
                assert!(!has_reminder, "第 {} 拍不应携带提醒（{why}）", idx + 1);
            }
        }
    }
}

/// 全链场景合并（25 号票，FR-06-04/§8.6）：阈值常量钉死（附录 B N=8）；
/// 场景 A——连续 8 次 command 后第 9 拍请求携带一次性提醒、注入后清零
/// （第 10 拍不再携带）；场景 B——plan 成功更新清零后计数不达阈值全程
/// 不注入；场景 C——开关关闭（子代理/缺省装配）全程不计数不注入；场景
/// D——初始计数 7 经 resume 链传入（DriveState 挂起恢复延续，宿主恢复
/// 循环把 SuspendedInfo.state 原样传回 drive_run），1 次变更即达阈值。
#[tokio::test]
async fn plan_reminder_chain_cases() {
    assert_eq!(PLAN_REMINDER_EVERY, 8, "附录 B：N=8 阈值钉死");

    // 场景 A：9 轮 command + 1 轮文本 = 10 拍请求；第 9 拍（8 次变更后）
    // 携带一次性提醒，注入后计数清零
    let mut turns: Vec<Vec<MockStreamEvent>> = (1..=9).map(command_turn).collect();
    turns.push(text_turn());
    let (_sink, requests) = drive_with(
        turns,
        DriveState {
            plan_reminder_enabled: true,
            ..DriveState::default()
        },
    )
    .await;
    assert_eq!(requests.len(), 10, "10 拍模型请求");
    assert_reminder_only_on(&requests, Some(8), "第 9 拍一次性注入 + 清零后不再携带");

    // 场景 B：7 次 command + 1 次 plan 成功——plan 批清零后计数不达阈值，
    // 第 9 拍不携带提醒（无清零时第 8 批恰达 8 会注入）
    let mut turns: Vec<Vec<MockStreamEvent>> = (1..=7).map(command_turn).collect();
    turns.push(plan_turn(1));
    turns.push(text_turn());
    let (_sink, requests) = drive_with(
        turns,
        DriveState {
            plan_reminder_enabled: true,
            ..DriveState::default()
        },
    )
    .await;
    assert_eq!(requests.len(), 9, "9 拍模型请求");
    assert_reminder_only_on(&requests, None, "plan 更新清零后不达阈值");

    // 场景 C：同场景 A 剧本但开关关闭——全程不计数不注入（"根会话"口径）
    let mut turns: Vec<Vec<MockStreamEvent>> = (1..=9).map(command_turn).collect();
    turns.push(text_turn());
    let (_sink, requests) = drive_with(turns, DriveState::default()).await;
    assert_reminder_only_on(&requests, None, "开关关闭不计数");

    // 场景 D：初始计数 7 跨挂起延续——1 次变更即达阈值，第 2 拍请求携带提醒
    let turns = vec![command_turn(1), text_turn()];
    let (_sink, requests) = drive_with(
        turns,
        DriveState {
            plan_reminder_enabled: true,
            plan_change_count: PLAN_REMINDER_EVERY - 1,
            ..DriveState::default()
        },
    )
    .await;
    assert_eq!(requests.len(), 2);
    assert_reminder_only_on(&requests, Some(1), "延续计数 7 + 1 次变更达阈值");
}
