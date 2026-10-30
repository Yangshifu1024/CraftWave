//! 三协议方言的统一 wire 防腐层（FR-02-01，38 号票，§11.1 模型解析）。
//!
//! 名词解释：wire 是 rig 0.45 里"一种 API 方言的请求编码 + 回复解码"的
//! 实现单元（[`rig::wire::Wire`]）；方言（dialect）指同一件事在 CraftWave
//! 语境下的叫法（§3.2 `ProviderConfig.apiFormat`）。三个方言在 rig 中是
//! 三种 wire 实现：
//! - `openai_chat` → `POST {base}/chat/completions`（rig `Chat`）；
//! - `openai_responses` → `POST {base}/responses`（rig `Responses`）；
//! - `anthropic_messages` → `POST {base}/v1/messages`（rig `Messages`）。
//!
//! rig 自己已用 [`OpenAiWire`] 枚举统一了 Chat / Responses 两条 OpenAI
//! 路线（按 `Route` 配置分派）；本模块照同款先例再包一层，把 Anthropic
//! Messages 也收进来，得到 [`DialectWire`]——`start_chat` 全链
//! （TurnDriver / drive_run，37 号票已泛型化为 `Model<W, T>`）由此以单一
//! 生产类型贯穿，方言差异收敛在本文件（防腐层原则 §4.1：rig 升级只应
//! 改到 backend/ 与 providers/）。
//!
//! 三个 wire 的请求载荷（`Encoded`）与回复帧（`WireFrame`）同形，因此
//! 代理工厂注入的 HTTP 客户端（rig `DynHttpClient`，经 rig 的 blanket
//! `Transport` 实现）无需感知方言；差异只在流事件的分类类型（Event）与
//! 解码器（Decoder / Reassembler），逐变体委托转发。

use rig::driver::Model;
use rig::error::{EncodeError, ProviderError};
use rig::http_client::DynHttpClient;
use rig::operation::Completion;
use rig::providers::anthropic::streaming::{MessagesDecoder, MessagesDocument, MessagesEvent};
use rig::providers::anthropic::wire::{AnthropicConfig, Messages};
use rig::providers::openai::wire::{
    OpenAIConfig, OpenAiDecoder, OpenAiEvent, OpenAiReassembler, OpenAiWire, Route,
};
use rig::wire::document::Reassemble;
use rig::wire::{Decoder, Descriptor, Encoded, Flow, Mode, Out, Wire, WireEvent, WireFrame};

use super::types::ApiFormat;

/// 三方言统一的 completion wire：`start_chat` 生产链的模型客户端类型
/// （`Model<DialectWire>`）。变体二选一，由 [`dialect_wire`] 按
/// `ApiFormat` 构造。
#[derive(Clone, Debug)]
pub enum DialectWire {
    /// OpenAI 系（openai_chat 与 openai_responses 两条路线，内部按
    /// `Route` 分派——rig `OpenAiWire` 原生承载）。
    Openai(Box<OpenAiWire>),
    /// Anthropic Messages 方言（Box：`OpenAiWire`/`Messages` 都是显著大于
    /// 指针的内联类型，clippy large_enum_variant；委托经 Deref 照常）。
    Anthropic(Box<Messages>),
}

/// 三方言统一的一条流事件（解码器的分类产物，再折叠为统一的
/// `StreamEvent`——`Model::stream` 对 `Op = Completion` 的全部 wire 同形）。
/// 纯透传载体，不派生额外能力（rig 侧事件类型未实现 Clone/Debug）。
pub enum DialectEvent {
    /// OpenAI 系路线的一条事件。
    Openai(OpenAiEvent),
    /// Anthropic Messages 的一条事件。
    Anthropic(MessagesEvent),
}

/// 三方言统一的回复解码器（rig `Decoder` 的枚举分发壳）。Anthropic 变体
/// Box 化理由同 [`DialectWire`]。
pub enum DialectDecoder {
    /// OpenAI 系路线的解码器。
    Openai(Box<OpenAiDecoder>),
    /// Anthropic Messages 的解码器。
    Anthropic(Box<MessagesDecoder>),
}

/// 三方言统一的回复文档重建器（流收尾把事件重建成 raw 文档，供审计与
/// usage 补算面）。变体与解码器一一配对。
pub enum DialectReassembler {
    /// OpenAI 系路线的重建器（默认变体，对齐 rig `OpenAiWire` 的 Chat
    /// 默认路线先例；实际配对由 [`Wire::reassembler`] 决定）。
    Openai(OpenAiReassembler),
    /// Anthropic Messages 的重建器。
    Anthropic(MessagesDocument),
}

// Default 给 OpenAI 变体（derive 不支持带数据的变体，照 rig
// OpenAiReassembler 的手写先例）。
impl Default for DialectReassembler {
    fn default() -> Self {
        Self::Openai(Default::default())
    }
}

impl Wire for DialectWire {
    type Op = Completion;
    type Payload = Encoded;
    type Frame = WireFrame;
    type Event = DialectEvent;
    type Decoder<'id> = DialectDecoder;
    type Reassembler = DialectReassembler;

    fn describe(&self) -> Descriptor<'_> {
        match self {
            Self::Openai(wire) => wire.describe(),
            Self::Anthropic(wire) => wire.describe(),
        }
    }

    fn encode(
        &self,
        request: rig::completion::CompletionRequest,
        mode: Mode,
    ) -> Result<Encoded, EncodeError> {
        match self {
            Self::Openai(wire) => wire.encode(request, mode),
            Self::Anthropic(wire) => wire.encode(request, mode),
        }
    }

    fn decoder<'id>(&self) -> Self::Decoder<'id> {
        match self {
            Self::Openai(wire) => DialectDecoder::Openai(Box::new(wire.decoder())),
            Self::Anthropic(wire) => DialectDecoder::Anthropic(Box::new(wire.decoder())),
        }
    }

    fn reassembler(&self) -> Self::Reassembler {
        match self {
            Self::Openai(wire) => DialectReassembler::Openai(wire.reassembler()),
            Self::Anthropic(wire) => DialectReassembler::Anthropic(wire.reassembler()),
        }
    }
}

impl<'id> Decoder<'id, Completion> for DialectDecoder {
    type Event = DialectEvent;

    fn classify(&self, frame: WireFrame) -> WireEvent<DialectEvent> {
        match self {
            Self::Openai(decoder) => decoder.classify(frame).map(DialectEvent::Openai),
            Self::Anthropic(decoder) => decoder.classify(frame).map(DialectEvent::Anthropic),
        }
    }

    fn decode(
        &mut self,
        event: DialectEvent,
        out: Out<'id, Completion>,
    ) -> Result<Flow, ProviderError> {
        match (self, event) {
            (Self::Openai(decoder), DialectEvent::Openai(event)) => decoder.decode(event, out),
            (Self::Anthropic(decoder), DialectEvent::Anthropic(event)) => {
                decoder.decode(event, out)
            }
            // 解码器只会收到自己 classifier 分出的事件（driver 的配对面），
            // 交叉变体按"还有更多"继续（同 rig route.rs 的防御口径）
            (Self::Openai(_), DialectEvent::Anthropic(_))
            | (Self::Anthropic(_), DialectEvent::Openai(_)) => Ok(Flow::More),
        }
    }

    fn eof(&mut self, out: Out<'id, Completion>) -> Result<Flow, ProviderError> {
        match self {
            Self::Openai(decoder) => decoder.eof(out),
            Self::Anthropic(decoder) => decoder.eof(out),
        }
    }
}

impl Reassemble<DialectEvent> for DialectReassembler {
    fn absorb(&mut self, event: &DialectEvent) {
        match (self, event) {
            (Self::Openai(document), DialectEvent::Openai(event)) => document.absorb(event),
            (Self::Anthropic(document), DialectEvent::Anthropic(event)) => document.absorb(event),
            // 变体不配对时无事可做（同上：decoder 只产自己的事件）
            _ => {}
        }
    }

    fn finish(self) -> serde_json::Value {
        match self {
            Self::Openai(document) => document.finish(),
            Self::Anthropic(document) => document.finish(),
        }
    }
}

impl rig::wire::document::Serves<Completion> for DialectReassembler {}

/// 按方言构造统一的 completion wire（§11.1 模型解析的方言装配面）。
///
/// - 地址：`base_url` 为空时取该方言的官方缺省地址（§3.2 注记）；Anthropic
///   面的 `/v1`、`/messages`、`/v1/messages` 尾缀由 rig
///   `AnthropicConfig::with_base_url` 归一（防"地址已含端点路径"产生
///   `/v1/messages/v1/messages`）；OpenAI 系原样使用（兼容端点地址即 base）。
/// - 路线：openai_chat 与 openai_responses 都**显式**指定 `Route`——OpenAI
///   官方 dialect 的缺省路线是 Responses，不显式强制会让 openai_chat 打错
///   端点；反之 responses 方言也不能落回 Chat。
/// - Key：调用方已按 `ProviderConfig` 装配（本地服务以占位值过 rig 必填
///   面，§11.1 回写口径）。
pub fn dialect_wire(
    api_format: ApiFormat,
    base_url: &str,
    api_key: &str,
    model: &str,
) -> DialectWire {
    match api_format {
        ApiFormat::OpenaiChat => {
            let config = OpenAIConfig::new(api_key.to_owned())
                .with_base_url(base_url.to_owned())
                .with_route(Route::Chat);
            DialectWire::Openai(Box::new(OpenAiWire::new(config, model)))
        }
        ApiFormat::OpenaiResponses => {
            let config = OpenAIConfig::new(api_key.to_owned())
                .with_base_url(base_url.to_owned())
                .with_route(Route::Responses);
            DialectWire::Openai(Box::new(OpenAiWire::new(config, model)))
        }
        ApiFormat::AnthropicMessages => {
            let config = AnthropicConfig::new(api_key.to_owned()).with_base_url(base_url);
            DialectWire::Anthropic(Box::new(config.completion(model)))
        }
    }
}

/// 按方言构造挂好代理 HTTP 客户端的模型端点（§11.1 传输注入口径的方言
/// 收口）：`http` 为 providers/http.rs 代理工厂快照包装出的 rig 传输句柄，
/// 模型请求、连接探测与网络工具共用同一条代理语义。
pub fn dialect_model(
    api_format: ApiFormat,
    base_url: &str,
    api_key: &str,
    model: &str,
    http: DynHttpClient,
) -> Model<DialectWire> {
    Model::new(dialect_wire(api_format, base_url, api_key, model), http)
}

#[cfg(test)]
#[path = "wire_test.rs"]
mod wire_test;
