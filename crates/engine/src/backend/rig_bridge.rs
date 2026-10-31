//! rig 流事件到帧的桥接（"流事件 → 帧"的映射面：
//! 文本/推理/工具参数三类增量，参数增量为半截 JSON 时也原样转发）。
//!
//! 名词解释：rig 是本项目使用的第三方大模型编排库；流事件（StreamEvent）
//! 是模型流式回复过程中的原始事件；帧（frame）是后端发给前端的事件消息。
//! rig 升级只应改到本文件与 `providers/`（防腐层原则）。
//!
//! callId 同源问题：工具参数还在流式传输时，rig 的事件只携带
//! part 序号（流内分片序号；模型真正的调用 id 要到该分片的 End 事件才
//! 确定），因此 toolStarted/toolArgs 帧先用桥接派生 id（`call_part_{轮}_{序号}`）
//! 承载；End 事件到达时登记"模型调用 id → 桥接 id"映射，主循环发
//! toolResult 帧时按模型调用 id 反查出同一个桥接 id——三类帧的 id 同源，
//! 前端工具卡片才能按 callId 把开始/参数/结果配对成一张卡。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rig::streaming::{Item, PartKind, StreamEvent};

use super::frames::ChatFrame;

/// 单轮流事件的桥接器：把 rig 流事件转换成轮内的帧序列。
#[derive(Default)]
pub struct RigEventBridge {
    /// 分片序号 → 桥接 callId（帧面使用的同源 id）。工具调用的 Start
    /// 事件带名时登记。
    open_tool_calls: BTreeMap<usize, String>,
    /// 已见过的文本分片序号（用于在 End 事件时收敛登记）。
    opened_text: BTreeSet<usize>,
    /// 模型调用 id → 桥接 callId（End 事件定稿时登记；发 toolResult 帧
    /// 时反查用）。
    model_call_ids: HashMap<String, String>,
}

/// 桥接产物：产出一帧，或该事件无需出帧。
pub enum Bridged {
    /// 转换得到的一帧。
    Frame(ChatFrame),
    /// 无需出帧（如 Start/End 事件本身不对应界面增量）。
    None,
}

/// RigEventBridge 的转换与映射查询。
impl RigEventBridge {
    /// 创建空桥接器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 用既有的"模型调用 id → 桥接 id"映射播种（审批挂起恢复场景：
    /// 挂起批的 toolStarted/toolArgs 帧已按旧映射发出，恢复后的
    /// toolResult 必须沿用同一映射才同源）。
    pub fn seed_model_call_ids(&mut self, map: HashMap<String, String>) {
        self.model_call_ids = map;
    }

    /// 取映射快照（挂起时随驱动状态带出，恢复时再播种回来）。
    pub fn model_call_ids(&self) -> &HashMap<String, String> {
        &self.model_call_ids
    }

    /// 转换一个 rig 流事件。`turn` 为帧的轮次字段（从 1 起）。
    pub fn bridge(&mut self, item: &Item<StreamEvent>, turn: u32) -> Bridged {
        let Item::Event(event) = item else {
            // 未建模的 provider 透传负载：不出帧（生产应记日志）
            return Bridged::None;
        };
        // 派生 id 带 turn 前缀：part 序号按响应各自从 0 计，跨轮会撞车
        // （第二轮 tool-call 的帧面 callId 不得复用第一轮的）。
        let derive = |part_index: usize| format!("call_part_{turn}_{part_index}");
        match event {
            StreamEvent::Start { part, kind, name } => match kind {
                PartKind::ToolCall => {
                    let call_id = derive(part.index());
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
                            .or_insert_with(|| derive(part.index()));
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
                    .or_insert_with(|| derive(part.index()))
                    .clone();
                // 参数增量 JSON 片段原样转发（半截 JSON 保留原样，前端以
                // 「参数准备中」态渲染）
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
                        .unwrap_or_else(|| derive(part.index()));
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

    /// 按模型调用 id 反查帧面 callId（callId 同源：主循环发 toolResult 帧
    /// 前调用；映射未命中——如 End 事件尚未到达的异常路径——回退用模型
    /// id 本身，保证帧仍能发出）。
    pub fn frame_call_id_for_model(&self, model_call_id: &str) -> String {
        self.model_call_ids
            .get(model_call_id)
            .cloned()
            .unwrap_or_else(|| model_call_id.to_owned())
    }
}
