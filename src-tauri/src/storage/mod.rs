//! storage 模块：CraftWave 全部本地数据落盘能力的入口。
//!
//! 这里统一管理几类磁盘数据（术语解释：「条目」指会话日志里的一行记录；
//! 「JSONL」指每行一个 JSON 对象的文本文件格式；「分段」指把一个会话的日志
//! 切成多个编号递增的小文件；「原子写」指先写临时文件再改名，保证读者永远
//! 只看到完整文件、不会读到写了一半的内容）：
//!
//! - 会话日志（append-only 分段 JSONL，见 `segment` 与 `entries`）：
//!   只追加、永不改写历史行；每个会话一个目录，落在应用数据目录的会话区。
//! - 会话索引与会话树存取（`index_store`、`session_tree`）：索引是所有会话
//!   的摘要清单（标题、修订号等，方便列表页快速加载）；会话树负责把整棵
//!   消息树完整读出/写回。「修订号 revision」是一个只增不减的整数，每次
//!   保存会话都加一，用来识别哪个版本最新。
//! - canonical 追加原语、崩溃恢复（「canonical」指唯一的标准写入路径，所有
//!   写操作都必须经过它，避免多处写盘互相打架）。
//! - toolres 边车（`toolres`：「边车 sidecar」指和主日志放在一起、存放某一类
//!   附属数据的独立文件，这里存工具调用的结果内容）。
//! - 配置协调器（`config_store`：合并默认配置、全局配置、项目配置三层）。
//! - 项目注册表（`project_store`：记录用户打开过哪些项目目录）。
//! - 文件锁（`locks`：防止多个进程同时写同一个文件）。
//!
//! 下方 `pub use` 把各子模块的公共类型重新导出，crate 其他位置统一从
//! `crate::storage::…` 引用，不必关心具体藏在哪个子文件里。

pub mod backup;
// 产物登记（sessions/<sid>.artifacts.json）：write/edit 成功
// 落盘的路径登记面（去重计数 + 存在探测查询）。
pub mod artifacts;
pub mod config_store;
pub mod entries;
pub mod imgblob;
pub mod index_db;
pub mod index_store;
pub mod locks;
pub mod prefs;
pub mod project_store;
pub mod segment;
pub mod session_tree;
pub mod stats;
pub mod toolres;
// 统一信任存储（trust/project.json）：项目资源 + MCP 启动
// 信任两类记录收敛为单一文件单一实现（TrustStore trait + FileTrustStore）。
pub mod trust;

#[cfg(test)]
#[path = "trust_test.rs"]
mod trust_test;

#[cfg(test)]
#[path = "index_store_test.rs"]
mod index_store_test;

#[cfg(test)]
#[path = "prefs_test.rs"]
mod prefs_test;

#[cfg(test)]
#[path = "artifacts_test.rs"]
mod artifacts_test;

pub use artifacts::{
    ArtifactEntry, ArtifactRow, ArtifactsFile, ListArtifactsResult, artifacts_path,
    list as list_artifacts_rows, load as load_artifacts, record as record_artifact_entry,
};
pub use entries::{
    CompleteReason, IncompleteReason, MessageEntry, MessageRole, MessageStatus, MetaEntry,
    PlanItem, PlanItemStatus, RequiresActionReason, SessionEntry, StoredCompactionEntry,
    StoredEntry, WirePart,
};
pub use imgblob::{
    BLOB_HASH_HEX_LEN, MAX_BLOB_BYTES, blob_hash, get_blob, imgblob_dir, is_valid_blob_hash,
    put_blob, sniff_image_mime,
};
pub use index_db::{
    INDEX_DB_VERSION, IndexDb, StatsCacheRow, index_db_path, open as open_index_db,
};
pub use index_store::{IndexError, SessionIndex, SessionMeta};
pub use locks::LockMap;
pub use project_store::{ProjectError, ProjectManifest, ProjectMeta, ProjectStore};
pub use segment::{SEGMETA_FILE, SegmentInfo, Segmeta, SessionLog};
pub use session_tree::{SessionStore, SessionTreeData, StorageError};
pub use stats::{
    DailyStats, ModelDayStats, ModelRunUsage, RunUsage, STATS_DIR, STATS_FORMAT_VERSION,
    STATS_RETENTION_DAYS, SessionUsage, StatsStore, UsageDetailStore, UsageRecord, is_valid_day,
    load_session_usage, save_session_usage, shift_day, usage_detail_path,
};
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
