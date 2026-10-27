//! rig `StreamEvent`（provider 流事件）→ `ChatFrame` 的桥接（§4.3「流事件 →
//! BackendEvent → 帧」映射面：Text/Reasoning/Arguments（工具参数增量，含半截
//! JSON 原样转发））。rig 升级只应改到这里（防腐层 §4.1）。
//!
//! callId 同源（§3.4，S2 实证约定）：工具参数流式增量阶段，rig 的流事件只携带
//! part 序号（模型 call id 要到该 part 的 End 事件才定稿），因此 `toolStarted`/
//! `toolArgs` 帧以桥接派生 id（`call_part_{n}`）承载；End 事件到达时登记
//! 「模型 call id → 桥接 id」映射，RunLoop 在 CallTools 步骤发 `toolResult` 帧时
//! 按模型 call id 反查同一桥接 id——帧面三帧同源、且与装配器产出的模型调用
//! 一一对应，镜像工具卡可按 callId 配对（差异说明见 13 号票 Answer）。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rig::streaming::{Item, PartKind, StreamEvent};

use super::frames::ChatFrame;

/// 单 turn 流事件桥：把 rig 流事件转成 turn 内的帧序列。
#[derive(Default)]
pub struct RigEventBridge {
    /// part 序号 → 桥接 callId（帧面同源 id）。工具调用在 Start 事件命名后登记。
    open_tool_calls: BTreeMap<usize, String>,
    /// 已见过的 text part（用于在 End 时收敛登记）。
    opened_text: BTreeSet<usize>,
    /// 模型 call id → 桥接 callId（End 事件定稿时登记；toolResult 反查用）。
    model_call_ids: HashMap<String, String>,
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
            // 未建模的 provider 透传负载：不出帧（生产应记日志）
            return Bridged::None;
        };
        match event {
            StreamEvent::Start { part, kind, name } => match kind {
                PartKind::ToolCall => {
                    let call_id = format!("call_part_{}", part.index());
                    if let Some(tool_name) = name {
                        // 幂等：wire 层在 close 阶段可能补发 Start（同 part），
                        // 已登记的 part 不重发 toolStarted（帧面不重复）。
                        if let std::collections::btree_map::Entry::Vacant(slot) =
                            self.open_tool_calls.entry(part.index())
                        {
                            slot.insert(call_id.clone());
                            Bridged::Frame(ChatFrame::ToolStarted {
                                call_id,
                                name: tool_name.to_string(),
                            })
                        } else {
                            Bridged::None
                        }
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
                // 工具调用的 End 定稿了模型 call id：登记映射供 toolResult 反查
                if let rig::message::AssistantContent::ToolCall(call) = content {
                    let bridged = self
                        .open_tool_calls
                        .get(&part.index())
                        .cloned()
                        .unwrap_or_else(|| format!("call_part_{}", part.index()));
                    self.model_call_ids
                        .insert(call.id.to_string(), bridged.clone());
                    self.open_tool_calls.insert(part.index(), bridged);
                }
                self.open_tool_calls.remove(&part.index());
                let _ = content;
                Bridged::None
            }
        }
    }

    /// 模型 call id → 帧面 callId（§3.4 同源：RunLoop 发 toolResult 帧前反查；
    /// 映射未命中（如 End 事件尚未到达的异常路径）回退模型 id 本身）。
    pub fn frame_call_id_for_model(&self, model_call_id: &str) -> String {
        self.model_call_ids
            .get(model_call_id)
            .cloned()
            .unwrap_or_else(|| model_call_id.to_owned())
    }
}
