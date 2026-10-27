//! 会话树存取引擎（§6.3/§6.4/§7）：`save_session_tree` / `load_session_tree` 的 Rust 侧实现。
//!
//! 职责（§6.3/§6.4）：revision 乐观并发校验（不符 E_SESSION_REV）、既有节点内容
//! 校验（同 id 内容变 → E_SESSION_REV）、仅追加缺失节点（父先子后由前端
//! `thread.export()` 导出序保证，此处再校验父存在性）、追加 meta 行推进 revision。
//! Tauri 命令层（host）只做校验/转调（§2.2 依赖方向 host → storage）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::entries::{MessageEntry, MetaEntry, SessionEntry, StoredEntry};
use super::locks::LockMap;
use super::segment::SessionLog;

/// 会话存储器：数据根 + 会话锁表（Tauri `State` 托管）。
pub struct SessionStore {
    root: PathBuf,
    locks: LockMap,
}

/// 存储引擎错误（命令层映射为 D14 `{code,message}`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageError {
    /// §6.4：树同步基线过期或既有节点内容漂移。
    RevisionConflict { expected: u64, actual: u64 },
    /// 目标会话不存在。
    NotFound(String),
    /// 参数校验失败（空 id、非法会话 id、孤儿节点、未知活动叶等）。
    InvalidArg(String),
    /// 磁盘 IO / 序列化失败（§3.6 P0 注册表暂无对应码，命令层映射 E_STORAGE_IO）。
    Io(String),
}

/// `load_session_tree` 返回（§6.1 定稿结构：headId=activeLeafId + messages 含 `status?`）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTreeData {
    pub session_id: String,
    pub messages: Vec<MessageEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_leaf_id: Option<String>,
    pub revision: u64,
}

impl SessionStore {
    /// 以指定数据根构造（测试注入临时目录；生产为 `~/.craftwave`，§7.1）。
    pub fn with_root(root: PathBuf) -> Self {
        Self {
            root,
            locks: LockMap::default(),
        }
    }

    /// 生产数据根（§7.1）：`~/.craftwave`；无法定位主目录时退回系统临时目录。
    pub fn default_root() -> PathBuf {
        // std::env::home_dir 自 1.85 起解除废弃（返回 Option，修复空 HOME 行为）
        match std::env::home_dir() {
            Some(home) => home.join(".craftwave"),
            None => std::env::temp_dir().join("craftwave-data"),
        }
    }

    /// `save_session_tree`（§3.3 #26 / §6.3 / §6.4）：全树同步，仅追加缺失节点。
    ///
    /// 返回推进后的 revision。首存（无既有文件）要求 `base_revision == 0`，
    /// 并自动写入 session 头条目；每次成功推进 revision 并追加 meta 行。
    pub fn save_session_tree(
        &self,
        session_id: &str,
        messages: &[MessageEntry],
        active_leaf_id: Option<&str>,
        base_revision: u64,
    ) -> Result<u64, StorageError> {
        validate_session_id(session_id)?;
        if messages.iter().any(|m| m.id.trim().is_empty()) {
            return Err(StorageError::InvalidArg("message.id 不能为空".into()));
        }
        let mutex = self.locks.get(session_id);
        let _guard = mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut log = SessionLog::open(self.histories_dir().join(session_id))
            .map_err(|e| StorageError::Io(e.to_string()))?;

        let current = log.revision().unwrap_or(0);
        if base_revision != current {
            return Err(StorageError::RevisionConflict {
                expected: base_revision,
                actual: current,
            });
        }

        // 既有节点规范化串（键序=声明序，等价内容哈希比对，§6.3）
        let mut known: HashMap<&str, String> = HashMap::new();
        for entry in log.entries() {
            if let StoredEntry::Message(m) = entry {
                known.insert(
                    m.id.as_str(),
                    canonical(m).map_err(|e| StorageError::Io(e.to_string()))?,
                );
            }
        }

        let mut batch: Vec<StoredEntry> = Vec::with_capacity(messages.len() + 2);
        for message in messages {
            let canonical = canonical(message).map_err(|e| StorageError::Io(e.to_string()))?;
            match known.get(message.id.as_str()) {
                Some(stored) if *stored != canonical => {
                    // §6.3：同 id 内容漂移 → E_SESSION_REV
                    return Err(StorageError::RevisionConflict {
                        expected: base_revision,
                        actual: current,
                    });
                }
                Some(_) => {} // 已存在且一致：跳过（幂等全量同步）
                None => {
                    if let Some(parent) = message.parent_id.as_deref()
                        && !known.contains_key(parent)
                    {
                        return Err(StorageError::InvalidArg(format!(
                            "消息 {} 的父节点 {parent} 不存在（导出序必须父先子后）",
                            message.id
                        )));
                    }
                    known.insert(message.id.as_str(), canonical);
                    batch.push(StoredEntry::Message(message.clone()));
                }
            }
        }

        if let Some(leaf) = active_leaf_id
            && !known.contains_key(leaf)
        {
            return Err(StorageError::InvalidArg(format!(
                "activeLeafId {leaf} 不在树中"
            )));
        }

        if log.is_empty() {
            batch.insert(
                0,
                StoredEntry::Session(SessionEntry {
                    v: 1,
                    id: session_id.to_string(),
                    created_at: now_ms(),
                }),
            );
        }
        let revision = current + 1;
        batch.push(StoredEntry::Meta(MetaEntry {
            active_leaf_id: active_leaf_id.map(str::to_string),
            title: None,
            revision,
        }));
        log.append(&batch)
            .map_err(|e| StorageError::Io(e.to_string()))?;
        Ok(revision)
    }

    /// `load_session_tree`（承载 §6.1 定稿结构）：全量条目重建 + 最新 meta 投影。
    pub fn load_session_tree(&self, session_id: &str) -> Result<SessionTreeData, StorageError> {
        validate_session_id(session_id)?;
        let mutex = self.locks.get(session_id);
        let _guard = mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let log = SessionLog::open(self.histories_dir().join(session_id))
            .map_err(|e| StorageError::Io(e.to_string()))?;
        if log.is_empty() {
            return Err(StorageError::NotFound(session_id.to_string()));
        }
        let mut active_leaf_id = None;
        for entry in log.entries() {
            if let StoredEntry::Meta(meta) = entry {
                active_leaf_id = meta.active_leaf_id.clone(); // 最新赢
            }
        }
        Ok(SessionTreeData {
            session_id: session_id.to_string(),
            messages: log
                .entries()
                .iter()
                .filter_map(|e| match e {
                    StoredEntry::Message(m) => Some(m.clone()),
                    _ => None,
                })
                .collect(),
            active_leaf_id,
            revision: log.revision().unwrap_or(0),
        })
    }

    fn histories_dir(&self) -> PathBuf {
        self.root.join("histories")
    }
}

/// 会话 id 合法性（防御性：直接用作目录名，仅允许 uuid 风格字符）。
fn validate_session_id(session_id: &str) -> Result<(), StorageError> {
    let ok_len = !session_id.is_empty() && session_id.len() <= 128;
    let ok_chars = session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok_len && ok_chars {
        Ok(())
    } else {
        Err(StorageError::InvalidArg(format!(
            "非法 sessionId：{session_id:?}"
        )))
    }
}

/// 条目规范化串（内容比对口径；键序确定，语义同内容哈希）。
fn canonical(message: &MessageEntry) -> serde_json::Result<String> {
    serde_json::to_string(message)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
