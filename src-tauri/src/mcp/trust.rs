//! MCP TOFU 启动信任与 Q44 工具级动态许可（§9.7 字段级；08 号票）。
//!
//! TOFU（Trust On First Use，首次使用信任）：首次连接一个 MCP 服务器前，
//! 计算启动指纹并向用户弹确认（本次允许 / 记住此版本 / 拒绝）；"记住"持久
//! 化后同指纹免问，指纹变更（配置被改动）重新询问。
//!
//! **存储定案（08 号票出入回写点）**：§9.7/§3.3#46 原文写"决策写 keyring"，
//! 而 v2.5 已废除钥匙串（§9.4 用户决策）——按对齐 §7.7 trust 域的用户决策
//! 落 `~/.craftwave/trust/mcp.json`（设计文档两处 keyring 字样已随本票
//! 回写）。文件形状与 §7.7 `trust/project.json` 同款：条目数组 + decidedAt。
//!
//! Q44 动态许可：免审批许可**不得**只绑展示名（归一名会因碰撞后缀漂移），
//! 绑定三元组 = 启动指纹（服务器身份）+ 原始工具名 + 已确认输入 schema 的
//! SHA-256 哈希。新增工具 / 输入 schema 变化 → 三元组不命中 → 首次调用
//! 重新确认；服务器启动指纹变化 → 全部工具许可连带失效（启动信任重核在
//! 先，许可自然作废）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::config::McpScope;

/// 信任与许可存储的目录/文件（§7.7 trust 域）。
const TRUST_DIR: &str = "trust";
const LAUNCH_TRUST_FILE: &str = "mcp.json";
const TOOL_LICENSE_FILE: &str = "mcp-tools.json";

/// 服务器条目的输入（指纹计算，§9.7 逐字段）。
#[derive(Debug, Clone)]
pub struct TrustSubject {
    /// 作用域。
    pub scope: McpScope,
    /// 项目 id（全局作用域为 None）。
    pub project_id: Option<String>,
    /// 配置里的服务器名。
    pub server_name: String,
    /// 配置文件 canonical 路径。
    pub canonical_config_path: String,
    /// 配置文件内容（整份文件字节——同一文件内其他服务器改动也会让指纹
    /// 变化并触发重问；这是 §9.7 "sha256(config)" 的字面语义：config 指
    /// 文件整体，改动可感知比改动不可感知更安全）。
    pub config_bytes: Vec<u8>,
}

/// 计算启动指纹（§9.7）：`sha256(scope|projectId|name|canonicalPath|sha256(bytes))`。
/// 返回完整 64 位 hex（展示面可用前 16 位，存储面用全量）。
pub fn launch_fingerprint(subject: &TrustSubject) -> String {
    let content_hash = hex(&Sha256::digest(&subject.config_bytes));
    let tuple = format!(
        "{}|{}|{}|{}|{}",
        subject.scope.as_str(),
        subject.project_id.as_deref().unwrap_or(""),
        subject.server_name,
        subject.canonical_config_path,
        content_hash
    );
    hex(&Sha256::digest(tuple.as_bytes()))
}

/// 字节 → 小写 hex。
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 输入 schema 的哈希（Q44 许可三元组第三元；schema JSON 规范化后取
/// SHA-256 前 16 hex——16 位足够防碰撞且便于展示）。
pub fn schema_fingerprint(schema: &serde_json::Value) -> String {
    // 规范化：序列化即规范化（serde_json 序列化确定；对象键序在
    // preserve_order 下按原序，server 端 schema 内容不变即哈希不变）
    let canonical =
        serde_json::to_string(schema).unwrap_or_else(|_| String::from("<unserializable>"));
    let digest = Sha256::digest(canonical.as_bytes());
    hex(&digest[..8]).chars().take(16).collect()
}

/// 启动信任决策记录（`trust/mcp.json` 条目，§7.7 同款形状）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LaunchTrustEntry {
    /// 启动指纹（完整 hex）。
    pub fingerprint: String,
    /// 决策时间（epoch ms）。
    pub decided_at: i64,
    /// 决策：remember（记住此版本）。deny 是"本次拒绝"，不产生记录——
    /// 用户再点连接时仍会询问。
    pub decision: LaunchDecision,
}

/// 启动信任决策值（§3.3 #46 mcp_set_launch_trust 的 decision 面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum LaunchDecision {
    /// 记住此版本（同指纹免问）。
    Remember,
    /// 撤销信任（管理页显式撤销；下次连接重新询问）。
    Revoke,
}

/// TOFU 启动信任存储（`~/.craftwave/trust/mcp.json`）。
#[derive(Debug, Clone)]
pub struct TrustStore {
    path: PathBuf,
}

// TrustStore 的读写方法。
impl TrustStore {
    /// 构造：数据目录根（文件 = `<data>/trust/mcp.json`）。
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(TRUST_DIR).join(LAUNCH_TRUST_FILE),
        }
    }

    /// 读取全部信任记录（文件缺失 = 空）。
    pub fn entries(&self) -> Vec<LaunchTrustEntry> {
        read_json_or_default(&self.path)
    }

    /// 指纹是否已被"记住"（同指纹免问）。
    pub fn is_trusted(&self, fingerprint: &str) -> bool {
        self.entries()
            .iter()
            .any(|e| e.fingerprint == fingerprint && e.decision == LaunchDecision::Remember)
    }

    /// 指纹的信任状态（管理页信任状态列：已记住 / 待决策 / 已撤销）。
    pub fn status_of(&self, fingerprint: &str) -> TrustStatus {
        match self
            .entries()
            .into_iter()
            .find(|e| e.fingerprint == fingerprint)
        {
            Some(e) => match e.decision {
                LaunchDecision::Remember => TrustStatus::Trusted,
                LaunchDecision::Revoke => TrustStatus::Revoked,
            },
            None => TrustStatus::Pending,
        }
    }

    /// 写入/覆盖一条决策（remember 或 revoke；原子写）。
    pub fn decide(
        &self,
        fingerprint: &str,
        decision: LaunchDecision,
        now_ms: i64,
    ) -> Result<(), String> {
        let mut entries: Vec<LaunchTrustEntry> = self.entries();
        match entries.iter_mut().find(|e| e.fingerprint == fingerprint) {
            Some(entry) => {
                entry.decision = decision;
                entry.decided_at = now_ms;
            }
            None => entries.push(LaunchTrustEntry {
                fingerprint: fingerprint.to_owned(),
                decided_at: now_ms,
                decision,
            }),
        }
        write_json_atomic(&self.path, &entries)
    }
}

/// 信任状态（管理页展示面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustStatus {
    /// 已记住（同指纹免问）。
    Trusted,
    /// 待决策（从未见过该指纹）。
    Pending,
    /// 已撤销（用户显式撤销；重连须重新询问）。
    Revoked,
}

impl TrustStatus {
    /// 快照载荷的字符串面（§3.3 #47 补充面："trusted"|"pending"|"revoked"）。
    pub fn as_str(self) -> &'static str {
        match self {
            TrustStatus::Trusted => "trusted",
            TrustStatus::Pending => "pending",
            TrustStatus::Revoked => "revoked",
        }
    }
}

/// Q44 工具级免审批许可（`trust/mcp-tools.json` 条目）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolLicenseEntry {
    /// 服务器启动指纹（许可绑定服务器身份，Q44）。
    pub server_fingerprint: String,
    /// 原始工具名（server 侧名单上的名字，非归一展示名）。
    pub tool_name: String,
    /// 已确认输入 schema 的哈希（[`schema_fingerprint`]）。
    pub schema_hash: String,
    /// 许可授予时间（epoch ms）。
    pub decided_at: i64,
}

/// Q44 工具级许可存储（`~/.craftwave/trust/mcp-tools.json`）。
#[derive(Debug, Clone)]
pub struct ToolLicenseStore {
    path: PathBuf,
}

// ToolLicenseStore 的读写与判定方法。
impl ToolLicenseStore {
    /// 构造：数据目录根。
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join(TRUST_DIR).join(TOOL_LICENSE_FILE),
        }
    }

    /// 许可三元组是否命中（Q44：服务器身份 + 原始工具名 + 已确认 schema
    /// 全等才免审批；任一变化都不命中）。
    pub fn is_licensed(
        &self,
        server_fingerprint: &str,
        tool_name: &str,
        schema_hash: &str,
    ) -> bool {
        self.entries().iter().any(|e| {
            e.server_fingerprint == server_fingerprint
                && e.tool_name == tool_name
                && e.schema_hash == schema_hash
        })
    }

    /// 是否存在同（服务器, 工具）但 schema 哈希不同的许可记录（Q44 的
    /// 「schema 变更未确认」标记）：always_allow 自动授权据此放行——有
    /// 冲突记录说明该工具的新版 schema 尚未经用户确认，不得自动放行。
    pub fn has_conflicting(
        &self,
        server_fingerprint: &str,
        tool_name: &str,
        schema_hash: &str,
    ) -> bool {
        self.entries().iter().any(|e| {
            e.server_fingerprint == server_fingerprint
                && e.tool_name == tool_name
                && e.schema_hash != schema_hash
        })
    }

    /// 授予一条许可（已存在同三元组则刷新时间；原子写）。
    pub fn grant(
        &self,
        server_fingerprint: &str,
        tool_name: &str,
        schema_hash: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        let mut entries = self.entries();
        match entries.iter_mut().find(|e| {
            e.server_fingerprint == server_fingerprint
                && e.tool_name == tool_name
                && e.schema_hash == schema_hash
        }) {
            Some(entry) => entry.decided_at = now_ms,
            None => entries.push(ToolLicenseEntry {
                server_fingerprint: server_fingerprint.to_owned(),
                tool_name: tool_name.to_owned(),
                schema_hash: schema_hash.to_owned(),
                decided_at: now_ms,
            }),
        }
        write_json_atomic(&self.path, &entries)
    }

    /// 撤销某指纹下的全部工具许可（启动信任撤销/配置指纹变更时调用——
    /// 服务器身份变了，旧工具许可不能延续）。
    pub fn revoke_server(&self, server_fingerprint: &str) -> Result<(), String> {
        let entries: Vec<ToolLicenseEntry> = self
            .entries()
            .into_iter()
            .filter(|e| e.server_fingerprint != server_fingerprint)
            .collect();
        write_json_atomic(&self.path, &entries)
    }

    /// 全量读取（单测与快照面用）。
    pub fn entries(&self) -> Vec<ToolLicenseEntry> {
        read_json_or_default(&self.path)
    }
}

/// 读 JSON 文件（缺失/损坏 = 空默认值；信任面宁缺勿炸——损坏文件按未
/// 信任处理，用户重新确认即可恢复）。
fn read_json_or_default<T: serde::de::DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// 当前毫秒时间戳（epoch ms；commands/pool 的 decidedAt 共用面）。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 原子写 JSON 数组/对象（util/atomic 的 serde_json::Value 面）。
fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建信任目录失败：{e}"))?;
    }
    let json = serde_json::to_value(value).map_err(|e| format!("序列化失败：{e}"))?;
    crate::util::atomic::write_json(path, &json).map_err(|e| format!("写信任文件失败：{e}"))
}
