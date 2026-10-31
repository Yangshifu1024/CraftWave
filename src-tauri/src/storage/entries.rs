//! 会话日志「条目」的 Rust 数据定义（条目类型、 WirePart）。
//!
//! 术语解释：「条目」是会话日志（JSONL 文件，每行一个 JSON 对象）中的一行记录；
//! 「WirePart」是消息里的一个内容片段（一段文字、一次工具调用等），前端与后端
//! 传输和存储共用同一种结构，所以叫 wire（传输线）中立 part。这些类型加上
//! serde 派生后，序列化结果就是日志文件的行格式，不需要手写转换代码。
//!
//! 命名纪律（+ ）：条目判别键用 `kind`（JSON 里靠这个字段区分行类型），
//! 负载字段用 camelCase（驼峰命名）；前端 TypeScript 侧的同形状定义由 ts-rs
//! 生成（`#[cfg_attr(test, derive(ts_rs::TS))]` → ui/src/ipc/generated/，
//! `?` 可选字段 = serde Option/缺省字段，线上可省略），跨语言一致性由
//! 契约测试守护（「契约测试」指用同一批样例数据在两种语言间往返序列化、
//! 比对字节是否一致的测试）。
//! 条目内容不可变（追加规则）：编辑消息、切换分支一律追加新行，
//! 本层不提供任何改写历史行的原语——这是崩溃恢复与多进程安全的基础。

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---- WirePart（M1 子集 + M2 image 变体）----
//
// 契约类型的 TS 声明由 ts-rs 生成（定案）：测试构建下派生 TS 并
// 由 ipc_manifest_test 的导出面统一写出 ui/src/ipc/generated/（cargo build 不
// 依赖生成步骤）。`#[cfg_attr(test, derive(ts_rs::TS), ts(...))]` 是统一写法：
// 非测试构建完全不见该派生，ts-rs 因此只需 dev-dependency。

/// 计划条目（planSnapshot part 载荷）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanItem {
    pub id: String,
    pub text: String,
    pub status: PlanItemStatus,
}

/// 计划条目状态（pending/in_progress/completed）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemStatus {
    Pending,
    InProgress,
    Completed,
}

/// wire 中立 part（D8 载体）：消息内容的组成片段，前端与后端共用同一形状。
/// camelCase 变体名与字段名对齐 TS 镜像。
#[cfg_attr(test, derive(ts_rs::TS))]
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
        #[cfg_attr(test, ts(optional))]
        ended_at: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        duration_ms: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(test, ts(optional))]
        tool_count: Option<u32>,
    },
    /// 图片内容引用（image part）：只存 blob 哈希与
    /// MIME 类型，**不存图片本体**——字节数据在会话专属 blob 目录
    /// `sessions/<sid>.imgblob/<hash>.bin`（内容寻址），JSONL 正文
    /// 不因附图膨胀。`blobHash` 是 SHA-256 前 32 hex 的内容寻址标识，
    /// `mimeType` 限 png/jpeg/webp/gif 白名单（后端写入时按魔数嗅探校验）。
    Image {
        blob_hash: String,
        mime_type: String,
    },
}

/// assistant 消息状态（assistant-ui 0.15.x `MessageStatus` 判别联合的镜像， `status?`）。
/// M1 只写入 complete（S4 实测：导入 running/requires-action 状态时前端会改写消息，complete 稳定）。
#[cfg_attr(test, derive(ts_rs::TS))]
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
        #[cfg_attr(test, ts(optional))]
        error: Option<Value>,
    },
}

/// requires-action 的原因（kebab-case：tool-calls / interrupt）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RequiresActionReason {
    ToolCalls,
    Interrupt,
}

/// complete 的原因（kebab-case：stop / unknown）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CompleteReason {
    Stop,
    Unknown,
}

/// incomplete 的原因（kebab-case：cancelled/tool-calls/length/content-filter/other/error）。
#[cfg_attr(test, derive(ts_rs::TS))]
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

// ---- 存储条目 ----

/// 消息角色。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

impl MessageRole {
    /// serde 字符串形态（snake_case，与序列化口径一致；摘要输入块标记用）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
        }
    }
}

/// message 条目载荷。同时是 `save_session_tree` 的 messages 元素形态
/// （前端树同步载荷，`parentId` null→省略字段，`status?` 仅 assistant）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub parent_id: Option<String>,
    pub role: MessageRole,
    pub parts: Vec<WirePart>,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub status: Option<MessageStatus>,
}

/// meta 条目：会话的轻量元数据行，追加在段尾或重要事件之后。
/// 采用追加式「最新赢」——不删除旧 meta 行，读取时以最后一条 meta 为准。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub active_leaf_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub title: Option<String>,
    pub revision: u64,
}

/// session 头条目（会话首行）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    pub v: u32,
    pub id: String,
    pub created_at: i64,
}

/// compaction 条目：上下文压缩边界——`summary` 是
/// 被裁历史（`cutEntryId` 之前的全部条目）的增量摘要文本，`tokensBefore`
/// 是压缩时的上下文占用估算（token 数）。条目自身不挂树（无 parentId，
/// 定稿：wire 重建按 cutEntryId 过滤、前端按投影面渲染分隔提示）。
///
/// id 规则（钉）：`{owner}:compact:{n}`——自动压缩 owner 为
/// 触发 run 的 runId，手动压缩 owner 为 `manual`；n 为该会话压缩序号
/// （跨 owner 连续递增，从 1 起）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredCompactionEntry {
    pub id: String,
    /// 压缩边界锚点：此 id（含）之前的条目被摘要取代，之后的正常展开。
    pub cut_entry_id: String,
    /// 压缩摘要正文（供 wire 重建注入为 system 前缀）。
    pub summary: String,
    /// 压缩时的上下文占用估算（token 数；估算器口径）。
    pub tokens_before: u64,
    pub created_at: i64,
}

/// 存储条目（会话 JSONL 每行一个，serde tag=kind）。
/// compaction 为 M2 首个新增变体：**这是本应用首次存储格式升级**，
/// 首次写入前必须成功创建一次可恢复备份（`storage::backup` 守卫——
/// 失败暂停写入并告知用户；旧版本读到本变体按损坏行处理，属预期的不前向兼容）。
/// branchSummary 仍属 M2 可选项，随分支需求另立。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredEntry {
    Session(SessionEntry),
    Message(MessageEntry),
    Meta(MetaEntry),
    Compaction(StoredCompactionEntry),
}
