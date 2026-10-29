//! 单轮流式调用的驱动（技术设计文档 §4.3）。
//!
//! 名词解释：turn（轮）= 一次"向模型发起请求并收到完整回复"的过程。本模块
//! 驱动一轮：步进 rig 的 AgentRun 状态机（sans-IO 指该库本身不做网络请求，
//! 由外部驱动）+ 用 StreamedTurnAssembler（流式回合装配器）把流事件装配成
//! 完整消息 + 经流事件桥接器出帧。
//!
//! 由早期验证原型（S2/S3 spike）产品化而来，当前版本要点：
//! - 桥接器由调用方（主循环）持有并传入，流收口后主循环用它的映射发同源
//!   的 toolResult 帧（§3.4 调用 id 同源）；
//! - 流收口返回该轮的 rig `Usage`（主循环发 usage 帧并累计，§10.5 口径）。

use std::sync::Arc;

use futures::StreamExt;
use rig::completion::Message;
use rig::completion::Usage;
use rig::completion::request::{CompletionRequest, ToolDefinition};
use rig::driver::{Model, Transport};
use rig::operation::Completion;
use rig::run::prepare::{PreparedRequest, prepare_request};
use rig::run::spec::RunSpec;
use rig::run::streamed::StreamedTurnAssembler;
use rig::run::{AgentRun, ModelTurnOutcome, PromptError};
use rig::streaming::Item;
use rig::wire::Wire;
use tokio_util::sync::CancellationToken;

use super::frames::ChatFrame;
use super::rig_bridge::{Bridged, RigEventBridge};

/// 单轮流式调用的退出态：自然收尾，或取消中途丢弃流（§5.3 取消传播）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamExit {
    /// 自然收尾（流正常结束）。
    Completed,
    /// 取消中途退出（流被丢弃，模型侧停止生成）。
    Cancelled,
}

/// 单轮流式调用的驱动器：持有模型客户端、运行规格与工具表，完成
/// "流式请求 → 事件装配 → 出帧 → 回喂 run"的整个过程。
///
/// 模型端泛型化（37 号票）：`W: Wire<Op = Completion>` + `T: Transport<W>`
/// 的任意组合——生产 = OpenAI Chat wire（`Model<Chat>`）；测试 = rig
/// test_utils 的剧本模型（`MockCompletionModel`），供看门狗（37 号票）等
/// 以"剧本假模型 + 假执行器"直驱 `drive_run` 全链。泛型只改客户端类型，
/// 流事件面（`Item<StreamEvent>`）与响应面（`CompletionResponse`）对
/// `Op = Completion` 的全部 wire 同形，方法体零改动。
pub struct TurnDriver<W, T>
where
    W: Wire<Op = Completion>,
    T: Transport<W>,
{
    /// 模型客户端（rig 封装的 completion 端点形态）。
    model: Model<W, T>,
    /// 运行规格（rig 的 RunSpec：采样参数与执行策略）。
    spec: RunSpec,
    /// 本轮广播给模型的工具定义表。
    tools: Vec<Arc<ToolDefinition>>,
}

/// 一次流式调用的收口产出。
pub struct TurnOutcome {
    /// 退出形态：自然收尾 / 取消中途退出。
    pub exit: StreamExit,
    /// 收口用量（取消路径没有收口，为 None——取消的 run 不计费的语义，
    /// §5.3）。
    pub usage: Option<Usage>,
}

/// TurnDriver 的流式调用与一次性调用。
impl<W, T> TurnDriver<W, T>
where
    W: Wire<Op = Completion>,
    T: Transport<W>,
{
    /// 构造：给定模型客户端、运行规格与工具表。
    pub fn new(model: Model<W, T>, spec: RunSpec, tools: Vec<Arc<ToolDefinition>>) -> Self {
        Self { model, spec, tools }
    }

    /// 不可取消版本（直通语义保留给单元测试）：内部取消令牌永不触发。
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

    /// 取消可感知版（§5.3）：取消令牌触发即丢弃 HTTP 流（模型侧停止生成），
    /// 返回 [`StreamExit::Cancelled`]；流事件边收边装配出帧的行为与
    /// [`Self::stream_turn`] 一致。
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

    /// 一次性非流式小调用（如 suggest 建议的后台生成，需求 C-9：不走
    /// AgentRun 步进、无工具表）。返回（拼接文本, usage）；失败由调用方
    /// 静默处理。
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

    /// 流式一轮的实际实现（上面两个公开方法是它的包装）：构造请求 → 开流 →
    /// 逐事件"装配记账 + 桥接出帧" → 流收尾把完整回合回喂 run。
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

/// 把一个待执行调用的执行结果打包为工具应答（取消路径也复用：已完成的
/// 工具在取消后仍以原结果收齐，不留悬空）。
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
