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
//! - image part（图片内容，§3.2【M2】，17 号票）按视觉策略双形态映射：
//!   vision 模型 → 多模态 content 块（blob 字节经 base64 嵌入，字节来自
//!   调用方装配的 [`ImageBlobMap`] 查找表——blob 读取是 IO，不进本纯函数）；
//!   非 vision 模型 → `[image: <hash>; 当前模型无法查看此图片]` 占位文本
//!   （历史图片降级，Q30：允许纯文字继续）。**本次新附图例外**：末条
//!   prompt（start_chat 消息序列的末条 user 消息）携带 image 且模型非
//!   vision 时按参数错误拒绝（Q30 定稿：新输入与持久历史后端区分，不能
//!   因历史有图拒绝纯文字请求，也不能由前端把新图标成历史绕过检查）。
//! - start_chat 传入的消息序列（前端活动路径的线性顺序，§6.3）末条 user
//!   消息即本轮 prompt（要发给模型的最新输入），其余为 history（历史
//!   上下文）。
//!
//! 不带视觉策略的 [`split_prompt`] / [`split_prompt_with_boundary`]（旧调用
//! 面）对 image part 仍按参数错误拒绝——语义为"调用方未声明视觉能力"。

use std::collections::HashMap;
use std::sync::Arc;

use rig::completion::Message;
use rig::message::{
    AssistantContent, AssistantMessage, CallId, DocumentSourceKind, Image, ImageMediaType,
    MimeType, ToolName, ToolResult, ToolResultContent, UserContent,
};

use crate::storage::{MessageEntry, MessageRole, WirePart};

/// wire 重建错误（错误码面为 E_INVALID_ARG，命令层转为 IpcError 返回前端）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError(pub String);

/// 带边界切分的返回面（history, prompt, 摘要注入层；`type_complexity` 收敛）。
pub type SplitWithBoundary = (Vec<Arc<Message>>, Arc<Message>, Option<String>);

/// WireError 的展示实现：直接输出内部错误文本。
impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// 一份已装配的图片 blob 数据（hash → mime + base64）：start_chat 命令层
/// 从会话 imgblob 目录读出后传入 wire 重建（IO 集中在装配面，重建保持
/// 纯函数可测）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageBlobData {
    /// MIME 类型（blob 读取时按魔数嗅探恢复，png/jpeg/gif/webp 白名单）。
    pub mime: String,
    /// 图片字节的 base64 编码（多模态 content 块直接嵌入）。
    pub base64: String,
}

/// 图片 blob 查找表（hash → 数据）。
pub type ImageBlobMap = HashMap<String, ImageBlobData>;

/// §6.2 非 vision 历史图片的占位文本（逐字格式）。
pub fn image_placeholder(hash: &str) -> String {
    format!("[image: {hash}; 当前模型无法查看此图片]")
}

/// 视觉策略（17 号票）：当前 run 模型是否具备视觉能力 + 已装配的 blob 查找表。
#[derive(Debug, Clone, Copy)]
pub struct VisionPolicy<'a> {
    /// 模型是否支持图片输入（ProviderModel.vision，§3.2）。
    pub vision: bool,
    /// 已装配的图片 blob（hash → mime/base64）。
    pub blobs: &'a ImageBlobMap,
}

/// 压缩边界（wire 重建面，§6.2【M2】）：史册最近 compaction 条目的锚点与
/// 摘要——`cut_entry_id`（含）之前的条目跳过（已被摘要取代），之后的正常
/// 展开；`summary` 由调用方注入为 system 前缀（customPrompt 之后）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionBoundary {
    pub cut_entry_id: String,
    pub summary: String,
}

/// 切分 prompt 与 history：末条 user 消息为 prompt（本轮发送给模型），
/// 其余按序重建为 history；assistant 条目内的工具结果拆为紧随的 user
/// 消息。空序列或末条不是 user 均拒绝。
pub fn split_prompt(
    entries: &[MessageEntry],
) -> Result<(Vec<Arc<Message>>, Arc<Message>), WireError> {
    let (history, prompt, _) = split_prompt_with_boundary(entries, None)?;
    Ok((history, prompt))
}

/// 带视觉策略的切分（§6.2 image part 双形态，17 号票）：vision 模型把
/// image part 映射为多模态 content 块（字节取自 `policy.blobs`）；非 vision
/// 模型把历史图片降级为占位文本、把**本次新附图**（末条 prompt 中的图片）
/// 按参数错误拒绝（Q30：新输入与持久历史后端区分）。
pub fn split_prompt_visioned(
    entries: &[MessageEntry],
    boundary: Option<&CompactionBoundary>,
    policy: &VisionPolicy<'_>,
) -> Result<SplitWithBoundary, WireError> {
    let Some(last) = entries.last() else {
        return Err(WireError("start_chat 消息序列为空".into()));
    };
    if last.role != MessageRole::User {
        return Err(WireError(format!(
            "start_chat 消息序列末条必须为 user（实际 {:?}）",
            last.role
        )));
    }
    // 新附图前置拦截（Q30）：非 vision 模型拒绝本次输入带图（历史不受影响）
    if !policy.vision && last.parts.iter().any(is_image_part) {
        return Err(WireError(
            "当前模型不支持图片输入：本次消息携带图片，请切换视觉模型或移除图片后发送".into(),
        ));
    }
    split_prompt_impl(entries, boundary, Some(policy))
}

/// 带压缩边界的切分（§6.2 压缩边界，04 号票）：边界存在时 `cutEntryId`
/// （含）之前的条目全部跳过、之后的正常展开，摘要作为第三返回值交由调用
/// 方注入（层叠次序 customPrompt → summary，[`with_system_prefix_layers`]）。
///
/// 不变量（§12.5 压缩往返属性）：未经摘要的历史不得被静默裁掉——边界锚点
/// 在序列中找不到时按参数错误拒绝（显式失败），不做静默全量展开。
pub fn split_prompt_with_boundary(
    entries: &[MessageEntry],
    boundary: Option<&CompactionBoundary>,
) -> Result<SplitWithBoundary, WireError> {
    split_prompt_impl(entries, boundary, None)
}

/// 切分实现（视觉策略可选）：`policy=None` = 旧调用面（image part 拒绝）。
fn split_prompt_impl(
    entries: &[MessageEntry],
    boundary: Option<&CompactionBoundary>,
    policy: Option<&VisionPolicy<'_>>,
) -> Result<SplitWithBoundary, WireError> {
    let Some(last) = entries.last() else {
        return Err(WireError("start_chat 消息序列为空".into()));
    };
    if last.role != MessageRole::User {
        return Err(WireError(format!(
            "start_chat 消息序列末条必须为 user（实际 {:?}）",
            last.role
        )));
    }
    let mut boundary_summary: Option<String> = None;
    let start = match boundary {
        Some(b) => {
            let Some(idx) = entries.iter().position(|e| e.id == b.cut_entry_id) else {
                return Err(WireError(format!(
                    "压缩边界锚点 {} 不在会话活动路径上（拒绝静默全量展开）",
                    b.cut_entry_id
                )));
            };
            boundary_summary = Some(b.summary.clone());
            idx + 1
        }
        None => 0,
    };
    let history = rebuild_message_stream_impl(&entries[start..entries.len() - 1], policy)?;
    let prompt = Arc::new(to_rig_user(last, policy)?);
    Ok((history, prompt, boundary_summary))
}

/// 是否 image part。
fn is_image_part(part: &WirePart) -> bool {
    matches!(part, WirePart::Image { .. })
}

/// 把消息序列重建为 rig 消息流（§6.2）：assistant 条目内 toolCall 后随的
/// toolResult 拆为紧随的 user 工具结果消息。
pub fn rebuild_message_stream(entries: &[MessageEntry]) -> Result<Vec<Arc<Message>>, WireError> {
    rebuild_message_stream_impl(entries, None)
}

/// 带视觉策略的重建（[`rebuild_message_stream`] 的 visioned 变体）。
pub fn rebuild_message_stream_visioned(
    entries: &[MessageEntry],
    policy: &VisionPolicy<'_>,
) -> Result<Vec<Arc<Message>>, WireError> {
    rebuild_message_stream_impl(entries, Some(policy))
}

/// 重建实现（视觉策略可选）。
fn rebuild_message_stream_impl(
    entries: &[MessageEntry],
    policy: Option<&VisionPolicy<'_>>,
) -> Result<Vec<Arc<Message>>, WireError> {
    let mut out = Vec::new();
    for entry in entries {
        match entry.role {
            MessageRole::User => out.push(Arc::new(to_rig_user(entry, policy)?)),
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
                            let args = serde_json::from_str(args_json)
                                .unwrap_or(serde_json::Value::Null);
                            content.push(AssistantContent::tool_call(
                                call_id.clone(),
                                tool_name,
                                args,
                            ));
                        }
                        WirePart::ToolResult { .. }
                        | WirePart::PlanSnapshot { .. }
                        | WirePart::RunMarker { .. }
                        // assistant 侧 image part：M2 无图片生成（多模态输出
                        // 不在本票），照 planSnapshot 同类跳过
                        | WirePart::Image { .. } => {}
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

/// ai_language 注入层（31 号票，§6.2 注入次序回写）：模型回复语言的常驻
/// 指引行。`config.ui.aiLanguage` 非空 → 「请始终使用 <ai_language> 回答」；
/// 空/未配置 → 跟随界面语言（zh-CN/en-US 各一行固定文案）；返回 None =
/// 不应注入（不会发生——空值也回退界面语言，仅参数异常防御）。
/// 纯函数无 IO，三态单测见 wire_test。
pub fn ai_language_layer(ai_language: Option<&str>, ui_language: &str) -> Option<String> {
    let target = match ai_language.map(str::trim).filter(|s| !s.is_empty()) {
        Some(custom) => custom.to_owned(),
        None => match ui_language {
            "en-US" => "English".to_owned(),
            _ => "简体中文".to_owned(),
        },
    };
    // 模板按界面语言选（发给模型的指令行——与 32 号票错误面口径同为
    // 「英文技术串/中文模板」双轨的模型侧文本）
    if ui_language == "en-US" {
        Some(format!("Always respond in {target}."))
    } else {
        Some(format!("请始终使用 {target} 回答。"))
    }
}

/// 系统级前缀注入（§6.2 注入次序，03 号票定稿）：`agent.customPrompt`
/// （FR-09-06）非空白时作为 history 首条 system 消息插入；空/未配置零注入
/// （原样返回）。注入不进消息史册，只在模型请求面生效。
pub fn with_system_prefix(history: Vec<Arc<Message>>, prefix: Option<&str>) -> Vec<Arc<Message>> {
    with_system_prefix_layers(history, [prefix])
}

/// 多层系统级前缀注入（§6.2 层叠次序定稿：customPrompt → 压缩 summary →
/// 当前计划，04 号票接通第二层；计划注入随工具系统工单）。按传入顺序把
/// 非空白层各插为一条 system 消息（先传的在最前）；全空层零注入。
pub fn with_system_prefix_layers<'a, I>(history: Vec<Arc<Message>>, layers: I) -> Vec<Arc<Message>>
where
    I: IntoIterator<Item = Option<&'a str>>,
{
    let prefixes: Vec<&str> = layers
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    if prefixes.is_empty() {
        return history;
    }
    let mut out = Vec::with_capacity(history.len() + prefixes.len());
    for text in prefixes {
        out.push(Arc::new(Message::System {
            content: text.to_owned(),
        }));
    }
    out.extend(history);
    out
}

/// 单条消息 → rig 的 user 消息（user 消息不允许携带 toolCall，遇到即报错；
/// 空内容时补一个空文本，避免无效消息）。视觉策略可选：None = image part
/// 按参数错误拒绝（旧调用面）；Some = 按 [`VisionPolicy`] 双形态映射。
fn to_rig_user(
    entry: &MessageEntry,
    policy: Option<&VisionPolicy<'_>>,
) -> Result<Message, WireError> {
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
            WirePart::Image { blob_hash, .. } => match policy {
                None => {
                    return Err(WireError(format!(
                        "消息 {} 携带 image part 但调用方未启用视觉重建（blob {}）",
                        entry.id, blob_hash
                    )));
                }
                Some(p) => content.push(image_user_content(blob_hash, p)),
            },
            WirePart::PlanSnapshot { .. } | WirePart::RunMarker { .. } => {}
        }
    }
    if content.is_empty() {
        content.push(UserContent::text(String::new()));
    }
    Ok(Message::User { content })
}

/// image part → rig 多模态 content 块或占位文本（§6.2 双形态，17 号票）：
/// vision 模型且有 blob → base64 多模态块；blob 缺失（会话 blob 目录被
/// 删除等）或非 vision 模型 → 明确占位文本（§6.2 逐字格式）。
fn image_user_content(blob_hash: &str, policy: &VisionPolicy<'_>) -> UserContent {
    if policy.vision
        && let Some(blob) = policy.blobs.get(blob_hash)
    {
        let media_type = ImageMediaType::from_mime_type(&blob.mime);
        return UserContent::Image(Image {
            data: DocumentSourceKind::Base64(blob.base64.clone()),
            media_type,
            detail: None,
            native: None,
        });
    }
    UserContent::text(image_placeholder(blob_hash))
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
