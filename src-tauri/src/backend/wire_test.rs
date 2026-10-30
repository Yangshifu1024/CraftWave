//! wire 重建的单元测试：文本/推理原样映射、toolCall 与 toolResult 按调用
//! id 配对拆分、prompt 与 history 切分、系统级前缀注入（customPrompt 有/无
//! 两态，03 号票）、非法形态（空序列/末条非 user/悬空 toolResult）拒绝。

use rig::completion::Message;
use rig::message::{ToolResultContent, UserContent};

use crate::backend::wire::{WireError, rebuild_message_stream, split_prompt, with_system_prefix};
use crate::storage::{MessageEntry, MessageRole, WirePart};

/// 造一条 user 文本消息。
fn user(id: &str, text: &str) -> MessageEntry {
    MessageEntry {
        id: id.to_owned(),
        parent_id: None,
        role: MessageRole::User,
        parts: vec![WirePart::Text {
            text: text.to_owned(),
        }],
        created_at: 1,
        run_id: None,
        status: None,
    }
}

/// 造一条含完整工具回合的 assistant 消息（推理 + 文本 + toolCall +
/// toolResult + 收尾文本）。
fn assistant_with_tool_round(id: &str, call_id: &str, tool_name: &str) -> MessageEntry {
    MessageEntry {
        id: id.to_owned(),
        parent_id: None,
        role: MessageRole::Assistant,
        parts: vec![
            WirePart::Reasoning {
                text: "先读文件".to_owned(),
            },
            WirePart::Text {
                text: "我来读一下。".to_owned(),
            },
            WirePart::ToolCall {
                call_id: call_id.to_owned(),
                name: tool_name.to_owned(),
                args_json: r#"{"path":"a.txt"}"#.to_owned(),
            },
            WirePart::ToolResult {
                call_id: call_id.to_owned(),
                ok: true,
                summary: "文件共 3 行".to_owned(),
            },
            WirePart::Text {
                text: "读完了。".to_owned(),
            },
        ],
        created_at: 2,
        run_id: Some("run-1".to_owned()),
        status: None,
    }
}

/// 提取 user 消息的全部文本（断言辅助）。
fn user_text(msg: &Message) -> String {
    let Message::User { content } = msg else {
        panic!("应为 user 消息");
    };
    content
        .iter()
        .filter_map(|c| match c {
            UserContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Arc<Message> 解引用辅助（测试断言用）。
fn as_message(arc: &std::sync::Arc<Message>) -> &Message {
    arc.as_ref()
}

/// 断言末条 user 消息作为 prompt，其余重建为 history（工具结果拆成一条
/// 独立的 user 消息）。
#[test]
fn last_user_as_prompt_rest_history() {
    let entries = vec![
        user("u1", "第一条"),
        assistant_with_tool_round("a1", "call_1", "read"),
        user("u2", "再来一条"),
    ];
    let (history, prompt) = split_prompt(&entries).unwrap();
    assert_eq!(user_text(&prompt), "再来一条");
    // history：u1 + (a1 的 assistant) + (a1 拆出的工具结果 user) = 3 条
    assert_eq!(history.len(), 3);
    assert_eq!(user_text(as_message(&history[0])), "第一条");
    assert!(matches!(as_message(&history[1]), Message::Assistant(_)));
    let Message::User { content } = as_message(&history[2]) else {
        panic!("工具结果应为 user 消息");
    };
    assert_eq!(content.len(), 1, "toolResult 合并为一条 user 消息");
}

/// 断言工具结果按调用 id 配对出工具名，ok 标志映射为 is_error 取反，
/// 摘要作为文本内容。
#[test]
fn tool_results_pair_by_call_id_and_error_mapping() {
    let entry = assistant_with_tool_round("a1", "call_9", "grep");
    let stream = rebuild_message_stream(&[entry]).unwrap();
    assert_eq!(stream.len(), 2, "assistant + 工具结果 user");
    let Message::User { content } = as_message(stream.last().unwrap()) else {
        panic!("应有工具结果消息");
    };
    let UserContent::ToolResult(result) = &content[0] else {
        panic!("应为 ToolResult content");
    };
    assert_eq!(result.call.to_string(), "call_9");
    assert_eq!(result.name.to_string(), "grep");
    assert!(!result.is_error, "ok=true → is_error=false");
    match &result.content[0] {
        ToolResultContent::Text(t) => assert_eq!(t.text, "文件共 3 行"),
        other => panic!("应为文本工具结果：{other:?}"),
    }
}

/// 断言失败的工具结果（ok=false）映射为 is_error=true。
#[test]
fn failed_tool_result_is_error_true() {
    let mut entry = assistant_with_tool_round("a1", "call_1", "read");
    entry.parts[3] = WirePart::ToolResult {
        call_id: "call_1".to_owned(),
        ok: false,
        summary: "E_INVALID_ARG: 参数错".to_owned(),
    };
    let stream = rebuild_message_stream(&[entry]).unwrap();
    let Message::User { content } = as_message(stream.last().unwrap()) else {
        panic!("应有工具结果消息");
    };
    let UserContent::ToolResult(result) = &content[0] else {
        panic!("应为 ToolResult content");
    };
    assert!(result.is_error);
}

/// 断言悬空的 toolResult（同条目内没有配对的 toolCall）被拒绝。
#[test]
fn tool_result_without_paired_call_rejected() {
    let entry = MessageEntry {
        id: "a1".to_owned(),
        parent_id: None,
        role: MessageRole::Assistant,
        parts: vec![WirePart::ToolResult {
            call_id: "call_x".to_owned(),
            ok: true,
            summary: "悬空结果".to_owned(),
        }],
        created_at: 1,
        run_id: None,
        status: None,
    };
    let entries = vec![user("u1", "hi"), entry, user("u2", "再问")];
    assert_eq!(
        split_prompt(&entries),
        Err(WireError(
            "消息 a1 的 toolResult call_x 缺配对的 toolCall".to_owned()
        ))
    );
}

/// 断言空序列与末条不是 user 的序列都被拒绝。
#[test]
fn empty_sequence_or_non_user_tail_rejected() {
    assert_eq!(
        split_prompt(&[]),
        Err(WireError("start_chat 消息序列为空".to_owned()))
    );
    let assistant_only = vec![assistant_with_tool_round("a1", "call_1", "read")];
    assert!(matches!(split_prompt(&assistant_only), Err(WireError(_))));
}

/// 断言 runMarker 与 planSnapshot 两类 part 被跳过（不产出消息、不影响
/// 其余内容）。
#[test]
fn run_marker_and_plan_snapshot_skipped() {
    let entry = MessageEntry {
        id: "a1".to_owned(),
        parent_id: None,
        role: MessageRole::Assistant,
        parts: vec![
            WirePart::RunMarker {
                run_id: "run-1".to_owned(),
                started_at: 1,
                ended_at: Some(2),
                duration_ms: Some(1),
                tool_count: Some(0),
            },
            WirePart::Text {
                text: "正文".to_owned(),
            },
        ],
        created_at: 1,
        run_id: None,
        status: None,
    };
    let stream = rebuild_message_stream(&[entry]).unwrap();
    assert_eq!(stream.len(), 1, "runMarker 不产消息");
    let Message::Assistant(m) = as_message(&stream[0]) else {
        panic!("应为 assistant");
    };
    assert_eq!(m.content.len(), 1, "只余 text part");
}

/// 提取 system 消息文本（断言辅助；非 system 即 panic）。
fn system_text(msg: &Message) -> String {
    let Message::System { content } = msg else {
        panic!("应为 system 消息");
    };
    content.clone()
}

/// 有 customPrompt：注入为 history 首条 system 消息（§6.2 次序首位），其余
/// 消息原序后移——模拟 start_chat 的 wire 出口调用形态。
#[test]
fn custom_prompt_injected_as_first_system_message() {
    let entries = vec![user("u1", "第一条"), user("u2", "第二条")];
    let (history, prompt) = split_prompt(&entries).unwrap();
    assert_eq!(history.len(), 1, "前置：无注入时 history 只有 u1");

    let injected = with_system_prefix(history, Some("始终用中文回复"));
    assert_eq!(injected.len(), 2, "注入后首条 system + 原 history");
    assert_eq!(system_text(as_message(&injected[0])), "始终用中文回复");
    assert_eq!(user_text(as_message(&injected[1])), "第一条");
    // prompt 不受注入影响
    assert_eq!(user_text(&prompt), "第二条");
}

/// 无/空白 customPrompt：零注入（history 原样返回，不产生空 system 消息）。
#[test]
fn empty_or_missing_custom_prompt_injects_nothing() {
    let entries = vec![user("u1", "第一条"), user("u2", "第二条")];
    let (history, _) = split_prompt(&entries).unwrap();
    let baseline_len = history.len();

    for prefix in [None, Some(""), Some("   \n\t ")] {
        let kept = with_system_prefix(history.clone(), prefix);
        assert_eq!(kept.len(), baseline_len, "前缀 {prefix:?} 应零注入");
        assert!(
            !matches!(as_message(&kept[0]), Message::System { .. }),
            "前缀 {prefix:?} 首条不应是 system"
        );
    }
}

/// 注入的空白归一：带首尾空白的前缀按 trim 后注入（与 AppConfig.normalize
/// 的归一口径双保险，防绕过配置入口的调用方带入空白前缀）。
#[test]
fn custom_prompt_is_trimmed_before_injection() {
    let injected = with_system_prefix(Vec::new(), Some("  保持简洁  "));
    assert_eq!(system_text(as_message(&injected[0])), "保持简洁");
}

// ---- 压缩边界重建（§6.2【M2】，04 号票）----

use crate::backend::wire::{
    CompactionBoundary, split_prompt_with_boundary, with_system_prefix_layers,
};

/// 历史序列：u1 → a1（含工具回合）→ u2（本次输入 prompt）。
fn compact_sample() -> Vec<MessageEntry> {
    vec![
        user("u1", "被压缩的早期内容"),
        assistant_with_tool_round("a1", "call_1", "read"),
        user("u2", "本次输入"),
    ]
}

/// 断言：cutEntryId（含）之前跳过、之后正常展开（工具配对完整）、prompt
/// 保留、摘要作为第三返回值交注入层。
#[test]
fn boundary_skips_before_cut_and_expands_after() {
    let entries = compact_sample();
    let boundary = CompactionBoundary {
        cut_entry_id: "u1".to_owned(),
        summary: "早期内容摘要".to_owned(),
    };
    let (history, prompt, summary) = split_prompt_with_boundary(&entries, Some(&boundary)).unwrap();
    // u1 被裁（摘要取代），a1 展开为 assistant + 工具结果 user 两条
    assert_eq!(history.len(), 2, "裁剪后 history：{history:?}");
    assert!(matches!(as_message(&history[0]), Message::Assistant(_)));
    assert!(matches!(as_message(&history[1]), Message::User { .. }));
    // 工具配对完整：a1 的 toolResult 以 user 工具结果消息展开
    if let Message::User { content } = as_message(&history[1]) {
        assert!(matches!(content.first(), Some(UserContent::ToolResult(_))));
    } else {
        panic!("第二条应为工具结果 user 消息");
    }
    // prompt = 本次输入，不受边界影响
    assert_eq!(user_text(as_message(&prompt)), "本次输入");
    assert_eq!(summary.as_deref(), Some("早期内容摘要"));

    // 无边界：全量展开（u1 + a1 的 assistant + 工具结果）
    let (full_history, _, summary) = split_prompt_with_boundary(&entries, None).unwrap();
    assert_eq!(full_history.len(), 3);
    assert!(summary.is_none(), "无边界无摘要层");
}

/// 断言：边界锚点不在活动路径上 → 显式拒绝（不静默全量展开——未经摘要的
/// 历史不得被静默裁掉的镜像不变量：不确定时不裁）。
#[test]
fn missing_cut_anchor_is_rejected_not_silently_expanded() {
    let entries = compact_sample();
    let boundary = CompactionBoundary {
        cut_entry_id: "不存在".to_owned(),
        summary: "x".to_owned(),
    };
    let err = split_prompt_with_boundary(&entries, Some(&boundary)).unwrap_err();
    assert!(err.0.contains("压缩边界锚点"), "错误面：{err}");
}

/// 断言：多层前缀层叠次序（§6.2 定稿）——customPrompt 在最前、压缩 summary
/// 紧随其后；空层零注入。
#[test]
fn layered_prefix_orders_custom_prompt_then_summary() {
    let entries = compact_sample();
    let (history, _, _) = split_prompt_with_boundary(
        &entries,
        Some(&CompactionBoundary {
            cut_entry_id: "u1".to_owned(),
            summary: "摘要层".to_owned(),
        }),
    )
    .unwrap();
    let layered = with_system_prefix_layers(history, [Some("自定义提示"), Some("摘要层")]);
    assert_eq!(system_text(as_message(&layered[0])), "自定义提示");
    assert_eq!(system_text(as_message(&layered[1])), "摘要层");
    assert!(matches!(as_message(&layered[2]), Message::Assistant(_)));

    // 空层零注入：customPrompt 未配置 + 无压缩边界 → 原样
    let (plain, _, _) = split_prompt_with_boundary(&entries, None).unwrap();
    let untouched = with_system_prefix_layers(plain, [None, None]);
    assert!(!matches!(as_message(&untouched[0]), Message::System { .. }));
}

// ---- image part 双形态重建（§6.2，17 号票）----

use crate::backend::wire::{
    ImageBlobData, ImageBlobMap, VisionPolicy, image_placeholder, rebuild_message_stream_visioned,
    split_prompt_visioned,
};
use std::collections::HashMap;

/// 造一条 user 消息的通用辅助（parts 自定义）。
fn user_parts(id: &str, parts: Vec<WirePart>) -> MessageEntry {
    MessageEntry {
        id: id.to_owned(),
        parent_id: None,
        role: MessageRole::User,
        parts,
        created_at: 1,
        run_id: None,
        status: None,
    }
}

/// 装配一个含一张图的 blob 查找表（hash "a".repeat(32) → png/base64 占位）。
fn blobs_one() -> ImageBlobMap {
    let mut map = HashMap::new();
    map.insert(
        "a".repeat(32),
        ImageBlobData {
            mime: "image/png".to_owned(),
            base64: "cG5nLWJ5dGVz".to_owned(),
        },
    );
    map
}

/// image part 的固定样本（hash = "a".repeat(32)，png）。
fn image_part() -> WirePart {
    WirePart::Image {
        blob_hash: "a".repeat(32),
        mime_type: "image/png".to_owned(),
    }
}

/// vision 模型：prompt 中的 image part 映射为多模态 content 块（base64 嵌入
/// + MIME 类型），文本 part 顺序保留。
#[test]
fn vision_prompt_maps_image_to_multimodal_block() {
    let entries = vec![user_parts(
        "u1",
        vec![
            WirePart::Text {
                text: "看这张图".to_owned(),
            },
            image_part(),
        ],
    )];
    let blobs = blobs_one();
    let policy = VisionPolicy {
        vision: true,
        blobs: &blobs,
    };
    let (_, prompt, _) = split_prompt_visioned(&entries, None, &policy).unwrap();
    let Message::User { content } = &*prompt else {
        panic!("应为 user 消息");
    };
    assert_eq!(content.len(), 2, "文本块 + 图片块");
    assert!(matches!(content[0], UserContent::Text(_)));
    let UserContent::Image(image) = &content[1] else {
        panic!("第二块应为图片");
    };
    let rig::message::DocumentSourceKind::Base64(b64) = &image.data else {
        panic!("图片数据应为 base64 载体");
    };
    assert_eq!(b64, "cG5nLWJ5dGVz");
    assert_eq!(
        image
            .media_type
            .as_ref()
            .map(rig::message::MimeType::to_mime_type),
        Some("image/png")
    );
}

/// 非 vision 模型：**历史**中的 image part 降级为明确占位文本（§6.2 逐字
/// 格式），纯文字继续（Q30）。
#[test]
fn non_vision_history_image_degrades_to_placeholder() {
    let history_img = user_parts("u1", vec![image_part()]);
    let prompt = user("u2", "改用文字描述一下");
    let entries = vec![history_img, prompt];
    let blobs = blobs_one();
    let policy = VisionPolicy {
        vision: false,
        blobs: &blobs,
    };
    let (history, _, _) = split_prompt_visioned(&entries, None, &policy).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(
        user_text(as_message(&history[0])),
        image_placeholder(&"a".repeat(32))
    );
}

/// 非 vision 模型：**本次新附图**（prompt 中的 image part）按参数错误拒绝
/// （Q30：新输入与持久历史后端区分，不能由前端把新图标成历史绕过检查）。
#[test]
fn non_vision_new_image_rejected() {
    let entries = vec![user_parts(
        "u1",
        vec![
            WirePart::Text {
                text: "看这张图".to_owned(),
            },
            image_part(),
        ],
    )];
    let blobs = blobs_one();
    let policy = VisionPolicy {
        vision: false,
        blobs: &blobs,
    };
    let err = split_prompt_visioned(&entries, None, &policy).unwrap_err();
    assert!(
        err.0.contains("不支持图片输入"),
        "错误应说明 vision 限制：{}",
        err.0
    );
}

/// vision 模型但 blob 缺失（会话 blob 目录被清等）：降级为占位文本，
/// 不拒绝请求（诚实降级，不静默丢上下文线索）。
#[test]
fn vision_with_missing_blob_degrades_to_placeholder() {
    let entries = vec![user_parts("u1", vec![image_part()])];
    let empty: ImageBlobMap = HashMap::new();
    let policy = VisionPolicy {
        vision: true,
        blobs: &empty,
    };
    let (_, prompt, _) = split_prompt_visioned(&entries, None, &policy).unwrap();
    assert_eq!(user_text(&prompt), image_placeholder(&"a".repeat(32)));
}

/// 旧调用面（split_prompt / rebuild_message_stream）对 image part 仍按参数
/// 错误拒绝——语义为"调用方未声明视觉能力"。
#[test]
fn legacy_split_prompt_rejects_image_part() {
    let entries = vec![user_parts("u1", vec![image_part()])];
    let err = split_prompt(&entries).unwrap_err();
    assert!(matches!(err, WireError(_)));
    let err = rebuild_message_stream(&entries).unwrap_err();
    assert!(matches!(err, WireError(_)));
}

/// visioned 重建流（rebuild_message_stream_visioned）：assistant 侧 image
/// part 照 runMarker 同类跳过（M2 无图片生成）。
#[test]
fn assistant_image_part_skipped_in_stream() {
    let entry = MessageEntry {
        id: "a1".to_owned(),
        parent_id: None,
        role: MessageRole::Assistant,
        parts: vec![
            image_part(),
            WirePart::Text {
                text: "回复正文".to_owned(),
            },
        ],
        created_at: 1,
        run_id: None,
        status: None,
    };
    let blobs = blobs_one();
    let policy = VisionPolicy {
        vision: true,
        blobs: &blobs,
    };
    let stream = rebuild_message_stream_visioned(&[entry], &policy).unwrap();
    assert_eq!(stream.len(), 1, "image part 跳过后只剩文本 assistant 消息");
    let Message::Assistant(turn) = &*stream[0] else {
        panic!("应为 assistant 消息");
    };
    assert_eq!(turn.content.len(), 1);
    assert!(matches!(
        turn.content[0],
        rig::message::AssistantContent::Text(_)
    ));
}
