//! 统一信任存储（§7.7 `trust/project.json` / §9.8；24 号票收敛落点）。
//!
//! 名词解释：TOFU（Trust On First Use，首次使用信任）——外部资源第一次被
//! 使用时先向用户弹确认，「记住此版本」后同指纹免问，内容一变（指纹不符）
//! 重新询问。此前 MCP 启动信任（08 号票，`trust/mcp.json`）与项目资源信任
//! （09 号票技能最小版，`trust/project.json`）各自持有一份存储实现；本票把
//! 两者收敛为**单一文件单一实现**：
//!
//! - 文件 = `<data>/trust/project.json`，条目数组
//!   `[{ scope, resourceName, fingerprint, decidedAt, decision }]`；
//! - `scope` 判别字段区分两类记录：`"project"`（项目/全局技能、项目指令等
//!   项目资源）与 `"mcp"`（MCP 服务器启动信任，§9.7）——设计文档 §7.7
//!   已随本票回写；
//! - MCP 侧历史遗留文件 `trust/mcp.json`（08 号票形状，无 scope/resourceName）
//!   在读取时**合并迁入**内存视图（project.json 优先、按
//!   (scope, resourceName, fingerprint) 去重），首次写入时随统一格式落回
//!   project.json——旧文件保留不删（只读迁移面，幂等安全）；
//! - 指纹算法不因收敛改变：MCP 沿用 §9.7 元组哈希（mcp/trust 的
//!   `launch_fingerprint`），技能沿用 Q21 整目录摘要（knowledge/skills 的
//!   `version_fingerprint`），项目指令 = 文件内容 SHA-256（§9.8）。
//!
//! 信任面磁盘纪律：**缺失/损坏按未信任处理**（宁缺勿炸——文件损坏时用户
//! 重新确认即可恢复，不做崩溃性失败）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 信任域目录与统一文件名（§7.7 trust 域）。
pub const TRUST_DIR: &str = "trust";
/// 统一信任文件（project 资源 + MCP 启动信任共用，scope 判别）。
pub const TRUST_FILE: &str = "project.json";
/// 08 号票遗留的 MCP 启动信任文件（只读迁入面）。
pub const LEGACY_MCP_FILE: &str = "mcp.json";

/// 决策值域（`decision` 字段的两个合法值）。
pub const DECISION_REMEMBER: &str = "remember";
pub const DECISION_REVOKE: &str = "revoke";

/// 信任判别作用域（§7.7 scope 字段，24 号票回写）：记录属于哪一类信任面。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum TrustScope {
    /// 项目资源信任（技能 / 项目指令；全局来源技能同属本域——
    /// §9.8 载荷的 global/project 作用域不进存储键，同名同内容共享记录）。
    Project,
    /// MCP 服务器启动信任（§9.7 TOFU）。
    Mcp,
}

/// 决策值（remember = 记住此版本同指纹免问；revoke = 显式撤销，下次使用
/// 重新询问）。deny / allow_once 不落盘（会话内存面与「不产生记录」口径，
/// §9.8/§9.7 同构）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum TrustDecision {
    /// 记住此版本。
    Remember,
    /// 显式撤销（管理页/技能页撤销动作；询问流程遇 revoke 记录按未信任
    /// 处理，重新询问）。
    Revoke,
}

// TrustDecision 的方法。
impl TrustDecision {
    /// 决策字符串值（`decision` 字段）。
    pub fn as_str(self) -> &'static str {
        match self {
            TrustDecision::Remember => DECISION_REMEMBER,
            TrustDecision::Revoke => DECISION_REVOKE,
        }
    }

    /// 从字符串解析（旧文件兼容面；未知值返回 None）。
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            DECISION_REMEMBER => Some(TrustDecision::Remember),
            DECISION_REVOKE => Some(TrustDecision::Revoke),
            _ => None,
        }
    }
}

/// 统一信任记录（§7.7 `trust/project.json` 条目，24 号票字段级）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustRecord {
    /// 判别作用域（project 资源 / mcp 服务器）。
    pub scope: TrustScope,
    /// 资源名（project 域 = 技能名或指令文件名；mcp 域恒空串——服务器身份
    /// 已完整包含在指纹元组里，08 号票遗留条目迁移时同样补空串）。
    #[serde(default)]
    pub resource_name: String,
    /// 内容指纹（完整 hex；各域算法见模块头注释）。
    pub fingerprint: String,
    /// 决策时间（epoch ms）。
    pub decided_at: i64,
    /// 决策（"remember" | "revoke"）。
    pub decision: String,
}

/// 指纹的信任状态（管理/撤销 UI 的展示面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustRecordStatus {
    /// 已记住（同指纹免问）。
    Trusted,
    /// 待决策（从未见过该指纹）。
    Pending,
    /// 已撤销（用户显式撤销；下次使用重新询问）。
    Revoked,
}

// TrustRecordStatus 的方法。
impl TrustRecordStatus {
    /// 快照/管理页的字符串面（"trusted"|"pending"|"revoked"）。
    pub fn as_str(self) -> &'static str {
        match self {
            TrustRecordStatus::Trusted => "trusted",
            TrustRecordStatus::Pending => "pending",
            TrustRecordStatus::Revoked => "revoked",
        }
    }
}

/// 信任存储的统一接口（24 号票收敛面：文件实现为生产形态，测试可替换桩）。
pub trait TrustStore: Send + Sync {
    /// 全部记录快照（含旧 mcp.json 迁入的条目）。
    fn entries(&self) -> Vec<TrustRecord>;

    /// 是否已被「记住」（scope + resourceName + fingerprint 全等命中
    /// remember）。resourceName 参与匹配：同名资源换内容后指纹变，自然
    /// 不命中（内容变更重新询问，§9.8）。
    fn is_trusted(&self, scope: TrustScope, resource_name: &str, fingerprint: &str) -> bool;

    /// 按 (scope, fingerprint) 查信任状态（mcp 管理页信任列 / 技能页撤销
    /// 列的判定面；同指纹多记录时取首条）。
    fn status_of(&self, scope: TrustScope, fingerprint: &str) -> TrustRecordStatus;

    /// 写入/覆盖一条决策（同 (scope, resourceName, fingerprint) 旧记录覆盖
    /// decidedAt 与 decision；原子写）。
    fn decide(&self, record: TrustRecord) -> Result<(), String>;

    /// 清空全部记录（Q41 备份恢复接线面：恢复不复活已撤销/已记住许可，
    /// 恢复流程由 33 号票调用；普通重启不调用）。
    fn clear_all(&self) -> Result<(), String>;
}

/// 统一信任存储的文件实现（`<data>/trust/project.json`）。
#[derive(Debug, Clone)]
pub struct FileTrustStore {
    /// 统一文件路径。
    path: PathBuf,
    /// 08 号票遗留文件路径（只读迁入面；不存在时零开销）。
    legacy_path: PathBuf,
}

// FileTrustStore 的构造与读写实现。
impl FileTrustStore {
    /// 构造：数据目录根（文件 = `<data>/trust/project.json`）。
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(TRUST_DIR).join(TRUST_FILE),
            legacy_path: data_dir.join(TRUST_DIR).join(LEGACY_MCP_FILE),
        }
    }

    /// 读取统一文件（缺失/损坏 = 空；信任面宁缺勿炸）。
    fn read_own(&self) -> Vec<TrustRecord> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// 读取 08 号票遗留 mcp.json（形状 `{fingerprint, decidedAt, decision}`，
    /// 无 scope/resourceName）并补齐为统一记录；文件缺失/损坏 = 空。
    fn read_legacy_mcp(&self) -> Vec<TrustRecord> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct LegacyEntry {
            fingerprint: String,
            decided_at: i64,
            decision: String,
        }
        std::fs::read(&self.legacy_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<LegacyEntry>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|e| TrustRecord {
                scope: TrustScope::Mcp,
                resource_name: String::new(),
                fingerprint: e.fingerprint,
                decided_at: e.decided_at,
                decision: e.decision,
            })
            .collect()
    }
}

// TrustStore trait 的文件实现。
impl TrustStore for FileTrustStore {
    fn entries(&self) -> Vec<TrustRecord> {
        // 统一文件优先，遗留文件按 (scope, resourceName, fingerprint) 去重
        // 补尾——project.json 已有同键记录时旧值不再出现（迁移幂等）。
        let own = self.read_own();
        let legacy = self.read_legacy_mcp();
        let mut merged = own;
        for record in legacy {
            let duplicate = merged.iter().any(|e| {
                e.scope == record.scope
                    && e.resource_name == record.resource_name
                    && e.fingerprint == record.fingerprint
            });
            if !duplicate {
                merged.push(record);
            }
        }
        merged
    }

    fn is_trusted(&self, scope: TrustScope, resource_name: &str, fingerprint: &str) -> bool {
        self.entries().iter().any(|e| {
            e.scope == scope
                && e.resource_name == resource_name
                && e.fingerprint == fingerprint
                && e.decision == DECISION_REMEMBER
        })
    }

    fn status_of(&self, scope: TrustScope, fingerprint: &str) -> TrustRecordStatus {
        match self
            .entries()
            .into_iter()
            .find(|e| e.scope == scope && e.fingerprint == fingerprint)
        {
            Some(e) => match TrustDecision::parse(&e.decision) {
                Some(TrustDecision::Remember) => TrustRecordStatus::Trusted,
                _ => TrustRecordStatus::Revoked,
            },
            None => TrustRecordStatus::Pending,
        }
    }

    fn decide(&self, record: TrustRecord) -> Result<(), String> {
        let mut entries = self.entries();
        match entries.iter_mut().find(|e| {
            e.scope == record.scope
                && e.resource_name == record.resource_name
                && e.fingerprint == record.fingerprint
        }) {
            Some(slot) => {
                slot.decision = record.decision;
                slot.decided_at = record.decided_at;
            }
            None => entries.push(record),
        }
        write_records_atomic(&self.path, &entries)
    }

    fn clear_all(&self) -> Result<(), String> {
        write_records_atomic(&self.path, &[])
    }
}

/// 原子写统一信任文件（util/atomic，§7.4 同款；目录按需创建）。
fn write_records_atomic(path: &Path, records: &[TrustRecord]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建信任目录失败：{e}"))?;
    }
    let json = serde_json::to_value(records).map_err(|e| format!("序列化失败：{e}"))?;
    crate::util::atomic::write_json(path, &json).map_err(|e| format!("写信任文件失败：{e}"))
}

/// 当前毫秒时间戳（epoch ms；信任域共用面）。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
