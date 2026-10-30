//! [`super::wire`] 三方言统一防腐层的单测（38 号票）。
//!
//! 不发网络请求：验证三方言按 `ApiFormat` 构造出正确变体、描述符里的
//! 方言名/模型名正确、对同一组请求参数的 encode 走向各自的端点路径
//! （ rig `Encoded` 载荷里的请求 URI），以及解码器/重建器变体配对。

use rig::completion::CompletionRequest;
use rig::message::Message;
use rig::wire::{Mode, Wire};

use crate::providers::types::ApiFormat;
use crate::providers::wire::{
    DialectDecoder, DialectEvent, DialectReassembler, DialectWire, dialect_wire,
};

const KEY: &str = "test-key";

/// 三方言各自构造出的 wire 变体与 describe 元数据。
#[test]
fn dialect_wire_builds_expected_variant_per_format() {
    // openai_chat：OpenAI 路线变体，描述符方言名 = OpenAI 官方 dialect 名
    let chat = dialect_wire(
        ApiFormat::OpenaiChat,
        "https://api.openai.com/v1",
        KEY,
        "gpt-mock",
    );
    match &chat {
        DialectWire::Openai(boxed) => match boxed.as_ref() {
            rig::providers::openai::wire::OpenAiWire::Chat(inner) => {
                assert_eq!(inner.model, "gpt-mock");
            }
            other => panic!("openai_chat 应构造 Chat 路线，实际 {other:?}"),
        },
        other => panic!("openai_chat 应构造 OpenAI 路线，实际 {other:?}"),
    }
    let descriptor = chat.describe();
    assert_eq!(descriptor.name, "openai");
    assert_eq!(descriptor.model, Some("gpt-mock"));

    // openai_responses：同为 OpenAI 路线变体（Responses 路线）
    let responses = dialect_wire(
        ApiFormat::OpenaiResponses,
        "https://api.openai.com/v1",
        KEY,
        "gpt-mock",
    );
    match &responses {
        DialectWire::Openai(boxed) => match boxed.as_ref() {
            rig::providers::openai::wire::OpenAiWire::Responses(inner) => {
                assert_eq!(inner.model, "gpt-mock");
            }
            other => panic!("openai_responses 应构造 Responses 路线，实际 {other:?}"),
        },
        other => panic!("openai_responses 应构造 OpenAI 路线，实际 {other:?}"),
    }

    // anthropic_messages：Anthropic Messages 变体
    let anthropic = dialect_wire(
        ApiFormat::AnthropicMessages,
        "https://api.anthropic.com",
        KEY,
        "claude-mock",
    );
    match &anthropic {
        DialectWire::Anthropic(inner) => assert_eq!(inner.model, "claude-mock"),
        other => panic!("anthropic_messages 应构造 Messages，实际 {other:?}"),
    }
}

/// 同一逻辑地址在两方言下的 encode 端点路径分派（Encoded 载荷的请求 URI）。
#[test]
fn dialect_wire_encodes_to_dialect_endpoints() {
    for (api_format, base_url, expected_suffix) in [
        (
            ApiFormat::OpenaiChat,
            "https://api.example.com/v1",
            "/v1/chat/completions",
        ),
        (
            ApiFormat::OpenaiResponses,
            "https://api.example.com/v1",
            "/v1/responses",
        ),
        (
            ApiFormat::AnthropicMessages,
            "https://api.anthropic.com",
            "/v1/messages",
        ),
    ] {
        let wire = dialect_wire(api_format, base_url, KEY, "mock-model");
        // Anthropic wire 硬性要求请求带 max_tokens（目录外模型无缺省可兜）——
        // 与 chat.rs 按方言注入 RunSpec.max_tokens 的口径互为印证
        let request = CompletionRequest::new(Message::user("ping")).max_tokens(
            if api_format == ApiFormat::AnthropicMessages {
                Some(1024)
            } else {
                None
            },
        );
        let encoded = wire.encode(request, Mode::Unary).expect("encode");
        let uri = encoded.request.uri().to_string();
        assert!(
            uri.ends_with(expected_suffix),
            "{api_format:?} 应编码到 {expected_suffix}，实际 {uri}"
        );
    }
}

/// Anthropic 地址归一：用户把端点路径（/v1、/v1/messages）也填进 baseUrl
/// 时，encode 仍落在唯一的 /v1/messages（防双路径拼接）。
#[test]
fn anthropic_base_url_suffixes_are_normalized() {
    for base in [
        "https://api.anthropic.com",
        "https://api.anthropic.com/",
        "https://api.anthropic.com/v1",
        "https://api.anthropic.com/v1/messages",
    ] {
        let wire = dialect_wire(ApiFormat::AnthropicMessages, base, KEY, "claude-mock");
        let request = CompletionRequest::new(Message::user("ping")).max_tokens(1024);
        let encoded = wire.encode(request, Mode::Unary).expect("encode");
        let uri = encoded.request.uri().to_string();
        assert_eq!(
            uri, "https://api.anthropic.com/v1/messages",
            "baseUrl {base} 归一后应落在唯一端点"
        );
    }
}

/// 解码器与重建器的变体按 wire 变体配对（不发流：仅取类型面）。
#[test]
fn decoder_and_reassembler_pair_with_wire_variant() {
    let chat = dialect_wire(
        ApiFormat::OpenaiChat,
        "https://api.openai.com/v1",
        KEY,
        "gpt-mock",
    );
    assert!(matches!(chat.decoder(), DialectDecoder::Openai(_)));
    assert!(matches!(chat.reassembler(), DialectReassembler::Openai(_)));

    let anthropic = dialect_wire(
        ApiFormat::AnthropicMessages,
        "https://api.anthropic.com",
        KEY,
        "claude-mock",
    );
    assert!(matches!(anthropic.decoder(), DialectDecoder::Anthropic(_)));
    assert!(matches!(
        anthropic.reassembler(),
        DialectReassembler::Anthropic(_)
    ));
}

/// 图片多模态块的方言 encode（17 号票 wire 重建双形态的下游实证）：
/// 带 image 块的请求经 anthropic wire 编码后，请求体出现 Anthropic 的
/// base64 图片块形状（{"type":"image","source":{...}}）——图片链路在
/// 三方言下由 rig encode 承载，CraftWave 侧零方言代码。
#[test]
fn anthropic_wire_encodes_image_blocks() {
    let image = rig::message::UserContent::Image(rig::message::Image {
        data: rig::message::DocumentSourceKind::base64("aGVsbG8="),
        media_type: Some(rig::message::ImageMediaType::PNG),
        ..rig::message::Image::default()
    });
    let message = rig::completion::Message::User {
        content: vec![rig::message::UserContent::text("看这张图"), image],
    };
    let request = CompletionRequest::new(message).max_tokens(1024);
    let wire = dialect_wire(
        ApiFormat::AnthropicMessages,
        "https://api.anthropic.com",
        KEY,
        "claude-mock",
    );
    let encoded = wire.encode(request, Mode::Unary).expect("encode");
    let bytes = match encoded.request.body() {
        rig::wire::Body::Bytes(bytes) => bytes,
        other => panic!("应为序列化 JSON 体，实际 {other:?}"),
    };
    let body_text = String::from_utf8(bytes.clone()).expect("utf8");
    assert!(
        body_text.contains("\"type\":\"image\"") && body_text.contains("\"source\""),
        "anthropic 请求体应含 base64 图片块：{body_text}"
    );
}

/// 事件枚举的透传面：DialectEvent 满足 Wire::Event 的承载约束（Send + 'static；
/// rig 侧事件类型未实现 Clone，事件枚举只做透传不做复制）。
#[test]
fn dialect_event_is_send_sync() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<DialectEvent>();
    assert_send_sync::<DialectWire>();
}
