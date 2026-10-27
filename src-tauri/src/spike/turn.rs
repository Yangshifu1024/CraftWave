//! 【Spike】单 turn 流式调用驱动 + `AgentRun` 手动工具循环（S2/S3 共用链路）。
//!
//! 架构对齐技术设计文档 §4.3/§5.2：backend 每次只跑一个 model turn（流式，
//! 事件边到边转帧），工具循环由外部 RunLoop 驱动——审批挂起需要外部循环。
//! rig 侧对应「AgentRun（sans-IO 状态机）+ StreamedTurnAssembler 装配」。

use std::sync::Arc;

use futures::StreamExt;
use rig::completion::Message;
use rig::completion::request::{CompletionRequest, ToolDefinition};
use rig::driver::Model;
use rig::providers::openai::wire::Chat;
use rig::run::prepare::{PreparedRequest, prepare_request};
use rig::run::spec::RunSpec;
use rig::run::streamed::StreamedTurnAssembler;
use rig::run::{AgentRun, AgentRunStep, ModelTurnOutcome, PromptError};
use rig::streaming::Item;
use tokio_util::sync::CancellationToken;

use super::frames::ChatFrame;
use super::rig_bridge::{Bridged, RigEventBridge};

/// 单 turn 流式调用的退出态：自然收尾，或取消中途 drop 流（S5 取消传播）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamExit {
    Completed,
    Cancelled,
}

/// 外部工具执行器（对应 RunLoop 的 §5.4 门控与执行；spike 里由测试注入）。
pub trait ToolExecutor: Send + Sync {
    /// 返回工具输出的文本回喂（生产为双通道结果 + 边车落盘）。
    fn execute(&self, name: &str, args_json: &str) -> String;
}

/// 单 turn 流式调用的驱动：S2 的事件帧化 + S3 的装配回喂。
pub struct TurnDriver {
    model: Model<Chat>,
    spec: RunSpec,
    tools: Vec<Arc<ToolDefinition>>,
}

impl TurnDriver {
    pub fn new(model: Model<Chat>, spec: RunSpec, tools: Vec<Arc<ToolDefinition>>) -> Self {
        Self { model, spec, tools }
    }

    /// 跑一个 CallModel 步骤：流式请求 rig，事件经 `on_frame` 逐个出帧
    /// （测试里直接收集；生产里先进 64ms 节流器再进 Channel），完成后把
    /// 装配好的 streamed turn 回喂 run。不可取消版（S2/S3 路径）。
    pub async fn stream_turn(
        &self,
        run: &mut AgentRun,
        prompt: Arc<Message>,
        history: Vec<Arc<Message>>,
        turn_no: u32,
        on_frame: &mut (dyn FnMut(ChatFrame) + Send),
    ) -> Result<ModelTurnOutcome, PromptError> {
        let no_cancel = CancellationToken::new();
        self.do_stream_turn(run, prompt, history, turn_no, &no_cancel, on_frame)
            .await
            .map(|exit| match exit {
                StreamExit::Completed => ModelTurnOutcome::Continue {
                    response_hook_suppressed: false,
                },
                // 不可取消版永不出 Cancelled（token 从不触发）
                StreamExit::Cancelled => unreachable!("未提供取消令牌不应出现取消退出"),
            })
    }

    /// 取消可感知版（S5 取消传播）：token 触发即 drop HTTP 流（模型流停止），
    /// 返回 [`StreamExit::Cancelled`]；流事件边收边装配出帧的行为与 [`Self::stream_turn`] 一致。
    pub async fn stream_turn_cancellable(
        &self,
        run: &mut AgentRun,
        prompt: Arc<Message>,
        history: Vec<Arc<Message>>,
        turn_no: u32,
        token: &CancellationToken,
        on_frame: &mut (dyn FnMut(ChatFrame) + Send),
    ) -> Result<StreamExit, PromptError> {
        self.do_stream_turn(run, prompt, history, turn_no, token, on_frame)
            .await
    }

    async fn do_stream_turn(
        &self,
        run: &mut AgentRun,
        prompt: Arc<Message>,
        history: Vec<Arc<Message>>,
        turn_no: u32,
        token: &CancellationToken,
        on_frame: &mut (dyn FnMut(ChatFrame) + Send),
    ) -> Result<StreamExit, PromptError> {
        let capabilities = self.model.capabilities().completion;
        let prepared: PreparedRequest = prepare_request(
            &self.spec,
            &capabilities,
            &history,
            self.tools.clone(),
            None,
            None,
        )
        .map_err(|e| PromptError::from(rig::error::ProviderError::request(e.to_string())))?;

        // 记录本 turn 广播给模型的工具（Driver 与 run 失同步的守卫面）
        run.advertise_tools(turn_no as usize, prepared.tools.clone());
        let policy = prepared.policy.clone();

        let request = prepared.apply(CompletionRequest::new(prompt));
        let mut stream = self
            .model
            .stream(request)
            .map_err(|e| PromptError::from(rig::error::ProviderError::request(e.to_string())))?;

        let mut assembler = StreamedTurnAssembler::new(policy);
        let mut bridge = RigEventBridge::new();
        loop {
            // §5.3 取消传播核心点：token 触发与下一流事件竞争，胜出即 drop 流
            //（HTTP 连接关闭，模型侧停止）。cancelled() 本身 cancel-safe，可安全 select。
            let item = tokio::select! {
                biased;
                _ = token.cancelled() => return Ok(StreamExit::Cancelled),
                next = stream.next() => match next {
                    Some(item) => item,
                    None => break,
                },
            };
            let item: Item<rig::streaming::StreamEvent> = item.map_err(|e| {
                PromptError::from(rig::error::ProviderError::request(e.to_string()))
            })?;
            // 装配器先记账（非法调用拦截等），再桥接出帧
            let _assembly_event = assembler.ingest(&item).map_err(|e| {
                PromptError::from(rig::error::ProviderError::request(e.to_string()))
            })?;
            match bridge.bridge(&item, turn_no) {
                Bridged::Frame(frame) => on_frame(frame),
                Bridged::None => {}
            }
        }

        // 流收尾：usage/identity 记账 → 装配 → 回喂 run
        let response = stream
            .finish()
            .await
            .map_err(|e| PromptError::from(rig::error::ProviderError::request(e.to_string())))?;
        run.record_streamed_completion_call(
            response.usage,
            response.identity(),
            response.finish_reason(),
            response.raw.clone(),
        )?;
        let streamed_turn = assembler.finish(&response);
        run.streamed_turn(streamed_turn)?;
        Ok(StreamExit::Completed)
    }
}

/// 手动工具循环（S3 核心）：「单 turn 返回 → 外部执行 → 结果回喂 → 下一 turn」。
/// 返回帧经 `on_frame` 透出（含两轮的流式 delta 与工具帧序列）。
pub async fn drive_manual_tool_loop(
    driver: &TurnDriver,
    mut run: AgentRun,
    executor: Arc<dyn ToolExecutor>,
    on_frame: &mut (dyn FnMut(ChatFrame) + Send),
) -> Result<rig::run::response::PromptResponse, PromptError> {
    loop {
        match run.next_step()? {
            AgentRunStep::CallModel {
                prompt,
                history,
                turn,
            } => {
                // 单 turn 流式调用；CallTools 状态可在此前挂起（审批）
                driver
                    .stream_turn(&mut run, prompt, history, turn as u32, on_frame)
                    .await?;
            }
            AgentRunStep::CallTools { calls } => {
                // 外部执行（生产：§5.4 门控 → 审批挂起 → 执行）；全部完成后整批回喂
                let mut answers = Vec::new();
                for call in calls {
                    let (call_id, name, args_json) = match &call {
                        rig::run::PendingToolCall::Execute(exec) => (
                            exec.id().to_string(),
                            exec.name().to_string(),
                            exec.arguments().to_string(),
                        ),
                        rig::run::PendingToolCall::Malformed(malformed) => (
                            malformed.id().to_string(),
                            malformed.name().to_string(),
                            malformed.raw_arguments().to_owned(),
                        ),
                        // #[non_exhaustive]：未来新增变体在 spike 里不执行
                        other => (
                            String::new(),
                            String::new(),
                            format!("unhandled pending call variant: {other:?}"),
                        ),
                    };
                    let output = executor.execute(&name, &args_json);
                    on_frame(ChatFrame::ToolResult {
                        call_id,
                        ok: true,
                        ui_summary: output.clone(),
                        error: None,
                    });
                    answers.push(answer_one(call, output));
                }
                run.answer_all(answers)?;
            }
            AgentRunStep::Done(response) => return Ok(response),
        }
    }
}

/// 把一个 pending call 的执行结果打包为 ToolAnswer（取消路径也复用：已完成
/// 的工具在取消后仍以原结果收齐）。
pub fn answer_one(call: rig::run::PendingToolCall, output: String) -> rig::run::ToolAnswer {
    match call {
        rig::run::PendingToolCall::Execute(exec) => exec.answer(rig::tool::ToolResult::success(
            rig::tool::ToolOutput::text(output),
        )),
        rig::run::PendingToolCall::Malformed(malformed) => malformed.answer(None),
        // #[non_exhaustive]：未来新增变体在 spike 里按跳过应答
        _ => unreachable!("unhandled pending call variant"),
    }
}
