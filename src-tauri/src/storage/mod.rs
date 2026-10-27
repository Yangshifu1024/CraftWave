//! storage 模块（§2.2/§7）：会话 JSONL、索引、配置协调器、原子写、toolres 边车、UI 状态。
//!
//! 本切片（10 号票）落地：§6.1 条目类型、append-only 分段 JSONL 读写器（§7.2–7.4）、
//! 会话树存取引擎（save/load_session_tree 的 storage 层）。索引（sessions/index.json）、
//! toolres 边车、配置协调器、UI 状态由后续票接入。

pub mod entries;
pub mod locks;
pub mod segment;
pub mod session_tree;

pub use entries::{
    CompleteReason, IncompleteReason, MessageEntry, MessageRole, MessageStatus, MetaEntry,
    PlanItem, PlanItemStatus, RequiresActionReason, SessionEntry, StoredEntry, WirePart,
};
pub use locks::LockMap;
pub use segment::{SEGMETA_FILE, SegmentInfo, Segmeta, SessionLog};
pub use session_tree::{SessionStore, SessionTreeData, StorageError};

#[cfg(test)]
#[path = "segment_test.rs"]
mod segment_test;
#[cfg(test)]
#[path = "session_tree_test.rs"]
mod session_tree_test;
