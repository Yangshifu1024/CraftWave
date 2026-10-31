//! 首次存储格式升级前的可恢复备份守卫与恢复保护序列
//! （先建立、后扩展为通用守卫）。
//!
//! 名词解释：「存储格式升级」指新版应用在旧数据文件里写入旧版本读不懂的
//! 新格式（首个实例：compaction 条目）——升级一旦落盘，旧版应用就无法
//! 再完整读取这些数据。「守卫」指在第一次写入新格式之前强制先做一次完整
//! 备份：备份成功才放行写入；备份失败就暂停升级并报错，原数据一个字节
//! 都不动。这样用户在任何时候都能退回旧版本或手动恢复到升级前的时点。
//!
//! 通用守卫（扩展）：任何新格式写入点都调用
//! [`ensure_upgrade_backup`] 并携带自己的 `reason` 标识（如 `compaction`），
//! 幂等标记 `.upgrade-<reason>.done` 保证一个 reason 只备一次。守卫位于
//! storage 层的存储写入入口，自动更新与手动安装走同一条写入路径——
//! 「自动更新和手动安装共用存储入口守卫」由「守卫在 storage 层、不在
//! 更新器层」保证（compaction 条目是首个使用方，调用面不变）。
//!
//! 备份范围（字段级定稿）：被迁移的权威数据 + 恢复
//! 必需的关联文件——`config.json`（含凭据）、`histories/`（会话日志与子代理
//! 历史）、`sessions/`（索引/偏好/用量明细/队列/toolres 边车/图片 blob）、
//! `projects/`（项目注册表）、`mcp.json`（全局 MCP 配置）、`stats/`（日模型
//! 用量聚合）、`skills-state.json`（技能启停）。排除可重建缓存（`cache/`、
//! `quota.json`）、日志（`logs/`）、可丢界面状态（`ui-state.json`、
//! `window-state.json`）、用户工作区（`workspace/`，非被迁移的存储格式数据）
//! 与备份目录自身（不递归备份已有备份）。`trust/` 不备份： 规定恢复后
//! 信任必须重新确认，备份它反而诱导旧许可复活——不备份天然达成重核。
//! 备份保存在数据目录独立的 `backups/` 子目录，不进入 agent 可见根集
//! （workspace 收窄天然覆盖）；含凭据文件保持原有严格权限，复制不放宽。
//!
//! 恢复流程（字段级）：备份目录内生成
//! [`BACKUP_MANIFEST_FILE`]（机器可读清单，恢复验证对照面）、
//! [`RESTORE_GUIDE_FILE`]（人话恢复步骤）与 [`RESTORE_PENDING_NAME`]（恢复
//! 待处理标记）。用户关闭应用后按说明把清单项复制回数据目录，并把
//! `.restore-pending.json` 一并复制回数据目录根——下次启动时
//! [`apply_restore_protection`] 检测到标记即执行 保护序列（信任清空、
//! 外部目录授权清空、审批白名单清空、全部队列强制暂停），保护完成后删除
//! 标记。普通重启（无标记）不触碰上述任何面。
//!
//! 幂等语义：同一 `reason` 的升级标记存在时直接放行（一次升级只备一次，
//! 反复压缩不产生备份风暴）。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::storage::config_store::ConfigStore;
use crate::storage::index_store::SessionIndex;
use crate::storage::trust::{FileTrustStore, TrustStore};

/// 备份根目录（数据目录独立的 `backups/` 子目录）。
pub fn backups_dir(root: &Path) -> PathBuf {
    root.join("backups")
}

/// 升级完成标记文件路径（`backups/.upgrade-<reason>.done`）。
fn marker_path(root: &Path, reason: &str) -> PathBuf {
    backups_dir(root).join(format!(".upgrade-{reason}.done"))
}

/// 备份清单文件名（备份目录内；`backup-manifest.json`）。
pub const BACKUP_MANIFEST_FILE: &str = "backup-manifest.json";
/// 人话恢复步骤文件名（备份目录内； 「手动恢复步骤」承载面）。
pub const RESTORE_GUIDE_FILE: &str = "恢复说明.txt";
/// 恢复待处理标记名：备份目录内的空标记，用户按说明复制回数据目录根，
/// 下次启动驱动 [`apply_restore_protection`] 执行 保护序列。
pub const RESTORE_PENDING_NAME: &str = ".restore-pending.json";

/// 权威数据备份清单（字段级定稿）：目录项与文件项混合，
/// 存在才复制。顺序即恢复说明里的复制清单顺序。
pub const BACKUP_ITEMS: &[&str] = &[
    "histories",         // 目录：会话日志 + 子代理历史（权威正文）
    "sessions",          // 目录：index.json / prefs / usage / queue / toolres / imgblob
    "projects",          // 目录：项目注册表（directories.json，会话归属恢复必需）
    "stats",             // 目录：日/模型用量聚合（权威消耗记录）
    "config.json",       // 文件：应用配置（含 API Key 凭据，0600）
    "mcp.json",          // 文件：全局 MCP 服务器配置（含启动命令，凭据性质）
    "skills-state.json", // 文件：技能启停状态
];

/// 备份清单文件（`backup-manifest.json` 载荷；恢复验证的对照面）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    /// 格式版本（当前 1）。
    pub version: u32,
    /// 升级原因标识（与目录名 `<reason>` 段一致，如 "compaction"）。
    pub reason: String,
    /// 备份完成时刻（Unix 毫秒）。
    pub created_at: u128,
    /// 已复制的清单项（相对数据目录的路径；只列实际复制了的）。
    pub items: Vec<String>,
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

/// 人话恢复步骤文本（关闭应用手动恢复、仅回备份时点、升级后新增
/// 内容须另行保留、关联文件须一并恢复不能只拷会话正文）。
fn restore_guide_text(reason: &str) -> String {
    format!(
        "CraftWave 存储格式升级前备份（原因：{reason}）\n\
         \n\
         本目录是应用在首次写入新存储格式前创建的完整备份，用于需要时手动恢复。\n\
         \n\
         恢复步骤（务必先完全退出 CraftWave，包括托盘）：\n\
         1. 完全退出 CraftWave。\n\
         2. 把本目录中的以下内容逐项复制回数据目录 ~/.craftwave/（覆盖同名）：\n\
         3. 同时把本目录中的 .restore-pending.json 复制到 ~/.craftwave/ 根目录\n\
         （它会让下次启动自动重新核验信任、外部目录授权、审批记忆，并暂停\n\
         全部待发队列——恢复的是旧时点数据，旧许可不能直接生效）。\n\
         4. 重新启动 CraftWave。\n\
         \n\
         注意：\n\
         - 恢复只会回到备份时点；备份之后新增的会话与设置改动会丢失，须另行保留。\n\
         - 须整组恢复（包括 config.json 与 sessions/），只恢复部分文件会造成数据不一致。\n\
         - workspace/（会话工作区里的用户文件）、日志与缓存不在备份内。\n\
         - 旧版应用无法读取升级后的新增格式；升级后的数据如需在旧版打开，只能靠本备份。\n"
    )
}

/// 首次格式升级前的备份守卫。
///
/// - `reason`：升级原因标识（如 `compaction`），用于标记文件与备份目录命名；
/// - 升级标记已存在 → 直接放行（读标记里记录的备份目录名原样返回，幂等）；
/// - 否则把权威数据复制到 `backups/pre-<reason>-<ts>/`，写入
///   `backup-manifest.json`、`恢复说明.txt` 与 `.restore-pending.json`，
///   全部 fsync 后写升级标记；
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

    // 权威数据复制（清单，字段级）：存在才复制，
    // 全部命中失败即 Err（原数据不动）。
    let mut copied: Vec<String> = Vec::new();
    for item in BACKUP_ITEMS {
        let src = root.join(item);
        if !src.exists() {
            continue;
        }
        if src.is_dir() {
            copy_dir_recursive(&src, &target.join(item))?;
        } else {
            fs::copy(&src, target.join(item))?;
        }
        copied.push((*item).to_owned());
    }
    if copied.is_empty() {
        // 没有任何旧数据可备份（全新安装）：无需备份即可升级，直接写标记放行
        fs::write(&marker, format!("{dir_name}\n"))?;
        return Ok(target);
    }

    // 机器可读备份清单（恢复验证对照面：备份里有哪几项、什么时点）
    let manifest = BackupManifest {
        version: 1,
        reason: reason.to_owned(),
        created_at: stamp,
        items: copied.clone(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| std::io::Error::other(format!("备份清单序列化失败：{e}")))?;
    fs::write(target.join(BACKUP_MANIFEST_FILE), manifest_bytes)?;
    // 人话恢复步骤（手动恢复说明）与恢复待处理标记（保护序列驱动面）
    fs::write(target.join(RESTORE_GUIDE_FILE), restore_guide_text(reason))?;
    fs::write(
        target.join(RESTORE_PENDING_NAME),
        format!("{{\"version\":1,\"reason\":\"{reason}\",\"createdAt\":{stamp}}}\n"),
    )?;

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

/// 恢复保护序列的执行报告（启动序记账面：`app.log` 逐条落账）。
#[derive(Debug, Default, PartialEq)]
pub struct RestoreProtectionReport {
    /// 是否检测到恢复标记并执行了保护（false = 普通重启，未触碰任何面）。
    pub applied: bool,
    /// 已执行的保护动作（人话，逐条对应 各面）。
    pub actions: Vec<String>,
    /// 失败面（人话）。保护序列尽力而为、不阻塞启动，但失败必须记账，
    /// 不静默——用户据此知道哪些面可能残留旧许可。
    pub failures: Vec<String>,
}

/// 数据目录根的恢复标记路径（`~/.craftwave/.restore-pending.json`）。
pub fn restore_pending_path(root: &Path) -> PathBuf {
    root.join(RESTORE_PENDING_NAME)
}

/// 单个 queue.json 文件的强制暂停（全部待发队列保持暂停，等待用户
/// 明确继续）。直接改磁盘文件——调用时机在启动序早期，FollowUpManager 的
/// 内存队列表尚未惰性加载任何会话，磁盘即权威。
fn force_pause_queue_file(path: &Path) -> Result<bool, String> {
    let bytes = fs::read(path).map_err(|e| format!("读取失败：{e}"))?;
    let mut value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("解析失败：{e}"))?;
    // 已是暂停态就不重写（幂等，少一次落盘）
    if value.get("paused").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(false);
    }
    if let Some(obj) = value.as_object_mut() {
        obj.insert("paused".to_owned(), serde_json::Value::Bool(true));
    } else {
        return Err("文件不是 JSON 对象".to_owned());
    }
    let out = serde_json::to_vec(&value).map_err(|e| format!("序列化失败：{e}"))?;
    crate::util::atomic::write_atomic(path, &out).map_err(|e| format!("原子写失败：{e}"))?;
    Ok(true)
}

/// 恢复保护序列（字段级）：数据目录根存在
/// `.restore-pending.json`（用户按恢复说明复制回）即视为「发生了恢复」，
/// 执行保护后再进入正常服务：
///
/// ① 统一信任存储 `trust/project.json` 清空（项目资源信任 + MCP 启动信任
///    全部重新确认，FileTrustStore::clear_all）；
/// ② MCP 工具级许可 `trust/mcp-tools.json` 删除（审批记忆面，不复活
///    旧许可）；
/// ③ 会话索引全部条目的 `authorizedExternalDirs` 清空（外部目录授权须
///    重新授权）；
/// ④ 配置审批白名单 `approval.commandAllowlist` 清空（覆盖配置中的审批
///    白名单，防恢复已撤销的许可）；
/// ⑤ 全部 `sessions/*.queue.json` 强制 `paused: true`（待发队列保持暂停，
///    旧待审批请求不直接恢复成可执行批准；用户经 resume_followup_queue
///    明确继续）；
/// ⑥ 审批记忆与计划范围授权为进程内存面，重启天然为空（无动作，语义上
///    同属保护序列）；
/// ⑦ 完成后删除恢复标记（防重复触发）。
///
/// 保护序列尽力而为、不阻塞启动（用户已恢复旧数据，应用拒绝启动更糟），
/// 但每一步失败都进 [`RestoreProtectionReport::failures`] 记账不静默。
/// `config` 为空时跳过步骤 ④（headless 测试无 ConfigStore 场景）。
pub fn apply_restore_protection(
    root: &Path,
    config: Option<&ConfigStore<crate::providers::types::AppConfig>>,
) -> RestoreProtectionReport {
    let mut report = RestoreProtectionReport::default();
    if !restore_pending_path(root).exists() {
        return report; // 无恢复标记：普通重启，零动作
    }
    report.applied = true;

    // ① 统一信任存储清空（资源信任 + MCP 启动信任）
    match FileTrustStore::new(root).clear_all() {
        Ok(()) => report
            .actions
            .push("已清空资源信任与 MCP 启动信任（恢复后须重新确认）".to_owned()),
        Err(e) => report
            .failures
            .push(format!("清空信任存储失败（旧信任可能残留）：{e}")),
    }

    // ② MCP 工具级许可删除（审批记忆）
    let mcp_tools = root
        .join(crate::storage::trust::TRUST_DIR)
        .join("mcp-tools.json");
    match fs::remove_file(&mcp_tools) {
        Ok(()) => report
            .actions
            .push("已删除 MCP 工具级许可（恢复后首次调用重新确认）".to_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => report
            .failures
            .push(format!("删除 MCP 工具级许可失败（旧许可可能残留）：{e}")),
    }

    // ③ 外部目录授权清空（会话索引全部条目）
    match SessionIndex::new(root).clear_authorized_external_dirs() {
        Ok(count) if count > 0 => report
            .actions
            .push(format!("已清空 {count} 个会话的外部目录授权（须重新授权）")),
        Ok(_) => {}
        Err(e) => report
            .failures
            .push(format!("清空外部目录授权失败（旧授权可能残留）：{e:?}")),
    }

    // ④ 配置审批白名单清空（防恢复已撤销的许可）
    if let Some(config) = config {
        match config.update(|c| {
            c.approval.command_allowlist.clear();
            Ok(())
        }) {
            Ok(_) => report
                .actions
                .push("已清空配置中的命令审批白名单（防恢复已撤销的许可）".to_owned()),
            Err(e) => report.failures.push(format!(
                "清空审批白名单失败（旧白名单可能残留）：{}",
                e.message
            )),
        }
    }

    // ⑤ 全部队列文件强制暂停（含未被本次进程触碰过的会话——磁盘扫描全量）
    let sessions_dir = root.join("sessions");
    if let Ok(entries) = fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_queue = path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                !name.starts_with('.') && name.ends_with(".queue.json")
            });
            if !is_queue {
                continue;
            }
            match force_pause_queue_file(&path) {
                Ok(true) => report.actions.push(format!(
                    "已强制暂停待发队列：{}",
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                )),
                Ok(false) => {}
                Err(e) => report
                    .failures
                    .push(format!("暂停队列文件失败（{}）：{e}", path.display())),
            }
        }
    } else {
        // sessions/ 目录都不存在（全新目录误放标记）：无队列可暂停，不算失败
    }

    // ⑦ 删除恢复标记（防重复触发；删除失败只记账——标记残留只会在下次
    // 启动重复执行一遍幂等的保护动作，不产生数据风险）
    if let Err(e) = fs::remove_file(restore_pending_path(root)) {
        report
            .failures
            .push(format!("删除恢复标记失败（下次启动会重复执行保护）：{e}"));
    }
    report
}

#[cfg(test)]
#[path = "backup_test.rs"]
mod backup_test;
