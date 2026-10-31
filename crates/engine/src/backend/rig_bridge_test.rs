//! 流事件桥接器的单元测试：文本与参数增量透传（半截 JSON 保留）、
//! "模型调用 id ↔ 帧面 id"同源映射的登记与反查、跨轮 id 不冲突、
//! 未带名 Start 的占位登记（OpenAI 边缘形态）。

use rig::message::{AssistantContent, ToolCall, ToolFunction, ToolName};
use rig::streaming::{Item, Part, PartKind, StreamEvent};

use super::rig_bridge::RigEventBridge;
use crate::backend::frames::ChatFrame;

/// 造一个工具调用的 End 事件：携带定稿模型调用 id 的 rig 消息形态
/// （工具名与参数在此定稿）。
fn tool_end_event(part: usize, model_call_id: &str) -> Item<StreamEvent> {
    let call = ToolCall::new(
        rig::message::CallId::from_wire(model_call_id),
        ToolFunction::new(
            ToolName::new("read".to_owned()).expect("tool name"),
            serde_json::json!({}),
        ),
    );
    Item::Event(StreamEvent::End {
        part: Part::new(part as u32),
        content: AssistantContent::ToolCall(call),
    })
}

/// 造一个工具调用的 Start 事件（带工具名）。
fn tool_start(part: usize, name: &str) -> Item<StreamEvent> {
    Item::Event(StreamEvent::Start {
        part: Part::new(part as u32),
        kind: PartKind::ToolCall,
        name: Some(ToolName::new(name.to_owned()).expect("tool name")),
    })
}

/// 造一个工具参数增量事件。
fn tool_args(part: usize, json: &str) -> Item<StreamEvent> {
    Item::Event(StreamEvent::Arguments {
        part: Part::new(part as u32),
        json: json.to_owned(),
    })
}

/// 造一个文本增量事件。
fn text_delta(part: usize, text: &str) -> Item<StreamEvent> {
    Item::Event(StreamEvent::Text {
        part: Part::new(part as u32),
        text: text.to_owned(),
    })
}

/// 断言文本与参数增量原样透传出帧（半截 JSON 保留），且参数帧与
/// toolStarted 帧使用同一桥接 id。
#[test]
fn text_and_args_deltas_passthrough_partial_json_kept() {
    let mut b = RigEventBridge::new();
    let mut frames = Vec::new();
    for item in [
        text_delta(0, "我来读"),
        tool_start(1, "read"),
        tool_args(1, "{\"pa"),
        tool_args(1, "th\": \"/a.toml\"}"),
    ] {
        if let super::rig_bridge::Bridged::Frame(f) = b.bridge(&item, 1) {
            frames.push(f);
        }
    }
    assert_eq!(
        frames,
        vec![
            ChatFrame::TextDelta {
                turn: 1,
                delta: "我来读".into()
            },
            ChatFrame::ToolStarted {
                call_id: "call_part_1_1".into(),
                name: "read".into()
            },
            ChatFrame::ToolArgs {
                call_id: "call_part_1_1".into(),
                chunk: "{\"pa".into()
            },
            ChatFrame::ToolArgs {
                call_id: "call_part_1_1".into(),
                chunk: "th\": \"/a.toml\"}".into()
            },
        ],
        "参数帧与 toolStarted 帧面同源（桥接派生 id）"
    );
}

/// 断言桥接 id 映射面：End 定稿模型调用 id 登记后可反查（toolResult 同源
/// 用）、同轮多工具一一对应、跨轮 part 序号复用不撞帧面 id（无
/// turn 前缀会撞同一帧面 id，前端卡片按 id 合并会丢卡片）、未命中映射回退
/// 模型 id 本身（异常路径容错）、未带名 Start 不出帧且首个参数增量补登记
/// （OpenAI 首个分片只带序号的边缘形态）。
#[test]
fn bridge_id_mapping_and_edge_cases() {
    let mut b = RigEventBridge::new();
    // 轮 1：两个工具分片（各分片各得其所）；轮 2：part 1 复用（跨轮撞车面）
    for item in [tool_start(1, "read"), tool_args(1, "{}")] {
        let _ = b.bridge(&item, 1);
    }
    let _ = b.bridge(&tool_end_event(1, "call_A"), 1);
    for item in [tool_start(2, "grep"), tool_args(2, "{}")] {
        let _ = b.bridge(&item, 1);
    }
    let _ = b.bridge(&tool_end_event(2, "call_B"), 1);
    for item in [tool_start(1, "read"), tool_args(1, "{}")] {
        let _ = b.bridge(&item, 2);
    }
    let _ = b.bridge(&tool_end_event(1, "call_t2"), 2);

    assert_eq!(
        b.frame_call_id_for_model("call_A"),
        "call_part_1_1",
        "模型 call id 反查到帧面桥接 id（轮 1 分片 1，toolResult 同源用）"
    );
    assert_eq!(
        b.frame_call_id_for_model("call_B"),
        "call_part_1_2",
        "同轮多工具一一对应（分片 2）"
    );
    assert_eq!(
        b.frame_call_id_for_model("call_t2"),
        "call_part_2_1",
        "轮 2 的 part 1 与轮 1 不同 id（无 turn 前缀会撞帧面 id 丢卡片）"
    );
    assert_eq!(
        b.frame_call_id_for_model("call_unknown"),
        "call_unknown",
        "未命中映射回退模型 id 本身（异常路径容错）"
    );

    // 未带工具名的 Start 不出帧；首个参数增量事件补登记桥接 id
    let anonymous = Item::Event(StreamEvent::Start {
        part: Part::new(3),
        kind: PartKind::ToolCall,
        name: None,
    });
    let mut frames = Vec::new();
    if let super::rig_bridge::Bridged::Frame(f) = b.bridge(&anonymous, 1) {
        frames.push(f);
    }
    assert!(frames.is_empty(), "未命名的 Start 不出帧");
    if let super::rig_bridge::Bridged::Frame(f) = b.bridge(&tool_args(3, "{\"x\":1}"), 1) {
        frames.push(f);
    }
    let ChatFrame::ToolArgs { call_id, .. } = &frames[0] else {
        panic!("应为 toolArgs 帧");
    };
    assert_eq!(call_id, "call_part_1_3", "首参数事件补登记桥接 id");
}
