//! 【S3 Spike】手动工具循环集成测试：
//! 「单 turn 返回工具调用 → 外部执行 → 结果回喂 → 下一 turn」，
//! 以及 AgentRun 序列化挂起/恢复（审批挂起需要的外部循环形态）。

mod common;

use std::sync::Arc;

use common::mock_openai::{MockOpenAI, TURN1_TOOL_ARG_PARTS, TURN2_TEXT};
use common::{FakeExecutor, TOOL_READ, TimedSink, turn_driver};
use craftwave_lib::spike::frames::ChatFrame;
use craftwave_lib::spike::frames::FrameSink;
use craftwave_lib::spike::turn::ToolExecutor;
use craftwave_lib::spike::turn::drive_manual_tool_loop;
use rig::run::{AgentRun, AgentRunStep};

#[tokio::test]
async fn s3_manual_loop_external_exec_result_feedback() {
    let mock = MockOpenAI::start().await;
    let mut sink = TimedSink::default();
    let response = drive_manual_tool_loop(
        &turn_driver(&mock),
        common::new_run(),
        Arc::new(FakeExecutor),
        &mut |frame| FrameSink::send(&mut sink, frame),
    )
    .await
    .expect("手动循环应正常完成");

    // —— 循环两轮：第 1 轮返回工具调用，第 2 轮在工具结果回喂后给出答复 ——
    assert_eq!(response.output(), TURN2_TEXT, "第 2 轮答复");
    assert_eq!(
        response.completion_calls.len(),
        2,
        "恰好两次模型调用（单 turn 两次）"
    );

    let bodies = mock.request_bodies().await;
    assert_eq!(bodies.len(), 2, "mock 收到两轮补全请求");
    // 第 2 轮请求带回工具结果消息（回喂实证）
    let second = &bodies[1];
    let tool_msgs: Vec<&serde_json::Value> = second["messages"]
        .as_array()
        .expect("messages 数组")
        .iter()
        .filter(|m| m["role"] == "tool")
        .collect();
    assert_eq!(tool_msgs.len(), 1, "第 2 轮恰带一条工具结果消息");
    let tool_text = tool_msgs[0]["content"].as_str().unwrap_or_default();
    assert!(tool_text.contains("port = 8087"), "工具输出原文进入回喂");
    // 执行器收到的是装配器解析后的参数（serde 紧凑串），与模型三段流式片段语义一致
    let expected_args = serde_json::json!({"path": "config.toml", "offset": 0}).to_string();
    assert!(
        tool_text.contains(&expected_args),
        "模型流式给出的完整参数（含半截片段拼合）经装配解析后传给执行器并回喂：{tool_text}"
    );

    // —— 帧序列：toolStarted → toolArgs* → toolResult ——
    let frames = sink.0.lock().unwrap().clone();
    let kinds: Vec<String> = frames
        .iter()
        .map(|t| {
            serde_json::to_value(&t.frame.frame).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    let started = kinds
        .iter()
        .position(|k| k == "toolStarted")
        .expect("有 toolStarted");
    let first_args = kinds
        .iter()
        .position(|k| k == "toolArgs")
        .expect("有 toolArgs");
    let last_args = kinds.iter().rposition(|k| k == "toolArgs").unwrap();
    let result = kinds
        .iter()
        .position(|k| k == "toolResult")
        .expect("有 toolResult");
    assert!(started < first_args && first_args <= last_args && last_args < result);
    // 装配器产出与帧一致的调用 id
    let started_frame = &frames[started].frame.frame;
    let ChatFrame::ToolStarted { name, .. } = started_frame else {
        unreachable!()
    };
    assert_eq!(name, TOOL_READ);

    // —— usage 记账：两轮 completion call 均带 mock 的 usage ——
    let total: u64 = response
        .completion_calls
        .iter()
        .map(|c| c.usage.total_tokens.unwrap_or(0))
        .sum();
    assert_eq!(
        total,
        2 * 168,
        "两轮 usage 均为 mock 报的 168 tok（记账无丢失）"
    );
}

/// 审批挂起形态：CallTools 步骤时把 run 序列化挂起（模拟等待用户审批/进程重启），
/// 反序列化恢复后继续 answer —— rig 的 sans-IO AgentRun 天然支持外部循环。
#[tokio::test]
async fn s3_approval_suspend_run_serialized_resume() {
    let mock = MockOpenAI::start().await;
    let driver = turn_driver(&mock);
    let mut run = common::new_run();
    let mut sink = TimedSink::default();
    let mut on_frame = |frame| FrameSink::send(&mut sink, frame);

    // turn 1：流式调用，模型返回工具调用
    let AgentRunStep::CallModel {
        prompt,
        history,
        turn,
    } = run.next_step().expect("第一步")
    else {
        panic!("期望 CallModel");
    };
    assert_eq!(turn, 1);
    driver
        .stream_turn(&mut run, prompt, history, 1, &mut on_frame)
        .await
        .expect("turn 1 流式完成");

    // 待执行的工具调用出现（外部循环挂起点：审批面板在此弹出）
    let AgentRunStep::CallTools { calls } = run.next_step().expect("第二步") else {
        panic!("期望 CallTools（审批挂起点）");
    };
    assert_eq!(calls.len(), 1, "单工具调用");
    let pending_call_id = match &calls[0] {
        rig::run::PendingToolCall::Execute(exec) => exec.id().to_string(),
        other => panic!("期望可执行调用，得到 {other:?}"),
    };

    // —— 挂起：序列化整个 run（含待执行调用）——
    let suspended = serde_json::to_string(&run).expect("run 可序列化（挂起）");
    assert!(suspended.contains(TOOL_READ), "挂起快照里有待执行调用");

    // —— 恢复：反序列化后继续（模拟用户批准后的进程内/跨重启恢复）——
    let mut run: AgentRun = serde_json::from_str(&suspended).expect("run 可反序列化（恢复）");
    let output = FakeExecutor.execute(TOOL_READ, &TURN1_TOOL_ARG_PARTS.concat());
    //（FakeExecutor::execute 经 ToolExecutor trait，需引入）
    // 恢复后的待执行调用按 id 应答
    let AgentRunStep::CallTools { calls } = run.next_step().expect("恢复后仍处 CallTools")
    else {
        panic!("恢复后期望 CallTools");
    };
    assert_eq!(
        match &calls[0] {
            rig::run::PendingToolCall::Execute(exec) => exec.id().to_string(),
            other => panic!("期望可执行调用，得到 {other:?}"),
        },
        pending_call_id
    );
    let answers = calls.into_iter().map(|call| match call {
        rig::run::PendingToolCall::Execute(exec) => exec.answer(rig::tool::ToolResult::success(
            rig::tool::ToolOutput::text(output.clone()),
        )),
        other => panic!("期望可执行调用，得到 {other:?}"),
    });
    run.answer_all(answers).expect("工具结果回喂");

    // turn 2：模型在收到工具结果后给出最终答复
    let AgentRunStep::CallModel {
        prompt,
        history,
        turn,
    } = run.next_step().expect("第三步")
    else {
        panic!("期望第 2 次 CallModel");
    };
    assert_eq!(turn, 2);
    driver
        .stream_turn(&mut run, prompt, history, 2, &mut on_frame)
        .await
        .expect("turn 2 流式完成");

    let response = match run.next_step().expect("终态") {
        AgentRunStep::Done(response) => response,
        other => panic!("期望 Done，得到 {other:?}"),
    };
    assert_eq!(response.output(), TURN2_TEXT);

    // 恢复路径与直通路径等价：mock 同样收到带回喂的第 2 轮请求
    let bodies = mock.request_bodies().await;
    assert_eq!(bodies.len(), 2);
    let has_tool_msg = bodies[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool");
    assert!(has_tool_msg, "挂起-恢复后的回喂与直通路径一致");
}
