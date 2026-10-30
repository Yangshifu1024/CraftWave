//! 产物登记（§10.7 / FR-10-06，28 号票）：本会话 AI 写过的文件清单。
//!
//! 术语解释：「产物（artifact）」指会话里模型经 write/edit 工具成功落盘的
//! 文件；「登记」指把写过的路径追加记录到每会话一份的边车文件里，供右栏
//! 产物面板回看与一键打开（模型侧行为完全不受影响——登记失败只记日志）。
//!
//! 文件布局（§7.7 一行口径，字段级本票定稿）：数据根下
//! `sessions/<sessionId>.artifacts.json`，内容 =
//! `{ version: 1, artifacts: [{ path, firstRunId, firstSeenAt, runCount, lastSeenAt }] }`。
//! 去重键 = 规范化绝对路径；重复命中 runCount+1、lastSeenAt 刷新。
//! `util::atomic` 原子写（§7.4）；调用方持 `MutexMap` 会话锁（与
//! segments/prefs 同锁序）串行化同会话读写；会话删除时随 `<sid>.*` 家族
//! 一并删除（§7.5 toolres 边车 GC 同款口径，GC 落点在
//! `SessionStore::delete_session`）。
//!
//! 容错口径：登记文件缺失 / JSON 损坏 → 按空清单处理（宁缺勿炸——登记是
//! 便利性数据，绝不阻塞工具执行主链路；损坏文件在下次登记时被空清单
//! 覆盖重建）。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::util::atomic::write_json;

/// 登记文件格式版本（§7.7：version 字段，结构演进时据此迁移）。
pub const ARTIFACTS_FORMAT_VERSION: u32 = 1;

/// 单条产物记录（存储面；§7.7 字段级定稿）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactEntry {
    /// 规范化绝对路径（去重键；symlink 由执行器 resolve 时已解析）。
    pub path: String,
    /// 首次写入该文件的 run id（溯源面）。
    pub first_run_id: String,
    /// 首次登记时刻（Unix 毫秒）。
    pub first_seen_at: i64,
    /// 命中次数（同会话内对该路径的第 N 次成功写入）。
    pub run_count: u32,
    /// 最近一次登记时刻（Unix 毫秒；面板按它倒序排列）。
    pub last_seen_at: i64,
}

/// 登记文件本体（`<sid>.artifacts.json` 的存储面形状）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactsFile {
    /// 格式版本（恒 [`ARTIFACTS_FORMAT_VERSION`]）。
    pub version: u32,
    /// 产物清单（按首次登记顺序追加；面板排序由查询面负责）。
    pub artifacts: Vec<ArtifactEntry>,
}

/// `list_artifacts` 返回条目（§3.3 #59l：`{ path, runCount, lastSeenAt, exists }`）。
/// exists 由 Rust 读取时探测（symlink 解析后判定）——已删除灰显一次 IPC
/// 搞定，前端不再逐路径试探。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRow {
    pub path: String,
    pub run_count: u32,
    pub last_seen_at: i64,
    pub exists: bool,
}

/// `list_artifacts` 返回契约（§3.3 #59l：`{ artifacts }`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ListArtifactsResult {
    pub artifacts: Vec<ArtifactRow>,
}

/// 登记文件路径（§7.7：`sessions/<sessionId>.artifacts.json`）。
///
/// 会话 id 合法性由调用方保证（登记源是已注册会话的运行中 run；命令层
/// 经 SessionStore 转调）。本函数只做路径拼接。
pub fn artifacts_path(root: &std::path::Path, session_id: &str) -> PathBuf {
    root.join("sessions")
        .join(format!("{session_id}.artifacts.json"))
}

/// 当前时间的 Unix 毫秒时间戳（取不到时兜底 0，与 SessionMeta 同口径）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 读取登记文件：缺失 / JSON 损坏 → 空清单兜底（模块头容错口径）。
pub fn load(root: &std::path::Path, session_id: &str) -> ArtifactsFile {
    let path = artifacts_path(root, session_id);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => ArtifactsFile::default(),
    }
}

/// 登记一次成功写入（读 → 去重合并 → 原子写）。调用方须已持该会话的
/// `MutexMap` 会话锁（串行化与 delete_session / 其他登记的并发）。
///
/// - `path`：规范化绝对路径（write/edit 执行器 resolve 后的 canonical
///   形态，symlink 已解析）；重复命中 runCount+1、lastSeenAt 刷新；
/// - `run_id`：本次写入所属 run（首次登记时记入 firstRunId）。
///
/// 落盘失败原样上抛（执行器只记日志不阻塞工具结果，§7.3 ⑤ 同口径）。
pub fn record(
    root: &std::path::Path,
    session_id: &str,
    run_id: &str,
    path: &str,
) -> std::io::Result<()> {
    let mut file = load(root, session_id);
    let ts = now_ms();
    if let Some(hit) = file.artifacts.iter_mut().find(|a| a.path == path) {
        hit.run_count = hit.run_count.saturating_add(1);
        hit.last_seen_at = ts;
    } else {
        file.artifacts.push(ArtifactEntry {
            path: path.to_owned(),
            first_run_id: run_id.to_owned(),
            first_seen_at: ts,
            run_count: 1,
            last_seen_at: ts,
        });
    }
    file.version = ARTIFACTS_FORMAT_VERSION;
    write_json(
        &artifacts_path(root, session_id),
        &serde_json::to_value(&file).map_err(std::io::Error::other)?,
    )
}

/// 产物清单查询（存在探测就地完成）：登记文件缺失/损坏 → 空清单（与
/// load 兜底一致）；`exists` = `fs::metadata`（symlink 解析后判定——目标
/// 已删的断链 symlink 记不存在）。排序 = 存在优先，其余按 lastSeenAt
/// 倒序（最近写的在前；前端灰显置底语义与此一致，后端排好省前端一手）。
pub fn list(root: &std::path::Path, session_id: &str) -> Vec<ArtifactRow> {
    let file = load(root, session_id);
    let mut rows: Vec<ArtifactRow> = file
        .artifacts
        .into_iter()
        .map(|a| ArtifactRow {
            exists: std::fs::metadata(&a.path).is_ok(),
            path: a.path,
            run_count: a.run_count,
            last_seen_at: a.last_seen_at,
        })
        .collect();
    rows.sort_by(|a, b| {
        b.exists
            .cmp(&a.exists)
            .then(b.last_seen_at.cmp(&a.last_seen_at))
    });
    rows
}
