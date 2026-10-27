//! 单 turn 流式调用驱动（§4.3）：「AgentRun sans-IO 步进 + StreamedTurnAssembler
//! 装配 + 流事件桥接出帧」。产品化自 S2/S3 spike 同名原型，增量：
//! - 桥接器由调用方（RunLoop）持有并传入，流收口后 RunLoop 以其映射发
//!   同源 `toolResult` 帧（§3.4 callId 同源）；
//! - 流收口返回该轮 rig `Usage`（RunLoop 发 usage 帧并累计，§10.5 口径）。

use std::sync::Arc;

use futures::StreamExt;
use rig::completion::Message;
use rig::completion::Usage;
use rig::completion::request::{CompletionRequest, ToolDefinition};
use rig::driver::Model;
use rig::providers::openai::wire::Chat;
use rig::run::prepare::{PreparedRequest, prepare_request};
use rig::run::spec::RunSpec;
use rig::run::streamed::StreamedTurnAssembler;
use rig::run::{AgentRun, ModelTurnOutcome, PromptError};
use rig::streaming::Item;
use tokio_util::sync::CancellationToken;

use super::frames::ChatFrame;
use super::rig_bridge::{Bridged, RigEventBridge};

/// 单 turn 流式调用的退出态：自然收尾，或取消中途 drop 流（§5.3 取消传播）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamExit {
    Completed,
    Cancelled,
}

/// 单 turn 流式调用的驱动：S2 的事件帧化 + S3 的装配回喂。
pub struct TurnDriver {
    model: Model<Chat>,
    spec: RunSpec,
    tools: Vec<Arc<ToolDefinition>>,
}

/// 一次流式 turn 的收口产出。
pub struct TurnOutcome {
    /// 自然收尾 / 取消中途退出。
    pub exit: StreamExit,
    /// 收口 usage（取消路径无收口，None——取消 run 不计费语义，§5.3/S5 票）。
    pub usage: Option<Usage>,
}

impl TurnDriver {
    pub fn new(model: Model<Chat>, spec: RunSpec, tools: Vec<Arc<ToolDefinition>>) -> Self {
        Self { model, spec, tools }
    }

    /// 不可取消版（直通语义保留给单测）：内部 token 永不触发。
    pub async fn stream_turn(
        &self,
        run: &mut AgentRun,
        prompt: Arc<Message>,
        history: Vec<Arc<Message>>,
        turn_no: u32,
        bridge: &mut RigEventBridge,
        on_frame: &mut (dyn FnMut(ChatFrame) + Send),
    ) -> Result<ModelTurnOutcome, PromptError> {
        let no_cancel = CancellationToken::new();
        let outcome = self
            .do_stream_turn(run, prompt, history, turn_no, &no_cancel, bridge, on_frame)
            .await?;
        match outcome.exit {
            StreamExit::Completed => Ok(ModelTurnOutcome::Continue {
                response_hook_suppressed: false,
            }),
            // 不可取消版永不出 Cancelled（token 从不触发）
            StreamExit::Cancelled => unreachable!("未提供取消令牌不应出现取消退出"),
        }
    }

    /// 取消可感知版（§5.3）：token 触发即 drop HTTP 流（模型流停止），返回
    /// [`StreamExit::Cancelled`]；流事件边收边装配出帧的行为与 [`Self::stream_turn`] 一致。
    #[allow(clippy::too_many_arguments)]
    pub async fn stream_turn_cancellable(
        &self,
        run: &mut AgentRun,
        prompt: Arc<Message>,
        history: Vec<Arc<Message>>,
        turn_no: u32,
        token: &CancellationToken,
        bridge: &mut RigEventBridge,
        on_frame: &mut (dyn FnMut(ChatFrame) + Send),
    ) -> Result<TurnOutcome, PromptError> {
        self.do_stream_turn(run, prompt, history, turn_no, token, bridge, on_frame)
            .await
    }

    /// 一次性非流式小调用（suggest 后台生成等，C-9：不走 AgentRun 步进、
    /// 无工具表）。返回 (拼接文本, usage)；失败由调用方静默处理。
    pub async fn complete_once(&self, prompt: &str) -> Result<(String, Usage), PromptError> {
        let request = CompletionRequest::new(prompt.to_owned());
        let response =
            self.model.call(request).await.map_err(|e| {
                PromptError::from(rig::error::ProviderError::request(e.to_string()))
            })?;
        let text = response
            .choice
            .iter()
            .filter_map(|block| match block {
                rig::message::AssistantContent::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        Ok((text, response.usage))
    }

    #[allow(clippy::too_many_arguments)]
    async fn do_stream_turn(
        &self,
        run: &mut AgentRun,
        prompt: Arc<Message>,
        history: Vec<Arc<Message>>,
        turn_no: u32,
        token: &CancellationToken,
        bridge: &mut RigEventBridge,
        on_frame: &mut (dyn FnMut(ChatFrame) + Send),
    ) -> Result<TurnOutcome, PromptError> {
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
        loop {
            // §5.3 取消传播核心点：token 触发与下一流事件竞争，胜出即 drop 流
            //（HTTP 连接关闭，模型侧停止）。cancelled() 本身 cancel-safe，可安全 select。
            let item = tokio::select! {
                biased;
                _ = token.cancelled() => {
                    return Ok(TurnOutcome { exit: StreamExit::Cancelled, usage: None });
                }
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
        let usage = response.usage;
        run.record_streamed_completion_call(
            response.usage,
            response.identity(),
            response.finish_reason(),
            response.raw.clone(),
        )?;
        let streamed_turn = assembler.finish(&response);
        run.streamed_turn(streamed_turn)?;
        Ok(TurnOutcome {
            exit: StreamExit::Completed,
            usage: Some(usage),
        })
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
        // #[non_exhaustive]：未来新增变体按跳过应答
        _ => unreachable!("unhandled pending call variant"),
    }
}

/// 把 rig `Usage` 折算为帧面 [`UsageTotal`](super::frames::UsageTotal)
/// （§3.2 口径：input/output/cacheRead/cacheWrite；未报告计数按 0）。
pub fn usage_total(usage: &Usage) -> super::frames::UsageTotal {
    super::frames::UsageTotal {
        input: usage.input_tokens.unwrap_or(0),
        output: usage.output_tokens.unwrap_or(0),
        cache_read: usage.cached_input_tokens.unwrap_or(0),
        cache_write: usage.cache_creation_input_tokens.unwrap_or(0),
    }
}
