//! wire 中立消息到 rig 消息的重建（技术设计文档 §6.2 的当前阶段子集）。
//!
//! 名词解释：wire 中立消息是存储层定义的通用消息格式（MessageEntry/WirePart，
//! 不依赖任何模型 SDK，便于持久化与迁移）；重建指把历史消息从存储格式
//! 还原成 rig 库的 Message 对象，供下一轮模型调用使用。
//!
//! 映射规则：
//! - 文本与推理内容原样映射；
//! - toolCall 与 toolResult 按调用 id 配对：toolCall 进 assistant 消息，
//!   toolResult 合并为紧随其后的 user 工具结果消息（§6.2"配对合并为模型
//!   工具消息"）；
//! - planSnapshot（计划快照）与 runMarker（run 边界标记）跳过不映射
//!   （计划注入随工具系统工单接入）；
//! - start_chat 传入的消息序列（前端活动路径的线性顺序，§6.3）末条 user
//!   消息即本轮 prompt（要发给模型的最新输入），其余为 history（历史
//!   上下文）。
//!
//! image part（图片内容）属后续阶段，出现即按参数错误拒绝。

use std::sync::Arc;

use rig::completion::Message;
use rig::message::{
    AssistantContent, AssistantMessage, CallId, ToolName, ToolResult, ToolResultContent,
    UserContent,
};

use crate::storage::{MessageEntry, MessageRole, WirePart};

/// wire 重建错误（错误码面为 E_INVALID_ARG，命令层转为 IpcError 返回前端）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError(pub String);

/// WireError 的展示实现：直接输出内部错误文本。
impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 切分 prompt 与 history：末条 user 消息为 prompt（本轮发送给模型），
/// 其余按序重建为 history；assistant 条目内的工具结果拆为紧随的 user
/// 消息。空序列或末条不是 user 均拒绝。
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

/// 把消息序列重建为 rig 消息流（§6.2）：assistant 条目内 toolCall 后随的
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

/// 系统级前缀注入（§6.2 注入次序首位，03 号票）：`agent.customPrompt`
/// （FR-09-06）非空白时作为 history 首条 system 消息插入（层叠次序
/// customPrompt → 压缩 summary → 当前计划，后两层随对应票在同一切入点续接）；
/// 空/未配置零注入（原样返回）。注入不进消息史册，只在模型请求面生效。
pub fn with_system_prefix(history: Vec<Arc<Message>>, prefix: Option<&str>) -> Vec<Arc<Message>> {
    let Some(text) = prefix.map(str::trim).filter(|t| !t.is_empty()) else {
        return history;
    };
    let mut out = Vec::with_capacity(history.len() + 1);
    out.push(Arc::new(Message::System {
        content: text.to_owned(),
    }));
    out.extend(history);
    out
}

/// 单条消息 → rig 的 user 消息（user 消息不允许携带 toolCall，遇到即报错；
/// 空内容时补一个空文本，避免无效消息）。
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

/// 按同条目内前置的 toolCall 配对出工具名（同调用 id 才算配对成功）。
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

/// 拼接条目内的全部文本 part（system 消息降为一整段文本，换行分隔）。
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
