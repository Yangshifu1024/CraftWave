//! 会话日志「条目」的 Rust 数据定义（技术设计文档 §6.1 条目类型、§3.2 WirePart）。
//!
//! 术语解释：「条目」是会话日志（JSONL 文件，每行一个 JSON 对象）中的一行记录；
//! 「WirePart」是消息里的一个内容片段（一段文字、一次工具调用等），前端与后端
//! 传输和存储共用同一种结构，所以叫 wire（传输线）中立 part。这些类型加上
//! serde 派生后，序列化结果就是日志文件的行格式，不需要手写转换代码。
//!
//! 命名纪律（D14 + §3.1）：条目判别键用 `kind`（JSON 里靠这个字段区分行类型），
//! 负载字段用 camelCase（驼峰命名）；前端 TypeScript 侧的同形状定义在
//! `ui/src/ipc/types.ts`（「TS 镜像」），跨语言一致性由 §12.3 契约测试守护
//! （「契约测试」指用同一批样例数据在两种语言间往返序列化、比对字节是否一致的测试）。
//! 条目内容不可变（§6.1 追加规则）：编辑消息、重新生成、切换分支一律追加新行，
//! 本层不提供任何改写历史行的原语——这是崩溃恢复与多进程安全的基础。

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

/// wire 中立 part（§3.2 D8 载体）：消息内容的组成片段，前端与后端共用同一形状。
/// camelCase 变体名与字段名对齐 TS 镜像。
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
/// M1 只写入 complete（S4 实测：导入 running/requires-action 状态时前端会改写消息，complete 稳定）。
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

/// meta 条目（§6.1）：会话的轻量元数据行，追加在段尾或重要事件之后。
/// 采用追加式「最新赢」——不删除旧 meta 行，读取时以最后一条 meta 为准。
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
