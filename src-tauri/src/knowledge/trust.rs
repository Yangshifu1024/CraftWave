//! 项目资源信任的门面（§7.7 `trust/project.json` / §9.8；09 号票建面，
//! 24 号票收敛为统一存储的项目域视图）。
//!
//! TOFU（Trust On First Use）同 MCP 启动信任：首次使用未信任资源 → 弹
//! `projectTrust` 询问；「记住此版本」写同指纹条目，之后免问；指纹变化
//! → 记录不命中 → 重新询问。
//!
//! 决策值域（§9.8 定案）：仅 `remember` / `revoke` 产生记录；`allow_session`
//! 是会话内存面（AppState 的 [`crate::knowledge::session_trust::SessionTrustSet`]，
//! 24 号票起技能与项目指令共用），`deny` 不产生记录（下一 run 重新询问）。
//!
//! **24 号票收敛**：存储实现 = [`crate::storage::trust::FileTrustStore`]
//! （`trust/project.json`，scope 判别字段区分 project/mcp，§7.7 已回写）；
//! 本模块保留 09 号票接口面（is_trusted/remember/entries）并新增 revoke
//! （技能页撤销 UI 的落盘面）。`revoke` 记录保留在文件里（显式撤销态，
//! 撤销 UI 可展示；询问流程按未信任处理重新询问）。

use std::path::Path;

use crate::storage::trust::{FileTrustStore, TrustDecision, TrustRecord, TrustScope};
// 09 号票接口面依赖 trait 方法（entries/is_trusted/decide）。
use crate::storage::trust::TrustStore;

/// 项目资源信任的项目域门面（24 号票起 = 统一存储的薄包装，scope 恒
/// Project；文件 = `<data>/trust/project.json`）。
#[derive(Debug, Clone)]
pub struct ProjectTrustStore {
    unified: FileTrustStore,
}

// ProjectTrustStore 的读写方法（09 号票接口面 + 24 号票 revoke）。
impl ProjectTrustStore {
    /// 构造：数据目录根。
    pub fn new(data_dir: &Path) -> Self {
        Self {
            unified: FileTrustStore::new(data_dir),
        }
    }

    /// 读取项目域信任记录（统一存储的 scope=project 过滤视图；测试面沿用
    /// 09 号票的字段断言：resourceName/fingerprint/decidedAt/decision）。
    pub fn entries(&self) -> Vec<TrustRecord> {
        self.unified
            .entries()
            .into_iter()
            .filter(|e| e.scope == TrustScope::Project)
            .collect()
    }

    /// 指纹是否已被「记住」（同资源名 + 同指纹免问；资源名参与匹配——同名
    /// 技能换内容后指纹变，自然不命中）。
    pub fn is_trusted(&self, resource_name: &str, fingerprint: &str) -> bool {
        self.unified
            .is_trusted(TrustScope::Project, resource_name, fingerprint)
    }

    /// 按 (project, 指纹) 查信任状态（技能页信任列的判定面；同指纹多
    /// 记录取首条）。
    pub fn status_of_project(&self, fingerprint: &str) -> crate::storage::trust::TrustRecordStatus {
        self.unified.status_of(TrustScope::Project, fingerprint)
    }

    /// 写入「记住此版本」决策（同资源名同指纹旧记录覆盖；原子写）。
    pub fn remember(
        &self,
        resource_name: &str,
        fingerprint: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        self.decide(resource_name, fingerprint, TrustDecision::Remember, now_ms)
    }

    /// 显式撤销信任（24 号票，技能页撤销 UI）：写 revoke 记录（覆盖旧
    /// remember）——下次使用重新询问；同指纹 `is_trusted` 立即转 false
    /// （安全权限撤销立即生效，不受任何缓存延迟，Q20 口径）。
    pub fn revoke(
        &self,
        resource_name: &str,
        fingerprint: &str,
        now_ms: i64,
    ) -> Result<(), String> {
        self.decide(resource_name, fingerprint, TrustDecision::Revoke, now_ms)
    }

    /// 决策落盘的单一出口（remember/revoke 共用）。
    fn decide(
        &self,
        resource_name: &str,
        fingerprint: &str,
        decision: TrustDecision,
        now_ms: i64,
    ) -> Result<(), String> {
        self.unified.decide(TrustRecord {
            scope: TrustScope::Project,
            resource_name: resource_name.to_owned(),
            fingerprint: fingerprint.to_owned(),
            decided_at: now_ms,
            decision: decision.as_str().to_owned(),
        })
    }
}

/// 当前毫秒时间戳（epoch ms；24 号票起 re-export 统一存储实现）。
pub use crate::storage::trust::now_ms;
