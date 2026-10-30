//! 快照持久化（§7.7 `quota.json`，21 号票）：数据目录根下的行数组文件。
//!
//! 文件形态（§7.7 字面）：顶层数组 `[{ providerId, state, data?, fetchedAt,
//! lastOkAt? }]`——每家「有请求结果的行」（ok/error/rejected）一行；
//! no_key / unsupported 是即时可算的占位（不依赖网络），不入文件。
//!
//! 行数据的两条不变式：
//! - `data.entries` 恒为「最近一次成功的明细」：成功行整行覆盖；失败行落盘
//!   时**继承**盘上旧行的 entries 与 `lastOkAt`（离线兜底面不因失败丢失）；
//! - `lastOkAt` 恒为「上次成功取数时刻」：从未成功恒 None——它是 401/403/404
//!   分 `rejected`（从未成功，密钥/套餐被拒）与 `error`（曾成功，临时失效）
//!   的持久判据，也是前端「数据获取时间」的标注面。
//!
//! 并发纪律（CodeWave 同款）：刷新是「先并发取数、批次末统一落盘」，两次
//! 批次并发（手动连点 + 5 分钟周期）会各自读到旧状态再写回、后写覆盖前写。
//! 因此「读—改—写」收进进程内互斥（[`commit`]），且临界区内**重新读盘**再
//! 合并——临界区只有同步文件 IO，绝不跨 `.await` 持锁，无死锁风险。
//!
//! 损坏处理（宁缺勿炸，§7.7 stats 同款惯例）：文件解析失败 → 先备份
//! `quota.json.corrupt` 再按空继续（快照是可重取的缓存面，丢历史只损失
//! 「曾成功过」判据与离线兜底，绝不 panic、不阻塞查询）。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use super::{QuotaData, QuotaSnapshot, QuotaStatus};

/// 状态文件名（位于传入的数据目录根，§7.1/§7.7）。
pub const FILE_NAME: &str = "quota.json";

/// 损坏文件的隔离后缀：`quota.json` → `quota.json.corrupt`
/// （与 stats 聚合文件、index.json 的 `.corrupt` 同一约定）。
pub const BACKUP_SUFFIX: &str = ".corrupt";

/// 文件路径：`<数据目录>/quota.json`。
pub fn path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// 隔离文件路径：`<数据目录>/quota.json.corrupt`。
pub fn backup_path(dir: &Path) -> PathBuf {
    dir.join(format!("{FILE_NAME}{BACKUP_SUFFIX}"))
}

/// 读盘：文件不存在 → 空表（不创建——维持「无记录 = 文件不存在」不变式）；
/// 解析失败 → 备份留证后按空表继续（绝不 panic）。
pub fn load(dir: &Path) -> Vec<QuotaSnapshot> {
    let Ok(bytes) = std::fs::read(path(dir)) else {
        return Vec::new();
    };
    match serde_json::from_slice::<Vec<QuotaSnapshot>>(&bytes) {
        Ok(rows) => rows,
        Err(e) => {
            quarantine(dir);
            // 只记结构化摘要：serde 的 to_string() 会回显出错处的文本内容
            eprintln!(
                "[craftwave] quota.json 解析失败（{:?}，第 {} 行 {} 列），已备份为 {FILE_NAME}{BACKUP_SUFFIX} 并按空快照继续",
                e.classify(),
                e.line(),
                e.column()
            );
            Vec::new()
        }
    }
}

/// 隔离损坏文件：rename 到 `.corrupt`（可覆盖、只作证据、不参与任何判据）。
/// 失败只记日志——快照是缓存面，任何路径都不 panic。
fn quarantine(dir: &Path) {
    let from = path(dir);
    let to = backup_path(dir);
    let _ = std::fs::remove_file(&to);
    if std::fs::rename(&from, &to).is_ok() {
        return;
    }
    // rename 失败（跨设备等）：退化为复制留证；原文件保留原地（宁可下次
    // 再判损坏，也不丢现场）
    if std::fs::copy(&from, &to).is_ok() {
        let _ = std::fs::remove_file(&from);
    } else {
        eprintln!(
            "[craftwave] quota.json 备份失败（{:?} → {:?}），原文件保留原地",
            from, to
        );
    }
}

/// 原子写盘（`util::atomic`，§7.4 同款）；失败只记日志，不阻断额度展示。
fn save(dir: &Path, rows: &[QuotaSnapshot]) -> bool {
    let bytes = match serde_json::to_vec_pretty(rows) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("[craftwave] quota.json 序列化失败：{e}");
            return false;
        }
    };
    match crate::util::atomic::write_atomic(&path(dir), &bytes) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("[craftwave] quota.json 写入失败：{e}");
            false
        }
    }
}

/// 该供应商是否曾经成功取过数（401/403/404 → rejected / error 的判据）。
pub fn has_succeeded(rows: &[QuotaSnapshot], provider_id: &str) -> bool {
    rows.iter()
        .any(|row| row.provider_id == provider_id && row.last_ok_at.is_some())
}

/// 该供应商在盘上的历史行（离线兜底面来源）。
fn find<'a>(rows: &'a [QuotaSnapshot], provider_id: &str) -> Option<&'a QuotaSnapshot> {
    rows.iter().find(|row| row.provider_id == provider_id)
}

/// 失败行落盘形态：继承盘上旧行的 entries（最近一次成功明细）与 lastOkAt，
/// 其余字段（state/error/fetchedAt）取本批值。
fn merge_failure(row: &QuotaSnapshot, old: Option<&QuotaSnapshot>, now: &str) -> QuotaSnapshot {
    let inherited_entries = old
        .and_then(|old| old.data.as_ref())
        .map(|data| data.entries.clone())
        .unwrap_or_default();
    QuotaSnapshot {
        provider_id: row.provider_id.clone(),
        state: row.state,
        data: Some(QuotaData {
            display_name: row
                .data
                .as_ref()
                .map(|data| data.display_name.clone())
                .unwrap_or_default(),
            entries: inherited_entries,
            reason: None,
            error: row.data.as_ref().and_then(|data| data.error.clone()),
            key_source: row.data.as_ref().and_then(|data| data.key_source.clone()),
        }),
        fetched_at: now.to_string(),
        last_ok_at: old.and_then(|old| old.last_ok_at.clone()),
    }
}

/// upsert：同 providerId 覆盖，不产生重复行（保持既有行序，新行追加）。
fn upsert(rows: &mut Vec<QuotaSnapshot>, row: QuotaSnapshot) {
    match rows.iter_mut().find(|r| r.provider_id == row.provider_id) {
        Some(existing) => *existing = row,
        None => rows.push(row),
    }
}

/// 进程内互斥：`quota.json` 的「读—改—写」必须串行（模块注释「并发纪律」）。
fn io_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 批次末的原子提交（临界区内）：重新读盘 → 清孤儿 → 合并本批结果行 →
/// 判变化 → 原子写。返回是否写了盘（测试观测面）。
///
/// - `ids` = 当前配置的 provider id（清孤儿判据；供应商删除后其行一并消失）；
/// - `result_rows` = 本批「有请求结果的行」（ok / error / rejected）；
/// - `now` = 本批取数时刻（RFC3339 秒，与行内 fetchedAt 同值）；
/// - 本批无请求结果且无孤儿 → **不写盘**（目录里不出现文件）。
pub fn commit(dir: &Path, ids: &[String], result_rows: &[QuotaSnapshot], now: &str) -> bool {
    let _guard = io_lock();
    let mut latest = load(dir);
    let mut changed = false;
    // ① 孤儿清理（供应商已删除的行）
    let before = latest.len();
    latest.retain(|row| ids.contains(&row.provider_id));
    changed |= before != latest.len();
    // ② 合并本批结果行：成功整行覆盖；失败行继承历史明细与 lastOkAt
    for row in result_rows {
        let old = find(&latest, &row.provider_id);
        let merged = if row.state == QuotaStatus::Ok {
            row.clone()
        } else {
            merge_failure(row, old, now)
        };
        upsert(&mut latest, merged);
        changed = true;
    }
    if !changed {
        return false;
    }
    save(dir, &latest)
}
