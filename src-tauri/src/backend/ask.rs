//! 询问与审批协议（技术设计文档 §3.2 的 AskPayload/AskAnswer）。
//!
//! 名词解释：ask（询问）指模型向用户发起的提问，必须等用户回答后 run
//! 才能继续；审批是询问的一种，特指模型要执行敏感操作（写文件、执行命令）
//! 时，先暂停整个 run 等用户批准。
//!
//! 两条使用路径共用同一套协议与 [`AskSlot`]（ask 槽 = 存放当前未决询问的
//! 共享单元）：
//! - **审批门**（§5.4 第 ④ 步）：ApprovalGate 判定需要挂起 → 宿主命令层
//!   构造 `kind:"approval"` 载荷（含改动预览 diff，或命令与围栏原因；
//!   围栏 = 命令工具的硬性安全底线）→ 暂停整个 run 等待应答；
//! - **ask 工具**（§8.3 交互类工具）：执行器构造 `kind:"question"` 载荷 →
//!   注册到槽位等待应答，用户的回答作为工具结果回喂给模型。
//!
//! 两条路径的应答都经 `resolve_ask` 命令（§3.3 第 22 条）进入，通过
//! oneshot 通道（只能发送一次消息的异步通道）唤醒等待方。

use tokio::sync::oneshot;

/// 询问/审批载荷（§3.2；08 号票起实装 question/approval/mcpTrust 三形态，
/// projectTrust 项目资源信任随 09 号票实装）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AskPayload {
    /// ask 工具的问卷（一组带选项的问题）。
    Question {
        /// 问卷标题（界面卡片展示）。
        title: String,
        /// 问题列表。
        questions: Vec<AskQuestion>,
    },
    /// 审批门载荷（confirm_each 档的写/联网类操作，或围栏要求确认的命令）。
    Approval {
        /// 审批标题（界面卡片展示）。
        title: String,
        /// 改动预览（diff = 逐行对比的新旧差异；写文件类审批展示）。
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        diff: Option<ApprovalDiff>,
        /// 待批准的命令（命令类审批展示）。
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        command: Option<String>,
        /// 围栏要求确认的原因（如"命令含写信号"）。
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        fence_reason: Option<String>,
        /// HTML 预览全文（FR-12-04，14 号票）：write/edit 的目标为 .html/.htm
        /// 且内容 ≤50,000 字符时由宿主审批载荷附带——审批卡「查看预览」入口
        /// 以 render_html 同款沙箱渲染（不落盘不解析，同工具约定）。
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        preview_html: Option<String>,
        /// 用户可选的操作列表（如仅本次允许/拒绝）。
        options: Vec<AskOption>,
    },
    /// MCP 启动信任（TOFU，§9.7/§3.2 字段级逐字；08 号票）：首次连接未
    /// 信任服务器前的确认。应答走 [`AskAnswer::Approval`]（optionId = 选项
    /// id：allow_once/remember/deny）——协议面不新增应答形态。
    McpTrust {
        /// 配置里的服务器名。
        server: String,
        /// 传输方式（stdio / http）。
        transport: McpTransportKind,
        /// 启动指纹（`scope|projectId|name|canonicalPath|sha256(config)` 的
        /// SHA-256，展示前 16 位即可）。
        fingerprint: String,
        /// 启动命令或 URL（stdio = command+args 拼接；HTTP = url）。
        command_or_url: String,
        /// 三选项（allow_once / remember / deny）。
        options: Vec<AskOption>,
    },
    /// 项目资源信任（§9.8 字段级，09 号票实装）：首次使用未信任技能
    /// （SKILL.md 及随附脚本、模板整体，Q21）前的确认。应答同走
    /// [`AskAnswer::Approval`]（optionId = allow_once/remember/deny）。
    ProjectTrust {
        /// 资源名（当前 = 技能名；24 号票扩展 memory 等资源时沿用本形态）。
        resource_name: String,
        /// 信任作用域（"global" | "project"；内置技能豁免不询问）。
        scope: SkillTrustScope,
        /// 整目录版本指纹（Q21 [`crate::knowledge::skills::version_fingerprint`]，
        /// 展示前 16 位）。
        fingerprint: String,
        /// 三选项（allow_once / remember / deny）。
        options: Vec<AskOption>,
    },
}

/// 项目资源信任的作用域（§9.8：全局 `~/.craftwave/skills/` 与项目
/// `<home>/.craftwave/skills/` 均属信任资源；内置豁免）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum SkillTrustScope {
    /// 全局作用域（`<data>/skills/`）。
    Global,
    /// 项目作用域（`<项目主目录>/.craftwave/skills/`）。
    Project,
}

/// 应用级询问槽（AppState.app_asks 的元素；09 号票由 08 号票的 MCP 专用
/// `mcp_asks`/`PendingTrustAsk` 泛化而来——应用级询问不属任何 run，统一挂
/// 本槽表，`resolve_ask` 命令在 run 槽未命中时落到这里）：ask_id → 原始
/// 载荷 + 应答通道（oneshot 一次性通道）。
pub struct AppAskSlot {
    /// 原始载荷（mcpTrust / projectTrust 等应用级形态）。
    pub payload: AskPayload,
    /// 应答发送端。
    pub tx: oneshot::Sender<AskAnswer>,
}

/// MCP 传输方式（§3.2 mcpTrust.transport："stdio" | "http"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum McpTransportKind {
    /// stdio 子进程传输。
    Stdio,
    /// streamable HTTP 传输。
    Http,
}

/// 单个问题（§3.2 question.questions 元素）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AskQuestion {
    /// 问题 id（应答按它对号入座）。
    pub id: String,
    /// 问题正文。
    pub text: String,
    /// 可选的候选项列表（None = 纯自由文本题）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub options: Option<Vec<AskChoice>>,
    /// 是否允许在选项之外自由输入文字。
    pub allow_free_text: bool,
}

/// 问题选项（id + 展示文案）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AskChoice {
    /// 选项 id（应答回传它）。
    pub id: String,
    /// 界面展示的选项文案。
    pub label: String,
}

/// 审批选项（§3.2 approval.options 元素）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AskOption {
    /// 选项 id（应答回传它，如 allow_once/deny）。
    pub id: String,
    /// 界面展示的选项文案。
    pub label: String,
    /// 是否为推荐选项（自动确认超时时默认选它）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub recommended: Option<bool>,
    /// 选该项后生效的模式（仅本次放行/本会话总放行/拒绝）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub mode: Option<AskMode>,
}

/// 审批动作模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum AskMode {
    /// 仅本次放行。
    AllowOnce,
    /// 本会话内总是放行（当前阶段不实装会话级记忆，命令白名单的接线为
    /// 后续工单，协议字段先固定下来）。
    AlwaysSession,
    /// 拒绝。
    Deny,
}

/// 改动预览（§3.2 approval.diff；write = 新文件全部为新增行，edit = 单段替换）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDiff {
    /// 目标文件路径。
    pub path: String,
    /// 改动分段列表。
    pub hunks: Vec<DiffHunk>,
}

/// diff 中的一段连续改动（hunk = 差异对比里带上下文的一小块）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    /// 段头（形如 `@@ -1,3 +1,4 @@` 的位置信息）。
    pub header: String,
    /// 该段内的逐行差异。
    pub lines: Vec<DiffLine>,
}

/// diff 中的单行。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    /// 行类别（新增/删除/上下文）。
    pub kind: DiffLineKind,
    /// 行内容。
    pub text: String,
}

/// diff 行类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub enum DiffLineKind {
    /// 新增行。
    Add,
    /// 删除行。
    Del,
    /// 上下文行（未改动，仅用于定位改动位置）。
    Ctx,
}

/// AskPayload 的固有方法实现。
impl AskPayload {
    /// 推荐选项的 id（自动确认超时时选它，§5.4）；问卷形态没有推荐选项，
    /// 返回 None。mcpTrust/projectTrust 同样返回 None——启动信任与项目资源
    /// 信任不参与 autoConfirm 自动批准（高危外部资源永远过人，FR-08-04
    /// 同精神）。
    pub fn recommended_option_id(&self) -> Option<&str> {
        match self {
            AskPayload::Question { .. }
            | AskPayload::McpTrust { .. }
            | AskPayload::ProjectTrust { .. } => None,
            AskPayload::Approval { options, .. } => options
                .iter()
                .find(|o| o.recommended == Some(true))
                .map(|o| o.id.as_str()),
        }
    }
}

/// 询问/审批应答（§3.2 AskAnswer）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AskAnswer {
    /// 问卷应答：问题 id → 选中的选项 id 或自由输入的文本。
    Question {
        answers: std::collections::BTreeMap<String, String>,
    },
    /// 审批应答：选中的选项 id（如 allow_once/deny）+ 可选的编辑后命令
    /// （当前阶段不实装命令编辑，字段先固定在协议里）。
    Approval {
        option_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        edited_command: Option<String>,
    },
}

/// 未决 ask 槽（§5.1 中 RunHandle.ask 存放的元素）：记录询问的元信息，
/// 并持有 oneshot 通道的发送端（一次性异步通道的发送侧），应答到达时
/// 用它唤醒等待中的 run。`payload` 留存开启时的原始载荷——WebView 重连
/// 后的待审批/询问恢复查询（§5.6，§3.3 #59c `session_pending_ask`）据此
/// 展示原始操作及参数（05 号票）。
#[derive(Debug)]
pub struct AskSlot {
    /// 本次询问的唯一 id（resolve_ask 命令按它路由应答）。
    pub ask_id: String,
    /// 所属会话 id。
    pub session_id: String,
    /// 所属 run id。
    pub run_id: String,
    /// 载荷形态（question/approval），关闭事件与前端界面路由用。
    pub kind: &'static str,
    /// 开启时的原始载荷（approval 含 diff/命令/围栏原因与选项，question
    /// 含问卷）；恢复查询的原样返回面。
    pub payload: AskPayload,
    /// oneshot 通道发送端：把用户应答发回等待中的 run。
    pub tx: oneshot::Sender<AskAnswer>,
}
