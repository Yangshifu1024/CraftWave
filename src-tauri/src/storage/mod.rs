//! storage 模块（§2.2/§7）：会话 JSONL、索引、配置协调器、原子写、toolres 边车、
//! 项目注册表、UI 状态。
//!
//! 已落地：§6.1 条目类型、append-only 分段 JSONL 读写器（§7.2–7.4）、
//! 会话树存取引擎（save/load_session_tree 的 storage 层，10 号票）、
//! 配置协调器（§7.6，12 号票）、canonical 追加原语 + 会话索引 + toolres 边车
//! + 崩溃恢复（15 号票）、项目注册表（FR-10-01，16 号票）。

pub mod config_store;
pub mod entries;
pub mod index_store;
pub mod locks;
pub mod project_store;
pub mod segment;
pub mod session_tree;
pub mod toolres;

#[cfg(test)]
#[path = "index_store_test.rs"]
mod index_store_test;

pub use entries::{
    CompleteReason, IncompleteReason, MessageEntry, MessageRole, MessageStatus, MetaEntry,
    PlanItem, PlanItemStatus, RequiresActionReason, SessionEntry, StoredEntry, WirePart,
};
pub use index_store::{IndexError, SessionIndex, SessionMeta};
pub use locks::LockMap;
pub use project_store::{ProjectError, ProjectManifest, ProjectMeta, ProjectStore};
pub use segment::{SEGMETA_FILE, SegmentInfo, Segmeta, SessionLog};
pub use session_tree::{SessionStore, SessionTreeData, StorageError};
pub use toolres::{load_outcomes, write_outcome};

#[cfg(test)]
#[path = "project_store_test.rs"]
mod project_store_test;
#[cfg(test)]
#[path = "segment_test.rs"]
mod segment_test;
#[cfg(test)]
#[path = "session_tree_test.rs"]
mod session_tree_test;
