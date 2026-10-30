//! 会话树存取引擎（技术设计文档 §6.3/§6.4/§7）：`save_session_tree` /
//! `load_session_tree` 的 Rust 侧实现。
//!
//! 术语解释：「会话树」指一个会话的全部消息按父子关系（parentId）组成的树，
//! 支持「分支」——同一父节点下的多个回答版本；「修订号 revision」是每次成功
//! 保存都加一的整数，用于乐观并发控制（「乐观并发」指写前不长期持锁，而是在
//! 写入时比对版本号，不一致就拒绝并报冲突，让调用方重试）；「run」指模型的一轮
//! 连续执行（一次从开始到结束的生成过程），「runMarker」是收口一轮 run 的标记
//! 条目；「幂等」指同一操作重复执行结果不变，不会产生重复数据。
//!
//! 职责（§6.3/§6.4）：revision 乐观并发校验（不符报 E_SESSION_REV）、既有节点
//! 内容校验（同 id 内容变化 → E_SESSION_REV）、仅追加缺失节点（父先子后的顺序
//! 由前端 `thread.export()` 导出序保证，此处再校验父节点存在性）、追加 meta 行
//! 推进 revision。数据落盘：数据根 `~/.craftwave` 下的 `histories/<会话id>/`
//! 分段日志。Tauri 命令层（host）只做校验与转调（§2.2 依赖方向 host → storage）。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::SessionMeta;
use super::backup;
use super::entries::{
    MessageEntry, MessageRole, MetaEntry, SessionEntry, StoredCompactionEntry, StoredEntry,
    WirePart,
};
use super::index_store::SessionIndex;
use super::locks::LockMap;
use super::segment::SessionLog;

/// 会话存储器：数据根 + 会话锁表 + 会话索引（由 Tauri `State` 托管为全局单例）。
pub struct SessionStore {
    /// 数据根目录（生产为 `~/.craftwave`）。
    root: PathBuf,
    /// 按会话 id 区分的进程内锁，串行化同一会话的读写。
    locks: LockMap,
    /// 会话索引（`sessions/index.json`）读写器。
    index: SessionIndex,
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

/// 展示为 "变体: 说明"（命令层 `to_string` 面；Display 补齐于 04 号票，
/// 供压缩编排等跨层调用方统一错误文本）。
impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RevisionConflict { expected, actual } => {
                write!(f, "树同步基线过期：base={expected} 实际={actual}")
            }
            Self::NotFound(id) => write!(f, "会话不存在：{id}"),
            Self::InvalidArg(m) => write!(f, "参数校验失败：{m}"),
            Self::Io(m) => write!(f, "存储 IO 失败：{m}"),
        }
    }
}

/// `load_session_tree` 返回（§6.1 定稿结构：headId=activeLeafId + messages 含 `status?`；
/// 04 号票增 `compactions` 投影面——§6.3 定稿：compaction 条目不投影为树节点，
/// 前端按此数组渲染"已压缩早期上下文"分隔提示，wire 边界由后端史册承载）。
#[cfg_attr(test, derive(ts_rs::TS))]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTreeData {
    pub session_id: String,
    pub messages: Vec<MessageEntry>,
    /// 全部压缩边界条目（按追加序；最近一条在末位，wire 重建取它）。
    pub compactions: Vec<StoredCompactionEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub active_leaf_id: Option<String>,
    pub revision: u64,
}

/// `SessionStore` 的实现：全树同步、全量读取、canonical 追加与崩溃恢复、会话删除。
impl SessionStore {
    /// 以指定数据根构造（测试注入临时目录；生产为 `~/.craftwave`，§7.1）。
    pub fn with_root(root: PathBuf) -> Self {
        Self {
            index: SessionIndex::new(&root),
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
        // 有新消息节点落盘时同步索引（发送即在左栏可见的数据面）：index 的
        // messageCount 原本只在 run 终态推进，发送到收口之间会话在列表侧
        // 仍是"空会话"——前端按 messageCount 过滤空会话会误伤首条消息。
        if batch.iter().any(|e| matches!(e, StoredEntry::Message(_))) {
            let message_count = log
                .entries()
                .iter()
                .filter(|e| matches!(e, StoredEntry::Message(_)))
                .count() as u64;
            let now = now_ms();
            let synced = self
                .index
                .mutate(session_id, |m| {
                    m.message_count = message_count.max(m.message_count);
                    m.revision = revision;
                    m.updated_at = now;
                })
                .map_err(|e| StorageError::Io(format!("{e:?}")))?;
            if synced.is_none() {
                // 索引缺条目（防御路径，正常协议 create_session 先建）：兜底建档
                self.index
                    .upsert(SessionMeta {
                        id: session_id.to_string(),
                        project_id: None,
                        title: "新会话".to_owned(),
                        created_at: now,
                        updated_at: now,
                        revision,
                        message_count,
                        total_tokens: 0,
                        authorized_external_dirs: Vec::new(),
                    })
                    .map_err(|e| StorageError::Io(format!("{e:?}")))?;
            }
        }
        Ok(revision)
    }

    /// `load_session_tree`（承载 §6.1 定稿结构）：全量条目重建 + 最新 meta 投影。
    /// 读时修复（§7.3 末行）：发现无 runMarker 收口的 run → 追加 interrupted 修复行。
    pub fn load_session_tree(&self, session_id: &str) -> Result<SessionTreeData, StorageError> {
        validate_session_id(session_id)?;
        let mutex = self.locks.get(session_id);
        let _guard = mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut log = SessionLog::open(self.histories_dir().join(session_id))
            .map_err(|e| StorageError::Io(e.to_string()))?;
        if log.is_empty() {
            return Err(StorageError::NotFound(session_id.to_string()));
        }
        self.repair_interrupted_runs(&mut log)?;
        let mut active_leaf_id = None;
        let mut compactions = Vec::new();
        for entry in log.entries() {
            match entry {
                StoredEntry::Meta(meta) => {
                    active_leaf_id = meta.active_leaf_id.clone(); // 最新赢
                }
                StoredEntry::Compaction(c) => compactions.push(c.clone()),
                StoredEntry::Message(_) | StoredEntry::Session(_) => {}
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
            compactions,
            active_leaf_id,
            revision: log.revision().unwrap_or(0),
        })
    }

    /// canonical 追加原语（§7.3 落盘点的引擎面）：追加条目批 + meta(revision+1，
    /// activeLeaf 推进为批内最后一条 message 条目；title 沿用最新 meta 值）。
    /// 首次追加自动写 session 头。返回 (新 revision, 史册 message 条目总数)。
    pub fn append_canonical(
        &self,
        session_id: &str,
        batch: &[StoredEntry],
    ) -> Result<(u64, u64), StorageError> {
        validate_session_id(session_id)?;
        if batch.is_empty() {
            return Ok((
                self.probe_revision(session_id)?,
                self.probe_message_count(session_id)?,
            ));
        }
        let mutex = self.locks.get(session_id);
        let _guard = mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut log = SessionLog::open(self.histories_dir().join(session_id))
            .map_err(|e| StorageError::Io(e.to_string()))?;

        let mut full = Vec::with_capacity(batch.len() + 2);
        if log.is_empty() {
            full.push(StoredEntry::Session(SessionEntry {
                v: 1,
                id: session_id.to_string(),
                created_at: now_ms(),
            }));
        }
        full.extend(batch.iter().cloned());
        let last_message = batch.iter().rev().find_map(|e| match e {
            StoredEntry::Message(m) => Some(m.id.clone()),
            _ => None,
        });
        let revision = log.revision().unwrap_or(0) + 1;
        full.push(StoredEntry::Meta(MetaEntry {
            active_leaf_id: last_message.or_else(|| latest_active_leaf(&log)),
            title: latest_title(&log),
            revision,
        }));
        log.append(&full)
            .map_err(|e| StorageError::Io(e.to_string()))?;
        Ok((
            revision,
            log.entries()
                .iter()
                .filter(|e| matches!(e, StoredEntry::Message(_)))
                .count() as u64,
        ))
    }

    /// 索引读写器（list_sessions / canonical 终态 upsert 用）。
    pub fn index(&self) -> &SessionIndex {
        &self.index
    }

    /// compaction 条目追加（§6.1【M2】/§7.3 时机表，04 号票）：**存储格式升级
    /// 入口守卫**（Q34）——首次写入前成功创建一次可恢复备份，失败暂停写入
    /// 并报错（原数据不动；自动更新与手动安装共用本入口，天然同受保护）。
    /// 成功后经 append_canonical 落盘 + meta(revision++)，activeLeaf 不变
    /// （compaction 非消息条目，§6.3 不挂树）。返回推进后的 revision。
    pub fn append_compaction(
        &self,
        session_id: &str,
        entry: StoredCompactionEntry,
    ) -> Result<u64, StorageError> {
        backup::ensure_upgrade_backup(&self.root, "compaction").map_err(|e| {
            StorageError::Io(format!(
                "首次写入压缩条目前的升级备份失败，已暂停格式升级（原数据未动，可手动恢复后重试）：{e}"
            ))
        })?;
        let (revision, _) = self.append_canonical(session_id, &[StoredEntry::Compaction(entry)])?;
        Ok(revision)
    }

    /// 压缩状态概览（一次开日志读全）：（路径上最近的 compaction 条目，
    /// 已有压缩总数）。wire 重建取最近的；手动/自动压缩按总数推下一条序号
    /// （§6.1 id 规则 `{owner}:compact:{n}`，n 跨 owner 连续递增）。
    /// 会话不存在（空日志）→ (None, 0)。
    pub fn compaction_overview(
        &self,
        session_id: &str,
    ) -> Result<(Option<StoredCompactionEntry>, usize), StorageError> {
        validate_session_id(session_id)?;
        let mutex = self.locks.get(session_id);
        let _guard = mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let log = SessionLog::open(self.histories_dir().join(session_id))
            .map_err(|e| StorageError::Io(e.to_string()))?;
        let mut latest = None;
        let mut count = 0usize;
        for entry in log.entries() {
            if let StoredEntry::Compaction(c) = entry {
                latest = Some(c.clone());
                count += 1;
            }
        }
        Ok((latest, count))
    }

    /// 会话标题落盘（§6.1 meta 行 `title?` 字段 + §7.3「自动标题落盘」行，
    /// 27 号票）：会话锁内向段尾追加一条 meta 行（`title=Some(新标题)`、
    /// `activeLeafId` 沿用当前活动叶、revision+1）。追加式「最新赢」——旧
    /// meta 行不动，读取以最后一条 meta 为准（与 save/append_canonical 的
    /// revision 链同源，前端下次 save 用 load 回的新 revision 无冲突）。
    /// index 侧的 title/updatedAt 更新由调用方随后经 `index().mutate` 完成
    /// （锁序 session → index，§7.4：本方法持会话锁返回后调用方才碰 index）。
    /// 返回推进后的 revision。
    pub fn update_session_title(&self, session_id: &str, title: &str) -> Result<u64, StorageError> {
        validate_session_id(session_id)?;
        if title.trim().is_empty() {
            return Err(StorageError::InvalidArg("会话标题不能为空".into()));
        }
        let mutex = self.locks.get(session_id);
        let _guard = mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut log = SessionLog::open(self.histories_dir().join(session_id))
            .map_err(|e| StorageError::Io(e.to_string()))?;
        let revision = log.revision().unwrap_or(0) + 1;
        let meta = StoredEntry::Meta(MetaEntry {
            active_leaf_id: latest_active_leaf(&log),
            title: Some(title.to_owned()),
            revision,
        });
        log.append(&[meta])
            .map_err(|e| StorageError::Io(e.to_string()))?;
        Ok(revision)
    }

    /// 数据根（§7.1；边车等同级布局的基准）。
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// 崩溃恢复（§7.3）：史册里出现过的 run 若无对应 runMarker 条目（该 run
    /// 终态前进程崩溃），补一条 interrupted 修复行（endedAt=None）+ meta 推进。
    fn repair_interrupted_runs(&self, log: &mut SessionLog) -> Result<(), StorageError> {
        let mut runs_seen: HashMap<String, i64> = HashMap::new();
        let mut runs_marked: HashSet<String> = HashSet::new();
        for entry in log.entries() {
            if let StoredEntry::Message(m) = entry {
                let marker_run = m.parts.iter().find_map(|p| match p {
                    WirePart::RunMarker { run_id, .. } => Some(run_id.clone()),
                    _ => None,
                });
                match marker_run {
                    Some(run_id) => {
                        runs_marked.insert(run_id);
                    }
                    None => {
                        // 崩溃判定只认 canonical 形态条目（id 规则 `{runId}:{turn}…`，
                        // 15 号票）：save_session_tree 直接落的任意 id 条目（前端
                        // 分支节点带 runId 收养等）不视为未收口 run。
                        if let Some(run_id) = &m.run_id
                            && m.id.starts_with(&format!("{run_id}:"))
                            && !runs_seen.contains_key(run_id)
                        {
                            runs_seen.insert(run_id.clone(), m.created_at);
                        }
                    }
                }
            }
        }
        let broken: Vec<String> = runs_seen
            .keys()
            .filter(|run| !runs_marked.contains(*run))
            .cloned()
            .collect();
        if broken.is_empty() {
            return Ok(());
        }
        let now = now_ms();
        let mut batch: Vec<StoredEntry> = Vec::with_capacity(broken.len() + 1);
        for run_id in broken {
            let started_at = runs_seen[&run_id];
            batch.push(StoredEntry::Message(MessageEntry {
                id: format!("{run_id}:marker"),
                parent_id: None,
                role: MessageRole::System,
                parts: vec![WirePart::RunMarker {
                    run_id,
                    started_at,
                    ended_at: None,
                    duration_ms: None,
                    tool_count: None,
                }],
                created_at: now,
                run_id: None,
                status: None,
            }));
        }
        let revision = log.revision().unwrap_or(0) + 1;
        batch.push(StoredEntry::Meta(MetaEntry {
            active_leaf_id: latest_active_leaf(log),
            title: latest_title(log),
            revision,
        }));
        log.append(&batch)
            .map_err(|e| StorageError::Io(e.to_string()))?;
        Ok(())
    }

    /// 只读探测当前 revision（空批次 append_canonical 的返回面）。
    fn probe_revision(&self, session_id: &str) -> Result<u64, StorageError> {
        let log = SessionLog::open(self.histories_dir().join(session_id))
            .map_err(|e| StorageError::Io(e.to_string()))?;
        Ok(log.revision().unwrap_or(0))
    }

    /// 只读探测当前消息条目总数（空批次 append_canonical 的返回面）。
    fn probe_message_count(&self, session_id: &str) -> Result<u64, StorageError> {
        let log = SessionLog::open(self.histories_dir().join(session_id))
            .map_err(|e| StorageError::Io(e.to_string()))?;
        Ok(log
            .entries()
            .iter()
            .filter(|e| matches!(e, StoredEntry::Message(_)))
            .count() as u64)
    }

    /// 会话删除（§3.3 #16 存储面）：index 条目 + histories/ + prefs +
    /// toolres 边车 + 图片 blob 目录（`<sid>.imgblob/`，17 号票 §7.7 v1
    /// GC 口径：随会话整体删除）一并清除。运行中拒绝在命令层判定
    /// （E_SESSION_RUNNING）。返回是否存在（幂等面：不存在 → Ok(false)，
    /// 命令层转 E_NOT_FOUND）。
    pub fn delete_session(&self, session_id: &str) -> Result<bool, StorageError> {
        validate_session_id(session_id)?;
        let mutex = self.locks.get(session_id);
        let _guard = mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let removed = self
            .index
            .remove(session_id)
            .map_err(|e| StorageError::Io(format!("{e:?}")))?;
        if removed.is_none() {
            return Ok(false);
        }
        // 索引已摘除，磁盘清理尽力而为（半删残留不阻塞用户操作）
        let _ = std::fs::remove_dir_all(self.histories_dir().join(session_id));
        let sessions_dir = self.root.join("sessions");
        let _ = std::fs::remove_file(sessions_dir.join(format!("{session_id}.prefs.json")));
        let _ = std::fs::remove_dir_all(sessions_dir.join(format!("{session_id}.toolres")));
        // 图片 blob 目录（17 号票：§7.7 v1 GC——会话删除即整体删除；
        // 跨会话不共享 blob，无引用计数问题）
        let _ = std::fs::remove_dir_all(super::imgblob::imgblob_dir(&self.root, session_id));
        Ok(true)
    }

    /// 会话日志根目录：数据根下的 `histories/`，每个会话占其中一个子目录。
    fn histories_dir(&self) -> PathBuf {
        self.root.join("histories")
    }
}

/// 史册最新 meta 的 activeLeaf（无则 None）。
fn latest_active_leaf(log: &SessionLog) -> Option<String> {
    log.entries().iter().rev().find_map(|e| match e {
        StoredEntry::Meta(meta) => meta.active_leaf_id.clone(),
        _ => None,
    })
}

/// 史册最新 meta 的标题（无则 None）。「最新赢」：只看最后一条 meta 行。
fn latest_title(log: &SessionLog) -> Option<String> {
    log.entries().iter().rev().find_map(|e| match e {
        StoredEntry::Meta(meta) => meta.title.clone(),
        _ => None,
    })
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

/// 当前 Unix 时间戳（毫秒）。取不到系统时间（早于 1970 年）时返回 0，
/// 保证时间戳字段总有值、不影响落盘流程。
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
