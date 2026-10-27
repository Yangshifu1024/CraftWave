//! wire 中立消息 → rig Message 重建（§6.2 的 M1 子集）。
//!
//! - text/reasoning 原样映射；
//! - toolCall + toolResult 按 callId 配对：toolCall 进 assistant content，
//!   toolResult 合并为紧随的 user 工具结果消息（§6.2「配对合并为模型工具消息」）；
//! - planSnapshot（计划注入随工具系统票）、runMarker 跳过；
//! - start_chat 的消息序列（前端活动路径线性序，§6.3）末条 user 即本轮
//!   prompt，其余为 history。
//!
//! image part 属 M2，出现即参数错误。

use std::sync::Arc;

use rig::completion::Message;
use rig::message::{
    AssistantContent, AssistantMessage, CallId, ToolName, ToolResult, ToolResultContent,
    UserContent,
};

use crate::storage::{MessageEntry, MessageRole, WirePart};

/// wire 重建错误（E_INVALID_ARG 面，命令层转 IpcError）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError(pub String);

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 重建结果：`(history, prompt)`——末条 user 消息为 prompt（本轮发送），
/// 其余按序重建为 history（assistant 条目内的工具结果拆为紧随 user 消息）。
pub fn split_prompt(
    entries: &[MessageEntry],
) -> Result<(Vec<Arc<Message>>, Arc<Message>), WireError> {
    let Some(last) = entries.last() else {
        return Err(WireError("start_chat 消息序列为空".into()));
    };
    if last.role != MessageRole::User {
        return Err(WireError(format!(
            "start_chat 消息序列末条必须为 user（实际 {:?}）",
            last.role
        )));
    }
    let history = rebuild_message_stream(&entries[..entries.len() - 1])?;
    let prompt = Arc::new(to_rig_user(last)?);
    Ok((history, prompt))
}

/// 消息序列重建为 rig 消息流（§6.2）：assistant 条目内 toolCall 后随的
/// toolResult 拆为紧随的 user 工具结果消息。
pub fn rebuild_message_stream(entries: &[MessageEntry]) -> Result<Vec<Arc<Message>>, WireError> {
    let mut out = Vec::new();
    for entry in entries {
        match entry.role {
            MessageRole::User => out.push(Arc::new(to_rig_user(entry)?)),
            MessageRole::Assistant => {
                let mut content = Vec::new();
                for part in &entry.parts {
                    match part {
                        WirePart::Text { text } => {
                            content.push(AssistantContent::text(text.clone()));
                        }
                        WirePart::Reasoning { text } => {
                            content.push(AssistantContent::reasoning(text.clone()));
                        }
                        WirePart::ToolCall {
                            call_id,
                            name,
                            args_json,
                        } => {
                            let tool_name = ToolName::new(name.clone())
                                .map_err(|e| WireError(format!("非法工具名 {name}：{e}")))?;
                            // argsJson 解析失败按 null 交 rig 容错（ToolFunction::new(null) → {}）
                            let args =
                                serde_json::from_str(args_json).unwrap_or(serde_json::Value::Null);
                            content.push(AssistantContent::tool_call(
                                call_id.clone(),
                                tool_name,
                                args,
                            ));
                        }
                        WirePart::ToolResult { .. }
                        | WirePart::PlanSnapshot { .. }
                        | WirePart::RunMarker { .. } => {}
                    }
                }
                if !content.is_empty() {
                    out.push(Arc::new(Message::Assistant(AssistantMessage::new(content))));
                }
                let tool_results = tool_result_contents(entry)?;
                if !tool_results.is_empty() {
                    out.push(Arc::new(Message::User {
                        content: tool_results,
                    }));
                }
            }
            MessageRole::System => {
                out.push(Arc::new(Message::System {
                    content: join_text(&entry.parts),
                }));
            }
        }
    }
    Ok(out)
}

fn to_rig_user(entry: &MessageEntry) -> Result<Message, WireError> {
    let mut content = Vec::new();
    for part in &entry.parts {
        match part {
            WirePart::Text { text } => content.push(UserContent::text(text.clone())),
            WirePart::Reasoning { text } => {
                // 用户侧 reasoning 非模型输入面，降级为占位文本（不丢上下文线索）
                content.push(UserContent::text(format!("[thinking] {text}")));
            }
            WirePart::ToolResult {
                call_id,
                ok,
                summary,
            } => {
                content.push(UserContent::ToolResult(ToolResult {
                    call: CallId::from_wire(call_id.clone()),
                    name: tool_name_for_result(entry, call_id)?,
                    content: vec![ToolResultContent::text(summary.clone())],
                    is_error: !ok,
                }));
            }
            WirePart::ToolCall { .. } => {
                return Err(WireError(format!(
                    "user 消息 {} 不应携带 toolCall part",
                    entry.id
                )));
            }
            WirePart::PlanSnapshot { .. } | WirePart::RunMarker { .. } => {}
        }
    }
    if content.is_empty() {
        content.push(UserContent::text(String::new()));
    }
    Ok(Message::User { content })
}

/// 收集条目内全部 toolResult part 为 user 工具结果 content（name 按同条目
/// 前置 toolCall 的 callId 配对，§6.2）。
fn tool_result_contents(entry: &MessageEntry) -> Result<Vec<UserContent>, WireError> {
    let mut out = Vec::new();
    for part in &entry.parts {
        if let WirePart::ToolResult {
            call_id,
            ok,
            summary,
        } = part
        {
            out.push(UserContent::ToolResult(ToolResult {
                call: CallId::from_wire(call_id.clone()),
                name: tool_name_for_result(entry, call_id)?,
                content: vec![ToolResultContent::text(summary.clone())],
                is_error: !ok,
            }));
        }
    }
    Ok(out)
}

/// 按同条目前置 toolCall part 配对出工具名。
fn tool_name_for_result(entry: &MessageEntry, call_id: &str) -> Result<ToolName, WireError> {
    for part in &entry.parts {
        if let WirePart::ToolCall {
            call_id: id, name, ..
        } = part
            && id == call_id
        {
            return ToolName::new(name.clone())
                .map_err(|e| WireError(format!("非法工具名 {name}：{e}")));
        }
    }
    Err(WireError(format!(
        "消息 {} 的 toolResult {call_id} 缺配对的 toolCall",
        entry.id
    )))
}

fn join_text(parts: &[WirePart]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            WirePart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
