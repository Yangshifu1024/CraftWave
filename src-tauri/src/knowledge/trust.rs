//! 项目资源信任存储（§7.7 `trust/project.json` / §9.8；09 号票技能信任的
//! 最小版落地面，完整机制随 24 号票收敛）。
//!
//! TOFU（Trust On First Use）同构 [`crate::mcp::trust::TrustStore`]：首次
//! 使用未信任技能 → 弹 `projectTrust` 询问；「记住此版本」写同指纹条目，
//! 之后免问；指纹（Q21 整目录版本摘要）变化 → 记录不命中 → 重新询问。
//!
//! 决策值域（§9.8 定案）：仅 `remember` 产生记录；`allow_once` 是会话内存
//! 面（[`crate::knowledge::skills::SkillRegistry::mark_session_trusted`]），
//! `deny` 不产生记录（下一 run 重新询问）。`revoke`（显式撤销）随 24 号票
//! 提供 UI 与决策面后扩展。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 项目资源信任条目（§7.7 `trust/project.json` 行逐字段）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrustEntry {
    /// 资源名（当前 = 技能名；24 号票扩展 memory 等资源时同名域复用）。
    pub resource_name: String,
    /// 资源版本指纹（Q21 整目录摘要，完整 hex）。
    pub fingerprint: String,
    /// 决策时间（epoch ms）。
    pub decided_at: i64,
    /// 决策（当前值域仅 "remember"；扩展随 24 号票）。
    pub decision: String,
}

/// 项目资源信任存储（`<data>/trust/project.json`；全局与项目作用域共用
/// 一个文件，`resourceName` 当前命名空间 = 技能名，冲突面由指纹区分）。
#[derive(Debug, Clone)]
pub struct ProjectTrustStore {
    path: PathBuf,
}

// ProjectTrustStore 的读写方法。
impl ProjectTrustStore {
    /// 构造：数据目录根（文件 = `<data>/trust/project.json`）。
    pub fn new(data_dir: &Path) -> Self {
        Self {
            path: data_dir.join("trust").join("project.json"),
        }
    }

    /// 读取全部信任记录（文件缺失/损坏 = 空；信任面宁缺勿炸——损坏按未
    /// 信任处理，用户重新确认即可恢复）。
    pub fn entries(&self) -> Vec<ProjectTrustEntry> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// 指纹是否已被「记住」（同资源名 + 同指纹免问；资源名参与匹配——同名
    /// 技能换内容后指纹变，自然不命中）。
    pub fn is_trusted(&self, resource_name: &str, fingerprint: &str) -> bool {
        self.entries().iter().any(|e| {
            e.resource_name == resource_name
                && e.fingerprint == fingerprint
                && e.decision == "remember"
        })
    }

    /// 写入「记住此版本」决策（同资源名旧记录覆盖——技能内容变更重新确认
    /// 后旧指纹记录作废；原子写）。
    pub fn remember(
        &self,
        resource_name: &str,
        fingerprint: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        let mut entries: Vec<ProjectTrustEntry> = self
            .entries()
            .into_iter()
            .filter(|e| e.resource_name != resource_name)
            .collect();
        entries.push(ProjectTrustEntry {
            resource_name: resource_name.to_owned(),
            fingerprint: fingerprint.to_owned(),
            decided_at: now_ms,
            decision: "remember".to_owned(),
        });
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("创建信任目录失败：{e}"))?;
        }
        let json = serde_json::to_value(&entries).map_err(|e| format!("序列化失败：{e}"))?;
        crate::util::atomic::write_json(&self.path, &json)
            .map_err(|e| format!("写项目信任文件失败：{e}"))
    }
}

/// 当前毫秒时间戳（epoch ms；knowledge 域内独立，避免依赖 host/mcp 的私有面）。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
