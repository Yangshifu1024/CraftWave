//! 会话索引（技术设计文档 §7.1 `sessions/index.json`，§3.2 SessionMeta 契约镜像）。
//!
//! 术语解释：「索引」是所有会话的摘要清单文件（每个会话一行元数据：标题、
//! 修订号、消息数、token 数等），让会话列表页不必打开每个会话的完整日志就能
//! 展示列表；「upsert」指插入或更新的合称——同一 id 已存在则覆盖，不存在则新增；
//! 「归档」指索引条目超过上限时把最旧的条目搬到 `sessions/archive.jsonl`
//! 存放，只是给索引瘦身，会话历史日志文件本身永远不动。
//!
//! 读侧：`list_sessions` 命令（§3.3 #14）按 updatedAt 倒序；
//! 写侧：canonical 落盘点（`append_canonical`，指所有追加都必须走的那条唯一
//! 标准写入路径）upsert——revision/updatedAt/messageCount/totalTokens 随每次
//! 追加推进；`create/rename/delete_session` 命令（16 号票）走 get/mutate/remove。
//! 整文件原子写（§7.4：先写临时文件再改名，读者永远看不到半个文件）；
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

/// 单条会话元数据（§3.2 SessionMeta）：既是 index.json 里 sessions 数组的元素，
/// 也是 `list_sessions` 命令返回给前端的条目形状。revision 是只增不减的修订号，
/// 用来识别同一会话的哪个版本最新。
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

/// 索引读写器（数据根托管；进程内 Mutex 串行化，§7.4 单进程口径）：
/// 同一实例上的读改写操作通过内部互斥量排队执行，避免并发交错写坏文件。
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

/// 索引读写器的实现：upsert/get/mutate/remove/list 全部走「读整文件 → 修改 →
/// 原子写回」流程，每个方法入口先拿互斥量保证串行。
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

    /// 读整份 index.json；文件不存在视为空索引（首次使用），解析失败报 Corrupt。
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

    /// 序列化整份索引并原子写回 index.json（临时文件 + 改名，§7.4）。
    fn write(&self, file: &IndexFile) -> Result<(), IndexError> {
        let bytes = serde_json::to_vec(file).map_err(|e| IndexError::Corrupt(e.to_string()))?;
        write_atomic(&self.path, &bytes).map_err(IndexError::Io)
    }
}
