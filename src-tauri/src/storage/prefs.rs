//! 会话偏好存取（§7.1 / FR-14-05，18 号票）：每会话一份 `<sid>.prefs.json`，
//! 记录「这个会话用哪个模型、什么思考档」，跨重启还原。
//!
//! 术语解释：「会话偏好」指用户在某个会话里做的持久化选择——当前只有
//! 模型覆盖（modelId）与 thinking 档（thinking）两项；**权限档明确不持久化**
//! （FR-14-05：重启回保守缺省档，防上次会话遗留的宽权限无声延续）。
//!
//! 文件布局（§7.1 既有行）：数据根下 `sessions/<sessionId>.prefs.json`，内容
//! 为 JSON 对象（camelCase 键，未设置字段省略）；随 `util::atomic` 原子写
//! （§7.4，临时文件 + fsync + rename）。会话删除时随 `SessionStore::
//! delete_session` 一并清除（session_tree.rs 既有路径）。
//!
//! 容错口径（票面「损坏/缺失 → 空偏好兜底」）：文件缺失、JSON 损坏、字段
//! 非法一律按空偏好处理——偏好只是便利性数据，绝不能阻塞会话加载。

use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::providers::reasoning::ThinkingLevel;
use crate::util::atomic::write_json;

/// 会话偏好（§3.3 #24/#25 载荷的存储面镜像）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionPrefs {
    /// 会话级模型覆盖（ProviderModel.id；None = 跟随全局 activeModelId）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// 会话 thinking 档（FR-02-08；None = 未显式设置，按 per-model
    /// reasoningEffort 回落，§11.4 优先级链）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingLevel>,
}

/// 会话偏好文件路径（§7.1：`sessions/<sessionId>.prefs.json`）。
///
/// 会话 id 合法性由调用方保证（命令层经索引存在性校验；运行时刷新源用
/// 已注册会话的 id）；本函数只做路径拼接。
pub fn prefs_path(root: &std::path::Path, session_id: &str) -> PathBuf {
    root.join("sessions")
        .join(format!("{session_id}.prefs.json"))
}

/// 读取会话偏好：文件缺失 / JSON 损坏 / 字段非法 → 空偏好兜底（不报错、
/// 不阻塞会话加载，票面定案）。
pub fn load_prefs(root: &std::path::Path, session_id: &str) -> SessionPrefs {
    let path = prefs_path(root, session_id);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => SessionPrefs::default(),
    }
}

/// 原子写会话偏好（§7.4：同目录临时文件 + fsync + rename）。
pub fn save_prefs(
    root: &std::path::Path,
    session_id: &str,
    prefs: &SessionPrefs,
) -> io::Result<()> {
    let value = serde_json::to_value(prefs).map_err(io::Error::other)?;
    write_json(&prefs_path(root, session_id), &value)
}
