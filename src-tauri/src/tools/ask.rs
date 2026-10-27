//! ask 工具（§8.3 Interactive 行）：向用户提问，answer 回喂为工具结果。
//!
//! 参数校验与 AskPayload(question) 构造是纯逻辑（本文件）；等待
//! resolve_ask 的挂起由 executor 承担（AskSlot + ask:opened 事件）。

use serde::Deserialize;

use crate::backend::ask::{AskChoice, AskPayload, AskQuestion};
use crate::safety::tier::Capability;
use crate::util::errors::ToolError;

use super::{ToolAnnotations, ToolSpec};

/// ask 工具参数（§8.3：{title, questions:[…]}）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskArgs {
    pub title: String,
    pub questions: Vec<AskQuestionArg>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskQuestionArg {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub options: Option<Vec<AskChoiceArg>>,
    pub allow_free_text: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskChoiceArg {
    pub id: String,
    pub label: String,
}

/// 校验并构造 AskPayload(kind=question)。无效问卷（空标题/空问题/选项
/// 缺 id/既无选项又不容自由文本）在执行前拒绝。
pub fn build_payload(args: &AskArgs) -> Result<AskPayload, ToolError> {
    if args.title.trim().is_empty() {
        return Err(ToolError::invalid_arg("ask 标题不能为空"));
    }
    if args.questions.is_empty() {
        return Err(ToolError::invalid_arg("ask 至少一个问题"));
    }
    let mut questions = Vec::with_capacity(args.questions.len());
    for q in &args.questions {
        if q.id.trim().is_empty() || q.text.trim().is_empty() {
            return Err(ToolError::invalid_arg("ask 问题 id 与 text 不能为空"));
        }
        let options = match &q.options {
            Some(opts) if opts.is_empty() => {
                return Err(ToolError::invalid_arg(format!(
                    "问题 {} 的 options 为空列表（无选项请省略该字段）",
                    q.id
                )));
            }
            Some(opts) => {
                let mut seen = std::collections::HashSet::new();
                let choices = opts
                    .iter()
                    .map(|o| {
                        if o.id.trim().is_empty() {
                            return Err(ToolError::invalid_arg("选项 id 不能为空"));
                        }
                        if !seen.insert(o.id.as_str()) {
                            return Err(ToolError::invalid_arg(format!(
                                "问题 {} 选项 id 重复：{}",
                                q.id, o.id
                            )));
                        }
                        Ok(AskChoice {
                            id: o.id.clone(),
                            label: o.label.clone(),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Some(choices)
            }
            None => None,
        };
        if options.is_none() && !q.allow_free_text {
            return Err(ToolError::invalid_arg(format!(
                "问题 {} 既无选项又不容自由文本（allowFreeText=false 须给 options）",
                q.id
            )));
        }
        questions.push(AskQuestion {
            id: q.id.clone(),
            text: q.text.clone(),
            options,
            allow_free_text: q.allow_free_text,
        });
    }
    Ok(AskPayload::Question {
        title: args.title.clone(),
        questions,
    })
}

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "ask",
        description: "向用户提出问题并等待回答。适合需要用户决策或补充信息的场景：\
每个问题可带选项（用户点选）和/或允许自由文本。回答会作为工具结果返回。"
            .to_string(),
        capability: Capability::Interactive,
        params_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "questions": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "text": { "type": "string" },
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "id": { "type": "string" },
                                        "label": { "type": "string" }
                                    },
                                    "required": ["id", "label"],
                                    "additionalProperties": false
                                }
                            },
                            "allowFreeText": { "type": "boolean" }
                        },
                        "required": ["id", "text", "allowFreeText"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["title", "questions"],
            "additionalProperties": false
        }),
        annotations: ToolAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: false,
            open_world_hint: false,
        },
    }
}
