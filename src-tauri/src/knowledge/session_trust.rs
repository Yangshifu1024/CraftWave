//! 会话级信任集（§9.8 `allow_session`/`allow_once`「本次会话允许」的内存
//! 面；24 号票统一承载）。
//!
//! 语义：用户在 `projectTrust` 询问里选「本次会话允许」时，把 (会话,
//! 作用域, 资源名, 指纹) 记进内存集合——同会话后续 run 对同内容资源免问。
//! **不持久化**：应用退出即清、新会话自然不延续、备份恢复（Q41）因内存面
//! 不落盘天然不复活。持久信任走 [`crate::storage::trust::FileTrustStore`]。
//!
//! 24 号票收敛说明：技能（09 号票落在 SkillRegistry 内的 session_trusted）
//! 与项目指令（本票新增挂接点）两类询问共用本集合——键含资源名与作用域，
//! 判定精确到「哪个作用域的哪个资源同内容已允许」。

use std::sync::{Arc, Mutex};

/// 单条会话信任标记的键：（会话 id, 作用域字符串, 资源名, 指纹）。
/// 作用域取值与 projectTrust 载荷一致（"project" / "global"）。
type TrustKey = (String, String, String, String);

/// 会话级信任集（AppState 持有；Arc 克隆即同源句柄，主 run 与询问应答方
/// 共享同一集合）。
#[derive(Debug, Default, Clone)]
pub struct SessionTrustSet {
    inner: Arc<Mutex<Vec<TrustKey>>>,
}

// SessionTrustSet 的登记与查询方法。
impl SessionTrustSet {
    /// 空集合构造。
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一条「本次会话允许」（同键重复登记幂等）。
    pub fn mark(&self, session_id: &str, scope: &str, resource_name: &str, fingerprint: &str) {
        let key: TrustKey = (
            session_id.to_owned(),
            scope.to_owned(),
            resource_name.to_owned(),
            fingerprint.to_owned(),
        );
        let mut keys = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !keys.contains(&key) {
            keys.push(key);
        }
    }

    /// 查询：该会话对该作用域的同名同内容资源是否已「本次会话允许」。
    pub fn contains(
        &self,
        session_id: &str,
        scope: &str,
        resource_name: &str,
        fingerprint: &str,
    ) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|(sid, sc, name, fp)| {
                sid == session_id && sc == scope && name == resource_name && fp == fingerprint
            })
    }

    /// 登记条数（测试与诊断面）。
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// 集合是否为空（测试面）。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
