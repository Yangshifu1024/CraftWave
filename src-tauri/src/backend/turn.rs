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
//! - 流收口返回该轮的 rig `Usage`（主循环发 usage 帧并累计，§10.5 口径）；
//! - 模型客户端与运行规格收进互斥锁（18 号票，FR-02-07 修正案）：每拍模型
//!   请求前宿主可经 [`TurnDriver::swap_model`] / [`TurnDriver::set_options`]
//!   热更新（运行中切模型/thinking 下一请求生效），正在进行的请求不被打断
//!   ——锁只在请求装配瞬间持有（clone 出快照后即释放），不影响流式收发。

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt;
use rig::completion::Message;
use rig::completion::Usage;
use rig::completion::options::{GenerationOptions, OnUnsupported};
use rig::completion::request::{CompletionRequest, ToolDefinition};
use rig::completion::{Effort, Reasoning};
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
use crate::providers::reasoning::ReasoningDirective;

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
/// 的任意组合——生产 = 三方言统一 wire（`Model<DialectWire>`，38 号票
/// providers::wire 防腐层，openai_chat / openai_responses /
/// anthropic_messages 三协议按 `ApiFormat` 构造）；测试 = rig test_utils
/// 的剧本模型（`MockCompletionModel`），供看门狗（37 号票）等以"剧本假
/// 模型 + 假执行器"直驱 `drive_run` 全链。泛型只改客户端类型，流事件面
/// （`Item<StreamEvent>`）与响应面（`CompletionResponse`）对
/// `Op = Completion` 的全部 wire 同形，方法体零改动。
///
/// 可热更新（18 号票，FR-02-07 运行中切换）：`model` 与 `spec` 收进互斥锁，
/// 每拍请求前宿主可替换模型客户端 / 运行参数（§11.1：每次模型请求前读取
/// 最新会话模型/thinking 偏好，切换时更新所需客户端和参数，不打断正在
/// 进行的请求）。`downgrade_images` 承载 Q30 运行中切到非 vision 模型的
/// 请求面降级（历史图片块替换为占位文本）。
pub struct TurnDriver<W, T>
where
    W: Wire<Op = Completion>,
    T: Transport<W>,
{
    /// 模型客户端（rig 封装的 completion 端点形态）。锁内替换（swap_model），
    /// 请求装配时 clone 快照使用——锁不跨 await。
    model: Mutex<Model<W, T>>,
    /// 运行规格（rig 的 RunSpec：采样参数与执行策略）。锁内更新（set_options
    /// / set_max_tokens），请求装配时 clone 快照使用。
    spec: Mutex<RunSpec>,
    /// 本轮广播给模型的工具定义表。
    tools: Vec<Arc<ToolDefinition>>,
    /// 请求面历史图片降级开关（18 号票 Q30）：true = 当前模型非 vision，
    /// 发送前把请求 history 中的多模态图片块替换为占位文本（允许纯文字
    /// 继续）；AgentRun 内部历史不动，切回 vision 模型自动恢复原图。
    downgrade_images: AtomicBool,
}

/// 一次流式调用的收口产出。
pub struct TurnOutcome {
    /// 退出形态：自然收尾 / 取消中途退出。
    pub exit: StreamExit,
    /// 收口用量（取消路径没有收口，为 None——取消的 run 不计费的语义，
    /// §5.3）。
    pub usage: Option<Usage>,
}

/// [`ReasoningDirective`] → rig 请求参数（§11.4 → GenerationOptions）。
///
/// 按指令面分派（指令自身即方言标识——`budget_tokens` 只有 Anthropic 映射
/// 产出、`effort_word` 只有 OpenAI 系产出）：
/// - **Anthropic 面（38 号票消费）**：`budget_tokens` 有值 → rig
///   `Reasoning::Budget`（anthropic options 层映射为
///   `{"thinking":{"type":"enabled","budget_tokens":N}}`，§11.4 比例轴）；
///   off 档在 `providers::reasoning` 即折算为空指令（§11.4「off → 不启用
///   thinking（缺省行为）」——不显式发 disabled 字段）。
/// - **OpenAI 系面**：effort 词翻译，off 档（"none"）走 `Reasoning::Off`；
///   其余档走 `Reasoning::Effort`。
///
/// `OnUnsupported::Ignore`：模型/目录不支持的档位跳过而非报错（§11.4
/// 「不支持者忽略」——目录内的官方模型按其 catalog 裁决，目录外的兼容
/// 端点直接透传）。
///
/// 注意：设置任一 options 字段（含 on_unsupported）都会让 rig 对目录内
/// 模型启用档位校验；`None` 指令返回默认空 options（请求不携带任何推理
/// 参数，也不触发校验——与"不传"语义一致）。
pub fn reasoning_directive_to_options(directive: &ReasoningDirective) -> GenerationOptions {
    if let Some(tokens) = directive.budget_tokens {
        // Anthropic 预算轴：u64 指令值收进 rig 的 u32 预算面（窗口缺省
        // 128k 时 20% ≈ 25.6k，远在 u32 内；溢出钳到 u32::MAX 属防御面）
        let tokens = u32::try_from(tokens).unwrap_or(u32::MAX);
        return GenerationOptions::default()
            .reasoning(Reasoning::Budget { tokens })
            .on_unsupported(OnUnsupported::Ignore);
    }
    let Some(word) = directive.effort_word else {
        return GenerationOptions::default();
    };
    let reasoning = if word == "none" {
        Reasoning::Off
    } else {
        Reasoning::Effort(effort_from_word(word))
    };
    GenerationOptions::default()
        .reasoning(reasoning)
        .on_unsupported(OnUnsupported::Ignore)
}

/// Anthropic 面的请求级 `max_tokens`（§11.4 + rig Messages wire 的硬性
/// 要求，38 号票）：Anthropic API 必填 max_tokens 且须大于 thinking 预算
/// （目录外模型 rig 无按模型缺省可兜，缺失即 EncodeError 拒发）——因此
/// anthropic_messages 方言**恒**注入：有上调规则值时用上调值
/// （`max_tokens_override`），否则用模型配置的输出上限
/// （`ProviderModel.maxTokens`）。OpenAI 系返回 None——维持 openai_chat /
/// openai_responses 现状（请求不带 max_tokens，不回归）。
pub fn directive_max_tokens(
    directive: &ReasoningDirective,
    base_max_tokens: u64,
    api_format: crate::providers::types::ApiFormat,
) -> Option<u64> {
    match api_format {
        crate::providers::types::ApiFormat::AnthropicMessages => {
            Some(directive.max_tokens_override.unwrap_or(base_max_tokens))
        }
        crate::providers::types::ApiFormat::OpenaiChat
        | crate::providers::types::ApiFormat::OpenaiResponses => None,
    }
}

/// effort 词 → rig `Effort`（词表与 [`crate::providers::reasoning::
/// ThinkingLevel::effort_word`] 同源，未知词按 Medium 兜底——正常路径不会
/// 出现，防御性收口）。
fn effort_from_word(word: &str) -> Effort {
    match word {
        "minimal" => Effort::Minimal,
        "low" => Effort::Low,
        "high" => Effort::High,
        "xhigh" => Effort::XHigh,
        "max" => Effort::Max,
        _ => Effort::Medium,
    }
}

/// TurnDriver 的流式调用与一次性调用。
impl<W, T> TurnDriver<W, T>
where
    W: Wire<Op = Completion>,
    T: Transport<W>,
{
    /// 构造：给定模型客户端、运行规格与工具表。
    pub fn new(model: Model<W, T>, spec: RunSpec, tools: Vec<Arc<ToolDefinition>>) -> Self {
        Self {
            model: Mutex::new(model),
            spec: Mutex::new(spec),
            tools,
            downgrade_images: AtomicBool::new(false),
        }
    }

    /// 热替换模型客户端（18 号票，运行中切模型）：下一拍请求起用新模型，
    /// 正在进行的请求不受影响（其客户端快照已在流式调用期间持有）。
    pub fn swap_model(&self, model: Model<W, T>) {
        *self
            .model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = model;
    }

    /// 热更新运行参数（18 号票，运行中切 thinking / 模型窗口参数）：
    /// 以 `apply` 变换当前 RunSpec（通常只动 `options` 与 `max_tokens`，
    /// max_turns 等执行策略保持 run 内一致）。
    pub fn update_spec(&self, apply: impl FnOnce(&mut RunSpec)) {
        let mut spec = self
            .spec
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        apply(&mut spec);
    }

    /// 设置请求面历史图片降级开关（18 号票 Q30）：true = 当前模型非 vision，
    /// 后续请求把 history 中的多模态图片块替换为占位文本。
    pub fn set_downgrade_images(&self, flag: bool) {
        self.downgrade_images.store(flag, Ordering::Relaxed);
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
        let model = self
            .model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let response = model
            .call(request)
            .await
            .map_err(|e| PromptError::from(rig::error::ProviderError::request(e.to_string())))?;
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
        // 客户端与规格快照（18 号票：锁内 clone 即释放，不跨 await——
        // 每拍请求前宿主可能已经 swap 过，本拍用最新组合）
        let model = self
            .model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let spec = self
            .spec
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        // Q30 请求面降级（18 号票）：当前模型非 vision 时把历史图片块替换
        // 为占位文本（AgentRun 内部历史不动，切回 vision 模型自动恢复）
        let history = if self.downgrade_images.load(Ordering::Relaxed) {
            downgrade_history_images(history)
        } else {
            history
        };
        let capabilities = model.capabilities().completion;
        let prepared: PreparedRequest = prepare_request(
            &spec,
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
        let mut stream = model
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

/// 请求面历史图片降级（18 号票，Q30 运行中切非 vision 模型）：遍历请求
/// history，把 user 消息里的多模态图片块替换为占位文本（允许纯文字继续）。
/// AgentRun 内部历史不动——切回 vision 模型后原图自动恢复。
///
/// 与存储面占位（`wire::image_placeholder`，带 blobHash 逐字格式）不同：
/// rig 多模态块已不携带哈希，此处用通用文案；请求面降级不入史册，前端
/// 展示不受影响（17 号票的气泡提示行按模型能力独立判定）。
pub fn downgrade_history_images(history: Vec<Arc<Message>>) -> Vec<Arc<Message>> {
    const RUNTIME_PLACEHOLDER: &str = "[历史图片已省略：当前模型无法查看图片]";
    history
        .into_iter()
        .map(|message| match Arc::unwrap_or_clone(message) {
            Message::User { content } if content.iter().any(is_image_content) => {
                Arc::new(Message::User {
                    content: content
                        .into_iter()
                        .map(|c| {
                            if is_image_content(&c) {
                                rig::message::UserContent::text(RUNTIME_PLACEHOLDER.to_owned())
                            } else {
                                c
                            }
                        })
                        .collect(),
                })
            }
            other => Arc::new(other),
        })
        .collect()
}

/// 是否图片多模态块。
fn is_image_content(content: &rig::message::UserContent) -> bool {
    matches!(content, rig::message::UserContent::Image(_))
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
