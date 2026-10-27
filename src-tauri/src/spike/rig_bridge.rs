//! 【Spike】rig `StreamEvent`（provider 流事件）→ `ChatFrame` 的桥接。
//!
//! 对应技术设计文档 §4.3 的「MultiTurnStreamItem/流事件 → BackendEvent → 帧」
//! 映射面：Text/Reasoning/Arguments（工具参数增量，含半截 JSON 原样转发）。
//! rig 升级只应改到这里（防腐层 §4.1）。

use rig::streaming::{Item, PartKind, StreamEvent};

use super::frames::ChatFrame;

/// 单 turn 流事件桥：把 rig 流事件转成 turn 内的帧序列。
#[derive(Default)]
pub struct RigEventBridge {
    /// part 序号 → (callId, 工具名)。工具调用在 Start 事件命名后登记。
    open_tool_calls: std::collections::BTreeMap<usize, String>,
    /// 已见过的 text part（用于在 End 时收敛登记）。
    opened_text: std::collections::BTreeSet<usize>,
}

/// 桥接产物：一帧，或无需出帧。
pub enum Bridged {
    Frame(ChatFrame),
    None,
}

impl RigEventBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// 转换一个 rig 流事件。`turn` 为帧的 turn 字段（从 1 起）。
    pub fn bridge(&mut self, item: &Item<StreamEvent>, turn: u32) -> Bridged {
        let Item::Event(event) = item else {
            // 未建模的 provider 透传负载：spike 不出帧（生产应记日志）
            return Bridged::None;
        };
        match event {
            StreamEvent::Start { part, kind, name } => match kind {
                PartKind::ToolCall => {
                    let call_id = format!("call_part_{}", part.index());
                    if let Some(tool_name) = name {
                        self.open_tool_calls.insert(part.index(), call_id.clone());
                        Bridged::Frame(ChatFrame::ToolStarted {
                            call_id,
                            name: tool_name.to_string(),
                        })
                    } else {
                        // Start 未带名：暂不出帧，等首个 Arguments 事件补登记
                        self.open_tool_calls
                            .entry(part.index())
                            .or_insert_with(|| format!("call_part_{}", part.index()));
                        Bridged::None
                    }
                }
                PartKind::Text => {
                    self.opened_text.insert(part.index());
                    Bridged::None
                }
                _ => Bridged::None,
            },
            StreamEvent::Text { part, text } => {
                let _ = part;
                Bridged::Frame(ChatFrame::TextDelta {
                    turn,
                    delta: text.clone(),
                })
            }
            StreamEvent::Reasoning { part, text } => {
                let _ = part;
                Bridged::Frame(ChatFrame::ReasoningDelta {
                    turn,
                    delta: text.clone(),
                })
            }
            StreamEvent::Arguments { part, json } => {
                let call_id = self
                    .open_tool_calls
                    .entry(part.index())
                    .or_insert_with(|| format!("call_part_{}", part.index()))
                    .clone();
                // 参数增量 JSON 片段原样转发（半截 JSON 保留原样，前端以
                // 「参数准备中」态渲染——§10.2）
                Bridged::Frame(ChatFrame::ToolArgs {
                    call_id,
                    chunk: json.clone(),
                })
            }
            StreamEvent::End { part, content } => {
                // 工具调用的 End 事件不带参数增量，工具名/参数已全部经
                // Start/Arguments 到达；文本 part 的 End 同理。这里清理登记。
                self.open_tool_calls.remove(&part.index());
                let _ = content;
                Bridged::None
            }
        }
    }
}
