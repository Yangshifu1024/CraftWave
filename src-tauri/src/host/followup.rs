//! 会话级 followUp 队列。
//!
//! 名词解释：followUp（后续消息）= 用户在 run 运行中或空闲时显式排队的消息
//! （Alt+Enter）——不注入当前 run，等 run 正常完成后由后端自动投递队列头
//! 并新开 run；run 失败（error）则队列进入**暂停态**，持久保留全部待发消息，
//! 等待用户明确继续（`resume_followup_queue`）；用户取消 run 则队列清空，
//! 文本经 `run:queue` 事件的 `returned` 数组交还前端草稿区。
//!
//! 持久化：每会话一个 `sessions/<sid>.queue.json`
//! （与既有 `<sid>.prefs.json` 同构布局），任何内存变更后立即原子写
//! （`util::atomic`，fsync + rename）；加载为惰性——会话首次被队列
//! 操作触及时从文件恢复进内存，重启后队列与暂停态原样保留。
//!
//! 接线说明：恢复旧升级备份后的强制暂停由恢复流程改写本文件达成
//! （queue.json 即权威），本模块只保证暂停态随文件持久化。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::safety::tier::PermissionTier;
use crate::util::atomic::write_json;

/// queue.json 格式版本（结构不兼容时整体拒绝加载并按空队列重建——待发消息
/// 不可用比半可用状态更诚实，错误记日志）。
pub const QUEUE_FILE_VERSION: u32 = 1;

/// 附着 runConfig 快照（`runConfig?`）：入队时刻的运行配置摘要，
/// 投递时按它构造新 run 的 RunConfig——排队时的权限档/模型/轮次在投递时
/// 原样生效（不随设置漂移）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct QueuedRunConfig {
    /// 会话级模型覆盖（缺省 = 全局 activeModelId，同口径）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub model_id: Option<String>,
    /// 会话 thinking 档快照：投递新 run 时按它设置
    /// 起点档位（serde default 向后兼容——旧队列文件无此字段照常恢复）；
    /// 运行中以后端 prefs 为权威（每拍刷新）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub thinking: Option<crate::providers::reasoning::ThinkingLevel>,
    pub tier: PermissionTier,
    pub max_turns: u32,
}

/// 带图待发条目的附件引用（「带图消息不入 steer，进 followUp」）：
/// 图片字节已由前端经 `attach_image` 落入会话 imgblob，入队只携带
/// 引用——`blobHash`（内容寻址）与 `mimeType`（png/jpeg/gif/webp 白名单）；
/// 投递时由 [`deliver 侧`](crate::host::commands::chat) 展开为 WirePart::Image。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct QueuedImageAttachment {
    /// 图片 blob 哈希（SHA-256 前 32 hex，内容寻址）。
    pub blob_hash: String,
    /// MIME 类型（png/jpeg/gif/webp）。
    pub mime_type: String,
}

/// 一条待发消息（Entry 字段级）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct FollowUpEntry {
    /// 条目唯一标识（`fu-{uuid}`；删除命令与前端队列条按键）。
    pub id: String,
    /// 消息正文（纯文本；带图经 [`QueuedImageAttachment`] 承接）。
    pub text: String,
    /// 入队时刻（Unix 纪元毫秒）。
    pub created_at: i64,
    /// 附着运行配置快照（投递新 run 的 RunConfig 来源）。
    pub run_config: QueuedRunConfig,
    /// 图片附件引用（带图不入 steer、经此通路进
    /// followUp）。空数组省略序列化。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub attachments: Vec<QueuedImageAttachment>,
}

/// queue.json 落盘信封（字段级格式）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FollowUpQueueFile {
    pub version: u32,
    /// 失败暂停态（跨重启保留；resume_followup_queue 解除）。
    pub paused: bool,
    /// 待发消息（队头在前）。
    pub entries: Vec<FollowUpEntry>,
}

/// 单会话的内存队列（互斥锁保护面在 [`FollowUpManager`] 一层）。
#[derive(Debug, Default, Clone)]
struct SessionQueue {
    paused: bool,
    entries: VecDeque<FollowUpEntry>,
}

/// 会话级 followUp 队列管理器（Clone = 同一份注册表的共享句柄，挂
/// AppState 与投递任务两侧）。锁被污染时直接恢复使用——队列是纯内存结构
/// 且每次变更后立即落盘，恢复使用比让后续操作全部失败更合理。
#[derive(Clone)]
pub struct FollowUpManager {
    /// 数据根（`~/.craftwave`）：队列文件在 `<root>/sessions/` 下。
    root: PathBuf,
    /// 会话 id → 内存队列；首触从 queue.json 惰性恢复（见 [`Self::load`]）。
    inner: Arc<Mutex<HashMap<String, SessionQueue>>>,
}

/// 队列文件路径（`sessions/<sid>.queue.json`）。
fn queue_file_path(root: &Path, session_id: &str) -> PathBuf {
    root.join("sessions")
        .join(format!("{session_id}.queue.json"))
}

/// 队列操作的公开结果（快照面，`run:queue` 事件载荷与命令返回共用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSnapshot {
    /// 待发消息条数。
    pub size: usize,
    /// 是否处于失败暂停态。
    pub paused: bool,
}

impl FollowUpManager {
    /// 以数据根构造空管理器（内存队列惰性恢复，构造时不做任何 IO）。
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 锁获取（污染即恢复：见类型级注释）。
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, SessionQueue>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 取出某会话的内存队列；首次触碰时从 queue.json 恢复（文件缺失 = 空队列，
    /// 属正常；文件损坏按空队列重建并记日志——待发消息不可用比半可用诚实）。
    fn load(&self, session_id: &str) -> SessionQueue {
        if let Some(q) = self.lock().get(session_id) {
            // 已在内存：克隆一份返回（变更走统一入口，避免持锁跨 IO）
            return q.clone();
        }
        let path = queue_file_path(&self.root, session_id);
        let restored = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<FollowUpQueueFile>(&bytes).ok())
            .map(|file| {
                if file.version != QUEUE_FILE_VERSION {
                    // 未知版本按空队列处理（前瞻性防御：降级读半兼容格式更危险）
                    SessionQueue::default()
                } else {
                    SessionQueue {
                        paused: file.paused,
                        entries: file.entries.into(),
                    }
                }
            })
            .unwrap_or_default();
        self.lock().insert(session_id.to_owned(), restored.clone());
        restored
    }

    /// 变更后的统一收口：写回内存 + 原子落盘（失败只记日志——队列以内存为
    /// 投递权威，落盘失败影响的是重启后恢复，不阻塞当前操作）。
    fn persist(&self, session_id: &str, queue: &SessionQueue) {
        self.lock().insert(session_id.to_owned(), queue.clone());
        let file = FollowUpQueueFile {
            version: QUEUE_FILE_VERSION,
            paused: queue.paused,
            entries: queue.entries.iter().cloned().collect(),
        };
        if let Ok(value) = serde_json::to_value(&file)
            && let Err(e) = write_json(&queue_file_path(&self.root, session_id), &value)
        {
            #[cfg(debug_assertions)]
            eprintln!("[craftwave] followUp 队列落盘失败（{session_id}）：{e:?}");
            #[cfg(not(debug_assertions))]
            let _ = e;
        }
    }

    /// 入队（追加到队尾；排队次序 = 入队次序）并返回变更后快照。
    pub fn push(&self, session_id: &str, entry: FollowUpEntry) -> QueueSnapshot {
        let mut queue = self.load(session_id);
        queue.entries.push_back(entry);
        let snapshot = QueueSnapshot {
            size: queue.entries.len(),
            paused: queue.paused,
        };
        self.persist(session_id, &queue);
        snapshot
    }

    /// 队头出队（自动投递取消息）；空队列返回 None。
    pub fn pop_head(&self, session_id: &str) -> Option<FollowUpEntry> {
        let mut queue = self.load(session_id);
        let entry = queue.entries.pop_front()?;
        self.persist(session_id, &queue);
        Some(entry)
    }

    /// 队头回队（投递撞 E_SESSION_RUNNING 时的回队重试，不丢消息）。
    pub fn push_front(&self, session_id: &str, entry: FollowUpEntry) -> QueueSnapshot {
        let mut queue = self.load(session_id);
        queue.entries.push_front(entry);
        let snapshot = QueueSnapshot {
            size: queue.entries.len(),
            paused: queue.paused,
        };
        self.persist(session_id, &queue);
        snapshot
    }

    /// 按 id 移除一条（`remove_followup` 命令与前端「编辑取出」共用）；
    /// 未命中返回 None（命令层转 E_NOT_FOUND）。
    pub fn remove(&self, session_id: &str, entry_id: &str) -> Option<FollowUpEntry> {
        let mut queue = self.load(session_id);
        let pos = queue.entries.iter().position(|e| e.id == entry_id)?;
        let entry = queue.entries.remove(pos)?;
        self.persist(session_id, &queue);
        Some(entry)
    }

    /// 清空全部待发并解除暂停（cancel_run 收尾返还面），返回交还文本
    /// （按排队序）。队列文件同步删除（e2e 面持久化文件删除语义）：
    /// 空队列无信息量，重启后惰性加载默认空队列语义一致；
    /// 删除失败只记日志（内存态已是空，下次变更会重写文件纠正）。
    pub fn clear(&self, session_id: &str) -> Vec<String> {
        let mut queue = self.load(session_id);
        let texts: Vec<String> = queue.entries.drain(..).map(|e| e.text).collect();
        queue.paused = false;
        self.lock().insert(session_id.to_owned(), queue.clone());
        let path = queue_file_path(&self.root, session_id);
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            #[cfg(debug_assertions)]
            eprintln!("[craftwave] followUp 队列文件删除失败（{session_id}）：{e:?}");
            #[cfg(not(debug_assertions))]
            let _ = e;
        }
        texts
    }

    /// 设置暂停态（run 失败收尾置 true；resume 命令置 false），返回变更后快照。
    pub fn set_paused(&self, session_id: &str, paused: bool) -> QueueSnapshot {
        let mut queue = self.load(session_id);
        queue.paused = paused;
        let snapshot = QueueSnapshot {
            size: queue.entries.len(),
            paused: queue.paused,
        };
        self.persist(session_id, &queue);
        snapshot
    }

    /// 只读快照（事件载荷与投递判定共用；不产生 IO 之外的副作用）。
    pub fn snapshot(&self, session_id: &str) -> QueueSnapshot {
        let queue = self.load(session_id);
        QueueSnapshot {
            size: queue.entries.len(),
            paused: queue.paused,
        }
    }

    /// 队头预览（投递前判定与测试断言用；不出队）。
    pub fn peek_head(&self, session_id: &str) -> Option<FollowUpEntry> {
        self.load(session_id).entries.front().cloned()
    }

    /// 全部会话的队列快照（`update_precheck` 安装门判定面）：以
    /// **磁盘为权威**扫描 `sessions/*.queue.json` 全量（不依赖内存表——
    /// 队列加载是惰性的，内存表只含被触碰过的会话；漏扫会把从未打开过
    /// 界面的待发队列误判为「没有队列」放行安装）。只读，不改内存缓存、
    /// 不落盘。文件损坏/版本未知按「存在且未暂停的有待发队列」保守计入
    /// running（更新门从严：解析不出的队列不能当不存在放行重启）。
    pub fn all_snapshots(&self) -> Vec<(String, QueueSnapshot)> {
        let sessions_dir = self.root.join("sessions");
        let Ok(entries) = std::fs::read_dir(&sessions_dir) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            let Some(name) = name else { continue };
            let Some(session_id) = name.strip_suffix(".queue.json") else {
                continue;
            };
            let snapshot = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<FollowUpQueueFile>(&bytes).ok())
                .map(|file| QueueSnapshot {
                    size: file.entries.len(),
                    paused: file.paused,
                })
                // 损坏/版本未知：保守计为「有队列且未暂停」（更新门从严）
                .unwrap_or(QueueSnapshot {
                    size: 1,
                    paused: false,
                });
            out.push((session_id.to_owned(), snapshot));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

/// 队列管理器的调试实现（内含互斥锁，只打印可比较的记账面）。
impl std::fmt::Debug for FollowUpManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sessions = self
            .lock()
            .iter()
            .map(|(sid, q)| (sid.clone(), q.entries.len(), q.paused))
            .collect::<Vec<_>>();
        f.debug_struct("FollowUpManager")
            .field("root", &self.root)
            .field("sessions", &sessions)
            .finish()
    }
}

// 单元测试挂载：测试代码放在独立文件 followup_test.rs，仅 cargo test 时编译。
#[cfg(test)]
#[path = "followup_test.rs"]
mod followup_test;
