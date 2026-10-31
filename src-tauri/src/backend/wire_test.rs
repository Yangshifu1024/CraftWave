//! wire 重建的单元测试：文本/推理原样映射、toolCall 与 toolResult 按调用
//! id 配对拆分、prompt 与 history 切分、系统级前缀注入（customPrompt 有/无
//! 两态，03 号票）、非法形态（空序列/末条非 user/悬空 toolResult）拒绝。

use rig::completion::Message;
use rig::message::{ToolResultContent, UserContent};

use crate::backend::wire::{
    WireError, ai_language_layer, rebuild_message_stream, split_prompt, with_system_prefix,
    with_system_prefix_layers,
};
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

/// 提取 system 消息文本（断言辅助；非 system 即 panic）。
fn system_text(msg: &Message) -> String {
    let Message::System { content } = msg else {
        panic!("应为 system 消息");
    };
    content.clone()
}

/// 断言 prompt 与 history 切分及非法形态拒绝：末条 user 消息作为 prompt、
/// 其余重建为 history（工具结果拆成一条独立 user 消息）；runMarker/
/// planSnapshot 两类 part 被跳过（不产出消息）；悬空 toolResult（无配对
/// toolCall）、空序列、末条非 user 各以明确错误拒绝。
#[test]
fn split_prompt_shape_and_rejection_cases() {
    // 正常切分：u1 + (a1 的 assistant) + (a1 拆出的工具结果 user) = 3 条
    let entries = vec![
        user("u1", "第一条"),
        assistant_with_tool_round("a1", "call_1", "read"),
        user("u2", "再来一条"),
    ];
    let (history, prompt) = split_prompt(&entries).unwrap();
    assert_eq!(user_text(&prompt), "再来一条");
    assert_eq!(history.len(), 3);
    assert_eq!(user_text(as_message(&history[0])), "第一条");
    assert!(matches!(as_message(&history[1]), Message::Assistant(_)));
    let Message::User { content } = as_message(&history[2]) else {
        panic!("工具结果应为 user 消息");
    };
    assert_eq!(content.len(), 1, "toolResult 合并为一条 user 消息");

    // runMarker / planSnapshot 跳过：不产消息、其余内容不受影响
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

    // 悬空 toolResult：显式拒绝
    let dangling = MessageEntry {
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
    let entries = vec![user("u1", "hi"), dangling, user("u2", "再问")];
    assert_eq!(
        split_prompt(&entries),
        Err(WireError(
            "消息 a1 的 toolResult call_x 缺配对的 toolCall".to_owned()
        ))
    );
    // 空序列与末条非 user：拒绝
    assert_eq!(
        split_prompt(&[]),
        Err(WireError("start_chat 消息序列为空".to_owned()))
    );
    let assistant_only = vec![assistant_with_tool_round("a1", "call_1", "read")];
    assert!(
        matches!(split_prompt(&assistant_only), Err(WireError(_))),
        "末条非 user 应拒绝"
    );
}

/// 断言工具结果按调用 id 配对出工具名、ok 标志映射为 is_error 取反、摘要
/// 作为文本内容；失败结果（ok=false）映射 is_error=true。
#[test]
fn tool_result_pairing_and_error_mapping_cases() {
    // (ok, 期望 is_error)
    for (ok_flag, summary, want_error) in [
        (true, "文件共 3 行", false),
        (false, "E_INVALID_ARG: 参数错", true),
    ] {
        let mut entry = assistant_with_tool_round("a1", "call_9", "grep");
        entry.parts[3] = WirePart::ToolResult {
            call_id: "call_9".to_owned(),
            ok: ok_flag,
            summary: summary.to_owned(),
        };
        let stream = rebuild_message_stream(&[entry]).unwrap();
        assert_eq!(stream.len(), 2, "assistant + 工具结果 user");
        let Message::User { content } = as_message(stream.last().unwrap()) else {
            panic!("应有工具结果消息");
        };
        let UserContent::ToolResult(result) = &content[0] else {
            panic!("应为 ToolResult content");
        };
        assert_eq!(result.call.to_string(), "call_9", "调用 id 配对");
        assert_eq!(result.name.to_string(), "grep", "按调用 id 配对出工具名");
        assert_eq!(result.is_error, want_error, "ok={ok_flag} → is_error 取反");
        match &result.content[0] {
            ToolResultContent::Text(t) => assert_eq!(t.text, summary, "摘要作为文本内容"),
            other => panic!("应为文本工具结果：{other:?}"),
        }
    }
}

// ---- 压缩边界重建（§6.2【M2】，04 号票）----

use crate::backend::wire::{CompactionBoundary, split_prompt_with_boundary};

/// 历史序列：u1 → a1（含工具回合）→ u2（本次输入 prompt）。
fn compact_sample() -> Vec<MessageEntry> {
    vec![
        user("u1", "被压缩的早期内容"),
        assistant_with_tool_round("a1", "call_1", "read"),
        user("u2", "本次输入"),
    ]
}

/// 断言压缩边界重建：cutEntryId（含）之前跳过、之后正常展开（工具配对
/// 完整）、prompt 保留、摘要作为第三返回值交注入层；无边界全量展开；边界
/// 锚点不在活动路径上 → 显式拒绝（不静默全量展开——未经摘要的历史不得被
/// 静默裁掉的镜像不变量：不确定时不裁）。
#[test]
fn compaction_boundary_cases() {
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
    if let Message::User { content } = as_message(&history[1]) {
        assert!(
            matches!(content.first(), Some(UserContent::ToolResult(_))),
            "工具配对完整展开"
        );
    } else {
        panic!("第二条应为工具结果 user 消息");
    }
    assert_eq!(
        user_text(as_message(&prompt)),
        "本次输入",
        "prompt 不受边界影响"
    );
    assert_eq!(summary.as_deref(), Some("早期内容摘要"));

    // 无边界：全量展开（u1 + a1 的 assistant + 工具结果）
    let (full_history, _, summary) = split_prompt_with_boundary(&entries, None).unwrap();
    assert_eq!(full_history.len(), 3);
    assert!(summary.is_none(), "无边界无摘要层");

    // 锚点缺失：显式拒绝
    let boundary = CompactionBoundary {
        cut_entry_id: "不存在".to_owned(),
        summary: "x".to_owned(),
    };
    let err = split_prompt_with_boundary(&entries, Some(&boundary)).unwrap_err();
    assert!(err.0.contains("压缩边界锚点"), "错误面：{err}");
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

/// 断言 image part 双形态重建与降级各面（Q30）：vision 模型 prompt 中
/// image 映射为多模态 content 块（base64 嵌入 + MIME）；非 vision 模型
/// **历史**中的 image 降级为明确占位文本；**本次新附图**按参数错误拒绝
/// （不能由前端把新图标成历史绕过检查）；blob 缺失降级为占位文本不拒绝
/// （诚实降级）；旧调用面对 image part 仍按参数错误拒绝；visioned 重建流
/// 中 assistant 侧 image part 照 runMarker 同类跳过（M2 无图片生成）。
#[test]
fn vision_image_part_cases() {
    // ① vision：prompt 中 image 映射多模态块，文本 part 顺序保留
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

    // ② 非 vision：历史中的 image 降级为占位文本
    let entries = vec![
        user_parts("u1", vec![image_part()]),
        user("u2", "改用文字描述一下"),
    ];
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

    // ③ 非 vision：本次新附图按参数错误拒绝（Q30）
    let entries = vec![user_parts(
        "u1",
        vec![
            WirePart::Text {
                text: "看这张图".to_owned(),
            },
            image_part(),
        ],
    )];
    let err = split_prompt_visioned(&entries, None, &policy).unwrap_err();
    assert!(
        err.0.contains("不支持图片输入"),
        "错误应说明 vision 限制：{}",
        err.0
    );

    // ④ vision 但 blob 缺失：降级为占位文本，不拒绝请求
    let entries = vec![user_parts("u1", vec![image_part()])];
    let empty: ImageBlobMap = HashMap::new();
    let policy = VisionPolicy {
        vision: true,
        blobs: &empty,
    };
    let (_, prompt, _) = split_prompt_visioned(&entries, None, &policy).unwrap();
    assert_eq!(user_text(&prompt), image_placeholder(&"a".repeat(32)));

    // ⑤ 旧调用面（split_prompt / rebuild_message_stream）对 image part
    // 仍按参数错误拒绝——语义为"调用方未声明视觉能力"
    let entries = vec![user_parts("u1", vec![image_part()])];
    assert!(matches!(split_prompt(&entries), Err(WireError(_))));
    assert!(matches!(
        rebuild_message_stream(&entries),
        Err(WireError(_))
    ));

    // ⑥ visioned 重建流：assistant 侧 image part 跳过（M2 无图片生成）
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

// ---- 系统级前缀注入（03 号票 customPrompt 两态 + 31 号票 ai_language）----

/// 断言系统级前缀注入各面：有 customPrompt 注入为 history 首条 system
/// 消息（§6.2 次序首位）；无/空白 customPrompt 零注入（不产生空 system
/// 消息）；带首尾空白的前缀按 trim 后注入（与 AppConfig.normalize 归一
/// 口径双保险）；ai_language 三态（非空 → 自定义语言、空 → 跟随界面语言
/// zh-CN/en-US）；层叠次序（§6.2 定稿）——customPrompt → 项目指令 →
/// ai_language → 压缩 summary → 技能清单按传入序层叠，空层零注入。
#[test]
fn system_prefix_and_language_layer_cases() {
    // ① 有 customPrompt：注入为首条 system，其余原序后移
    let entries = vec![user("u1", "第一条"), user("u2", "第二条")];
    let (history, prompt) = split_prompt(&entries).unwrap();
    assert_eq!(history.len(), 1, "前置：无注入时 history 只有 u1");
    let injected = with_system_prefix(history, Some("始终用中文回复"));
    assert_eq!(injected.len(), 2, "注入后首条 system + 原 history");
    assert_eq!(system_text(as_message(&injected[0])), "始终用中文回复");
    assert_eq!(user_text(as_message(&injected[1])), "第一条");
    assert_eq!(user_text(&prompt), "第二条", "prompt 不受注入影响");

    // ② 无/空白 customPrompt：零注入
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

    // ③ 带首尾空白的前缀按 trim 后注入
    let injected = with_system_prefix(Vec::new(), Some("  保持简洁  "));
    assert_eq!(system_text(as_message(&injected[0])), "保持简洁");

    // ④ ai_language 三态：(ai_language, 界面语言, 期望固定行)
    for (ai, ui_lang, want) in [
        (Some("English"), "zh-CN", "请始终使用 English 回答。"),
        (None, "zh-CN", "请始终使用 简体中文 回答。"),
        (Some("   "), "zh-CN", "请始终使用 简体中文 回答。"), // 空白同 None
        (None, "en-US", "Always respond in English."),
    ] {
        let layer = ai_language_layer(ai, ui_lang)
            .unwrap_or_else(|| panic!("应产生注入层：ai={ai:?} ui={ui_lang}"));
        assert_eq!(layer, want, "ai={ai:?} ui={ui_lang}");
    }
    // en 文案无 CJK
    let layer = ai_language_layer(None, "en-US").expect("en 界面同样产生注入层");
    assert!(
        !layer
            .chars()
            .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)),
        "en 文案无 CJK"
    );

    // ⑤ 层叠次序（§6.2 回写口径）：ai_language 层位于 customPrompt/项目
    // 指令之后、压缩 summary 之前；空层零注入
    let ai = ai_language_layer(Some("English"), "zh-CN").unwrap();
    let history: Vec<_> = Vec::new();
    let out = with_system_prefix_layers(
        history,
        [
            Some("CUSTOM"),
            Some("INSTRUCTIONS"),
            Some(ai.as_str()),
            Some("SUMMARY"),
            Some("SKILLS"),
        ],
    );
    let texts: Vec<_> = out
        .iter()
        .map(|m| match m.as_ref() {
            Message::System { content } => content.clone(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(
        texts,
        vec![
            "CUSTOM",
            "INSTRUCTIONS",
            "请始终使用 English 回答。",
            "SUMMARY",
            "SKILLS"
        ]
    );

    // customPrompt 与压缩 summary 的层叠次序 + 空层零注入
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
    let (plain, _, _) = split_prompt_with_boundary(&entries, None).unwrap();
    let untouched = with_system_prefix_layers(plain, [None, None]);
    assert!(
        !matches!(as_message(&untouched[0]), Message::System { .. }),
        "空层零注入：customPrompt 未配置 + 无压缩边界 → 原样"
    );
}
