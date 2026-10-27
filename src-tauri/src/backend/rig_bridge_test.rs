//! 流事件桥接器的单元测试：文本与参数增量透传（半截 JSON 保留）、
//! "模型调用 id ↔ 帧面 id"同源映射的登记与反查、跨轮 id 不冲突。

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

/// 断言 End 事件登记"模型调用 id → 桥接 id"映射并可反查；未命中映射时
/// 回退用模型 id 本身（异常路径容错）。
#[test]
fn end_registers_model_call_mapping_reverse_lookup() {
    let mut b = RigEventBridge::new();
    let _ = b.bridge(&tool_start(1, "read"), 1);
    let _ = b.bridge(&tool_args(1, "{}"), 1);
    // End 定稿模型 call id
    let _ = b.bridge(&tool_end_event(1, "call_model_9"), 1);

    assert_eq!(
        b.frame_call_id_for_model("call_model_9"),
        "call_part_1_1",
        "模型 call id 反查到帧面桥接 id（toolResult 同源用）"
    );
    assert_eq!(
        b.frame_call_id_for_model("call_unknown"),
        "call_unknown",
        "未命中映射回退模型 id 本身（异常路径容错）"
    );
}

/// 断言同一轮内多个工具调用的映射一一对应（各分片各得其所）。
#[test]
fn multi_tool_calls_map_one_to_one() {
    let mut b = RigEventBridge::new();
    for item in [
        tool_start(1, "read"),
        tool_args(1, "{}"),
        tool_end_event(1, "call_A"),
        tool_start(2, "grep"),
        tool_args(2, "{}"),
        tool_end_event(2, "call_B"),
    ] {
        let _ = b.bridge(&item, 1);
    }
    assert_eq!(b.frame_call_id_for_model("call_A"), "call_part_1_1");
    assert_eq!(b.frame_call_id_for_model("call_B"), "call_part_1_2");
}

/// 断言未带工具名的 Start 事件不出帧，首个参数增量事件到来时补登记
/// 桥接 id（OpenAI 首个分片只带序号的边缘形态）。
#[test]
fn anonymous_start_placeholder_first_args_registers() {
    let mut b = RigEventBridge::new();
    // Start 未带名（OpenAI 首个 tool_calls chunk 只有 index 的边缘形态）
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

/// 断言派生 id 带轮次前缀：两轮各自的分片 1 不会撞同一个帧面 id
/// （前端卡片按 id 合并，撞车会丢卡片）。
#[test]
fn derived_ids_no_collision_across_turns() {
    // 14 号票修正：part 序号按响应各自从 0 计，两轮各自的 part 1 若无
    // turn 前缀会撞同一帧面 id（前端卡片 Map 合并丢失）。
    let mut b = RigEventBridge::new();
    let _ = b.bridge(&tool_start(1, "read"), 1);
    let _ = b.bridge(&tool_end_event(1, "call_t1"), 1);
    let _ = b.bridge(&tool_start(1, "grep"), 2);
    let _ = b.bridge(&tool_end_event(1, "call_t2"), 2);
    assert_eq!(
        b.frame_call_id_for_model("call_t1"),
        "call_part_1_1",
        "第一轮 part 1"
    );
    assert_eq!(
        b.frame_call_id_for_model("call_t2"),
        "call_part_2_1",
        "第二轮 part 1 —— 与第一轮不同 id"
    );
}
