//! 询问/审批协议（§3.2 AskPayload/AskAnswer，FR-05-02/03/04）。
//!
//! 两条使用路径共用同一协议与 [`AskSlot`]：
//! - **审批门**（§5.4 ④）：ApprovalGate 判挂起 → host 构造 `kind:"approval"`
//!   载荷（diff 预览 / command + fenceReason）→ 挂起整只 run 等待应答；
//! - **ask 工具**（§8.3 Interactive）：executor 构造 `kind:"question"` 载荷 →
//!   注册槽等待应答，answers 回喂为工具结果。
//!
//! 应答统一经 `resolve_ask` 命令（§3.3 #22）→ oneshot 唤醒。

use tokio::sync::oneshot;

/// 询问/审批载荷（§3.2；M1 实装 question/approval 两形态，
/// mcpTrust/projectTrust 属 M2）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AskPayload {
    /// ask 工具的问卷。
    Question {
        title: String,
        questions: Vec<AskQuestion>,
    },
    /// 审批门（confirm_each 写/网络类，或围栏 Confirm）。
    Approval {
        title: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        diff: Option<ApprovalDiff>,
        #[serde(skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        fence_reason: Option<String>,
        options: Vec<AskOption>,
    },
}

/// 单个问题（§3.2 question.questions 元素）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskQuestion {
    pub id: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<AskChoice>>,
    pub allow_free_text: bool,
}

/// 问题选项（id + 展示文案）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskChoice {
    pub id: String,
    pub label: String,
}

/// 审批选项（§3.2 approval.options 元素）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskOption {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<AskMode>,
}

/// 审批动作模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskMode {
    AllowOnce,
    /// M1 不实装会话级记忆（commandAllowlist 接线后续票），键面先钉死。
    AlwaysSession,
    Deny,
}

/// diff 预览（§3.2 approval.diff；write=新文件全 add，edit=单 hunk 替换）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDiff {
    pub path: String,
    pub hunks: Vec<DiffHunk>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffLineKind {
    Add,
    Del,
    Ctx,
}

impl AskPayload {
    /// 推荐选项 id（autoConfirm 超时自动选它，§5.4）。
    pub fn recommended_option_id(&self) -> Option<&str> {
        match self {
            AskPayload::Question { .. } => None,
            AskPayload::Approval { options, .. } => options
                .iter()
                .find(|o| o.recommended == Some(true))
                .map(|o| o.id.as_str()),
        }
    }
}

/// 询问/审批应答（§3.2 AskAnswer）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AskAnswer {
    /// qid -> 选项 id 或自由文本。
    Question {
        answers: std::collections::BTreeMap<String, String>,
    },
    /// 选项 id（allow_once/deny）+ 可选编辑后的命令（M1 不实装编辑，键面钉死）。
    Approval {
        option_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        edited_command: Option<String>,
    },
}

/// 未决 ask 槽（§5.1 RunHandle.ask 的元素）：oneshot 回执。
#[derive(Debug)]
pub struct AskSlot {
    pub ask_id: String,
    pub session_id: String,
    pub run_id: String,
    /// 载荷形态（question/approval），关闭事件与 UI 路由用。
    pub kind: &'static str,
    pub tx: oneshot::Sender<AskAnswer>,
}
