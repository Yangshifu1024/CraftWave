//! 会话索引（§7.1 `sessions/index.json`，§3.2 SessionMeta 契约镜像）。
//!
//! 读侧：`list_sessions` 命令（§3.3 #14）按 updatedAt 倒序；
//! 写侧：canonical 落盘点（`append_canonical`）upsert——revision/updatedAt/
//! messageCount/totalTokens 随每次追加推进。整文件原子写（§7.4）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::util::atomic::write_atomic;

/// §3.2 SessionMeta（index.json 的 sessions 元素与 list_sessions 返回）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub revision: u64,
    pub message_count: u64,
    pub total_tokens: u64,
}

/// index.json 载荷（`{version:1, sessions:[SessionMeta]}`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexFile {
    version: u32,
    #[serde(default)]
    sessions: Vec<SessionMeta>,
}

/// 索引读写器（数据根托管；进程内 Mutex 串行化，§7.4 单进程口径）。
pub struct SessionIndex {
    path: PathBuf,
    lock: Mutex<()>,
}

/// 索引不可读（损坏/IO）时的降级语义：list 返回空表重建，
/// 与《数据存储调研》「可删可重建」铁律一致。
#[derive(Debug)]
pub enum IndexError {
    Corrupt(String),
    Io(std::io::Error),
}

impl SessionIndex {
    /// 数据根 → `sessions/index.json`。
    pub fn new(data_root: &Path) -> Self {
        Self {
            path: data_root.join("sessions").join("index.json"),
            lock: Mutex::new(()),
        }
    }

    /// upsert 一条会话元数据（同 id 覆盖字段，保持列表原序）。
    pub fn upsert(&self, meta: SessionMeta) -> Result<(), IndexError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut file = self.read()?;
        match file.sessions.iter_mut().find(|m| m.id == meta.id) {
            Some(slot) => *slot = meta,
            None => file.sessions.push(meta),
        }
        self.write(&file)
    }

    /// 全量列表，updatedAt 倒序（§3.3 #14）。
    pub fn list(&self) -> Vec<SessionMeta> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut sessions = self.read().map(|f| f.sessions).unwrap_or_default();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
        sessions
    }

    fn read(&self) -> Result<IndexFile, IndexError> {
        let raw = match fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(IndexFile {
                    version: 1,
                    sessions: Vec::new(),
                });
            }
            Err(e) => return Err(IndexError::Io(e)),
        };
        serde_json::from_str(&raw).map_err(|e| IndexError::Corrupt(e.to_string()))
    }

    fn write(&self, file: &IndexFile) -> Result<(), IndexError> {
        let bytes = serde_json::to_vec(file).map_err(|e| IndexError::Corrupt(e.to_string()))?;
        write_atomic(&self.path, &bytes).map_err(IndexError::Io)
    }
}
