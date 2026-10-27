//! §6.1 存储条目与 §3.2 WirePart 的 Rust 侧定义（serde 序列化即 JSONL 行格式）。
//!
//! 命名纪律（D14 + §3.1）：条目判别键 `kind`，负载字段 camelCase；TS 镜像在
//! `ui/src/ipc/types.ts`，跨语言一致性由 §12.3 契约测试（fixture 往返）守护。
//! 条目内容不可变（§6.1 追加规则）：编辑/重生成/分支一律追加新行，本层无改写原语。

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---- §3.2 WirePart（M1 子集；image 属 M2）----

/// 计划条目（planSnapshot part 载荷，FR-06）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub id: String,
    pub text: String,
    pub status: PlanItemStatus,
}

/// 计划条目状态（§3.2：pending/in_progress/completed）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemStatus {
    Pending,
    InProgress,
    Completed,
}

/// wire 中立 part（§3.2 D8 载体）。camelCase 变体名与字段名对齐 TS 镜像。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WirePart {
    Text {
        text: String,
    },
    Reasoning {
        text: String,
    },
    ToolCall {
        call_id: String,
        name: String,
        args_json: String,
    },
    ToolResult {
        call_id: String,
        ok: bool,
        summary: String,
    },
    PlanSnapshot {
        items: Vec<PlanItem>,
    },
    RunMarker {
        run_id: String,
        started_at: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        ended_at: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_count: Option<u32>,
    },
}

/// assistant 消息状态（assistant-ui 0.15.x `MessageStatus` 判别联合的镜像，§6.1 `status?`）。
/// M1 只落 complete（S4 实测：import 会把 running/requires-action 改写，complete 稳定）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum MessageStatus {
    Running,
    RequiresAction {
        reason: RequiresActionReason,
    },
    Complete {
        reason: CompleteReason,
    },
    Incomplete {
        reason: IncompleteReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<Value>,
    },
}

/// requires-action 的原因（kebab-case：tool-calls / interrupt）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RequiresActionReason {
    ToolCalls,
    Interrupt,
}

/// complete 的原因（kebab-case：stop / unknown）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompleteReason {
    Stop,
    Unknown,
}

/// incomplete 的原因（kebab-case：cancelled/tool-calls/length/content-filter/other/error）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IncompleteReason {
    Cancelled,
    ToolCalls,
    Length,
    ContentFilter,
    Other,
    Error,
}

// ---- §6.1 存储条目 ----

/// 消息角色（§3.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

/// message 条目载荷（§6.1）。同时是 `save_session_tree` 的 messages 元素形态
/// （§6.3：前端树同步载荷，`parentId` null→省略字段，`status?` 仅 assistant）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub role: MessageRole,
    pub parts: Vec<WirePart>,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<MessageStatus>,
}

/// meta 条目（§6.1：段尾/事件后追加的轻量元数据行，追加式「最新赢」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_leaf_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub revision: u64,
}

/// session 头条目（§6.1：会话首行）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    pub v: u32,
    pub id: String,
    pub created_at: i64,
}

/// §6.1 存储条目（会话 JSONL 每行一个，serde tag=kind）。
/// compaction / branchSummary 属 M2，暂不定义变体（未知 kind 读入即报损坏，M1 无前向兼容负担）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredEntry {
    Session(SessionEntry),
    Message(MessageEntry),
    Meta(MetaEntry),
}
