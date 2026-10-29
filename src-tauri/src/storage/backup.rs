//! 首次存储格式升级前的可恢复备份守卫（技术设计文档 §7.7 Q34/Q36，04 号票建立）。
//!
//! 名词解释：「存储格式升级」指新版应用在旧数据文件里写入旧版本读不懂的
//! 新格式（首个实例：compaction 条目，§6.1）——升级一旦落盘，旧版应用就无法
//! 再完整读取这些数据。「守卫」指在第一次写入新格式之前强制先做一次完整
//! 备份：备份成功才放行写入；备份失败就暂停升级并报错，原数据一个字节
//! 都不动。这样用户在任何时候都能退回旧版本或手动恢复到升级前的时点。
//!
//! 备份范围（Q36 定稿口径的当前子集）：被迁移的权威数据 + 恢复所需关联文件
//! ——会话日志（`histories/`）、会话索引与偏好（`sessions/`）、应用配置
//! （`config.json`）；排除可重建缓存（`cache/`）、日志（`logs/`）、UI 状态
//! （可丢的界面偏好）与备份目录自身。备份保存在数据目录独立的 `backups/`
//! 子目录，不进入 agent 可见根集（§6.2 workspace 收窄天然覆盖）。
//!
//! 与 33 号票（签名与更新）的关系：本函数是存储入口的共用守卫——自动更新
//! 与手动安装新版走同一条写入路径，Q34「自动更新和手动安装共用存储入口
//! 守卫」由「守卫在 storage 层、不在更新器层」保证。33 号票落地完整备份
//! 体系（清单/命名/升级标记细化）时演进本模块，调用面不变。
//!
//! 幂等语义：同一 `reason` 的升级标记存在时直接放行（一次升级只备一次，
//! 反复压缩不产生备份风暴）。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 备份根目录（§7.7：数据目录独立的 `backups/` 子目录）。
pub fn backups_dir(root: &Path) -> PathBuf {
    root.join("backups")
}

/// 升级完成标记文件路径（`backups/.upgrade-<reason>.done`）。
fn marker_path(root: &Path, reason: &str) -> PathBuf {
    backups_dir(root).join(format!(".upgrade-{reason}.done"))
}

/// 当前时间的 Unix 毫秒时间戳（取不到时兜底 0，只影响目录命名）。
fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 递归复制目录（不含符号链接解引用——数据目录内部不该出现软链，
/// 出现时按普通文件复制其内容）。`skip` 命中的顶层条目跳过。
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// 首次格式升级前的备份守卫（§7.7 Q34/Q36）。
///
/// - `reason`：升级原因标识（如 `compaction`），用于标记文件与备份目录命名；
/// - 升级标记已存在 → 直接放行（读标记里记录的备份目录名原样返回，幂等）；
/// - 否则把权威数据复制到 `backups/pre-<reason>-<ts>/`，全部成功后写标记；
/// - 任一步失败 → 返回 Err，**调用方必须暂停格式升级并告知用户**（原数据
///   未被改动——复制是只读源操作，唯一可能残留的是半份备份目录，不影响
///   旧版读取；下次重试会生成新的时间戳目录）。
///
/// 返回备份目录路径（诊断与手动恢复指引用）。
pub fn ensure_upgrade_backup(root: &Path, reason: &str) -> Result<PathBuf, std::io::Error> {
    let marker = marker_path(root, reason);
    if marker.exists() {
        // 幂等放行：标记内容 = 备份目录名（读不出按 backups 根兜底）
        let recorded = fs::read_to_string(&marker)
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty());
        return Ok(match recorded {
            Some(name) => backups_dir(root).join(name),
            None => backups_dir(root),
        });
    }
    let stamp = now_ms();
    let dir_name = format!("pre-{reason}-{stamp}");
    let target = backups_dir(root).join(&dir_name);
    fs::create_dir_all(&target)?;

    // 权威数据清单（Q36 当前子集；33 号票演进时扩充）：
    // histories/（会话日志）、sessions/（索引 + 偏好 + toolres 边车）、config.json
    let mut copied_any = false;
    for item in ["histories", "sessions"] {
        let src = root.join(item);
        if src.exists() {
            copy_dir_recursive(&src, &target.join(item))?;
            copied_any = true;
        }
    }
    let config = root.join("config.json");
    if config.exists() {
        fs::copy(&config, target.join("config.json"))?;
        copied_any = true;
    }
    if !copied_any {
        // 没有任何旧数据可备份（全新安装）：无需备份即可升级，直接写标记放行
        fs::write(&marker, format!("{dir_name}\n"))?;
        return Ok(target);
    }

    // fsync 收口：备份文件落盘后再写标记（标记是「备份可用」的承诺，
    // 必须晚于备份本体可见）。逐文件 fsync 成本可接受（升级一次性动作）。
    fsync_dir_recursive(&target)?;
    fs::write(&marker, format!("{dir_name}\n"))?;
    Ok(target)
}

/// 递归 fsync 目录与其下的普通文件（unix；windows 无目录 fsync 语义，跳过）。
fn fsync_dir_recursive(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                fsync_dir_recursive(&path)?;
            } else {
                fs::File::open(&path)?.sync_all()?;
            }
        }
        fs::File::open(dir)?.sync_all()?;
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
    Ok(())
}

#[cfg(test)]
#[path = "backup_test.rs"]
mod backup_test;
