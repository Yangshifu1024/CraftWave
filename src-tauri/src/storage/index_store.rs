//! 会话索引（§7.1 `sessions/index.json`，§3.2 SessionMeta 契约镜像）。
//!
//! 读侧：`list_sessions` 命令（§3.3 #14）按 updatedAt 倒序；
//! 写侧：canonical 落盘点（`append_canonical`）upsert——revision/updatedAt/
//! messageCount/totalTokens 随每次追加推进；`create/rename/delete_session`
//! 命令（16 号票）走 get/mutate/remove。整文件原子写（§7.4）；
//! 条目上限 2000，溢出最旧移入 `sessions/archive.jsonl`（§7.1）。

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::util::atomic::write_atomic;

/// index.json 条目上限（§7.1：≤2000 条，超限归档最旧）。
const MAX_INDEX_ENTRIES: usize = 2000;

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

    /// upsert 一条会话元数据（同 id 覆盖字段，保持列表原序）；
    /// 超过上限时把最旧条目移入 archive.jsonl（归档失败降级为保留超限，
    /// 索引本身不因归档 IO 抖动而写失败）。
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
        if file.sessions.len() > MAX_INDEX_ENTRIES
            && let Err(e) = self.archive_oldest(&mut file)
        {
            #[cfg(debug_assertions)]
            eprintln!("[craftwave] index 归档失败（保留超限条目）：{e:?}");
        }
        self.write(&file)
    }

    /// 按 id 取单条（start_chat / workspace 命令的会话 roots 解析入口）。
    pub fn get(&self, session_id: &str) -> Option<SessionMeta> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.read()
            .ok()?
            .sessions
            .into_iter()
            .find(|m| m.id == session_id)
    }

    /// 部分更新（rename / 项目转临时等字段级修改）；返回更新后的条目，
    /// 会话不存在返回 None。
    pub fn mutate(
        &self,
        session_id: &str,
        f: impl FnOnce(&mut SessionMeta),
    ) -> Result<Option<SessionMeta>, IndexError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut file = self.read()?;
        let Some(slot) = file.sessions.iter_mut().find(|m| m.id == session_id) else {
            return Ok(None);
        };
        f(slot);
        let updated = slot.clone();
        self.write(&file)?;
        Ok(Some(updated))
    }

    /// 移除一条（会话删除）；返回被移除条目，不存在返回 None。
    pub fn remove(&self, session_id: &str) -> Result<Option<SessionMeta>, IndexError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut file = self.read()?;
        let Some(pos) = file.sessions.iter().position(|m| m.id == session_id) else {
            return Ok(None);
        };
        let removed = file.sessions.remove(pos);
        self.write(&file)?;
        Ok(Some(removed))
    }

    /// 把 updatedAt 最旧的溢出条目追加到 archive.jsonl 并从索引移除
    /// （历史 JSONL 一律不动，归档只是索引瘦身）。
    fn archive_oldest(&self, file: &mut IndexFile) -> Result<(), IndexError> {
        let mut order: Vec<usize> = (0..file.sessions.len()).collect();
        order.sort_by(|&a, &b| {
            file.sessions[a]
                .updated_at
                .cmp(&file.sessions[b].updated_at)
                .then(file.sessions[a].id.cmp(&file.sessions[b].id))
        });
        let overflow = order.len() - MAX_INDEX_ENTRIES;
        let victims: Vec<SessionMeta> = order[..overflow]
            .iter()
            .map(|&i| file.sessions[i].clone())
            .collect();
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(IndexError::Io)?;
        }
        let archive = self.path.with_file_name("archive.jsonl");
        let mut out = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&archive)
            .map_err(IndexError::Io)?;
        let victim_ids: HashSet<String> = victims.iter().map(|m| m.id.clone()).collect();
        for meta in &victims {
            let line =
                serde_json::to_string(meta).map_err(|e| IndexError::Corrupt(e.to_string()))?;
            out.write_all(line.as_bytes()).map_err(IndexError::Io)?;
            out.write_all(b"\n").map_err(IndexError::Io)?;
        }
        file.sessions.retain(|m| !victim_ids.contains(&m.id));
        Ok(())
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
