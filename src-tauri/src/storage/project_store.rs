//! 项目注册表（FR-10-01，技术设计文档 §7.1）：`~/.craftwave/projects/directories.json`
//! （项目=名称+单主目录）与各主目录 `.craftwave/project.json`（注册清单，
//! 含 rootsSnapshot——M1 单主目录，快照即 [homeDir]）。
//!
//! 术语解释：「注册表」指记录全部项目的清单文件（每项目一行元数据）；「主目录
//! homeDir」指项目的工作区根目录；「roots」指一次对话允许访问的目录集合，
//! M1 恒等于项目主目录；「rootsSnapshot（roots 快照）」指注册时对 roots 的
//! 固定记录，写进项目主目录的 project.json；「重绑」指修改项目主目录时把
//! project.json 从旧目录搬到新目录。
//!
//! 读侧：`list_projects` 命令与 start_chat 的会话 roots 解析（项目会话
//! project_root = homeDir）；写侧：`save_project`（homeDir 变更重绑
//! project.json：旧目录移除、新目录写入）/ `delete_project`（只删注册与
//! project.json，绝不触碰项目数据目录本身——删除项目不等于删除用户文件）。
//!
//! 项目写频低，读走全量读盘（进程内 Mutex 串行化即可，§7.4 单进程口径）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::util::atomic::write_atomic;
use crate::util::ids::new_uuid;

/// §3.2 ProjectMeta（directories.json 元素与 list_projects 返回）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMeta {
    pub id: String,
    pub name: String,
    pub home_dir: String,
    pub created_at: i64,
}

/// directories.json 载荷（`{version:1, projects:[ProjectMeta]}`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectFile {
    version: u32,
    #[serde(default)]
    projects: Vec<ProjectMeta>,
}

/// 项目主目录 `.craftwave/project.json` 载荷（§7.1）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectManifest {
    pub id: String,
    pub name: String,
    pub home_dir: String,
    pub roots_snapshot: Vec<String>,
    pub created_at: i64,
}

/// 项目注册表读写器（数据根托管：`<root>/projects/directories.json`）。
pub struct ProjectStore {
    /// 注册表文件完整路径。
    path: PathBuf,
    /// 串行化读改写的进程内互斥锁。
    lock: Mutex<()>,
}

/// 注册表操作错误。
#[derive(Debug)]
pub enum ProjectError {
    /// homeDir 已被其他项目注册（一目录一项目，避免 roots 归属歧义）。
    DuplicateHome(String),
    /// 目标项目不存在。
    NotFound(String),
    /// 磁盘读写失败。
    Io(std::io::Error),
    /// 注册表文件内容不合法（解析失败）。
    Corrupt(String),
}

/// `ProjectStore` 的实现：list/home_of 读侧，save/delete 写侧，
/// 全部走「读整份注册表 → 修改 → 原子写回」流程。
impl ProjectStore {
    /// 以数据根构造注册表读写器（注册表固定在 `<root>/projects/directories.json`）。
    pub fn new(data_root: &Path) -> Self {
        Self {
            path: data_root.join("projects").join("directories.json"),
            lock: Mutex::new(()),
        }
    }

    /// 全量列表（注册序；前端自行分组排序）。
    pub fn list(&self) -> Result<Vec<ProjectMeta>, ProjectError> {
        let _guard = self.lock();
        Ok(self.read()?.projects)
    }

    /// 项目 id → 主目录（start_chat / workspace 命令的 roots 解析入口）。
    pub fn home_of(&self, project_id: &str) -> Option<PathBuf> {
        let _guard = self.lock();
        self.read()
            .ok()?
            .projects
            .into_iter()
            .find(|p| p.id == project_id)
            .map(|p| PathBuf::from(p.home_dir))
    }

    /// 新建/更新项目。homeDir 变更时重绑：旧目录 project.json 移除，
    /// 新目录写入注册清单（目录不存在则创建 `.craftwave/`）。
    pub fn save(
        &self,
        id: Option<&str>,
        name: &str,
        home_dir: &str,
    ) -> Result<ProjectMeta, ProjectError> {
        let _guard = self.lock();
        let mut file = self.read()?;
        let home_canon = canonical_home(home_dir)?;
        // 一目录一项目：homeDir 规范化后查重（排除自身）
        if file.projects.iter().any(|p| {
            p.id != id.unwrap_or_default()
                && canonical_home(&p.home_dir).is_ok_and(|h| h == home_canon)
        }) {
            return Err(ProjectError::DuplicateHome(home_dir.to_owned()));
        }
        let meta = match id {
            Some(existing) => {
                let slot = file
                    .projects
                    .iter_mut()
                    .find(|p| p.id == existing)
                    .ok_or_else(|| ProjectError::NotFound(existing.to_owned()))?;
                let old_home = slot.home_dir.clone();
                slot.name = name.to_owned();
                slot.home_dir = home_dir.to_owned();
                if canonical_home(&old_home).is_ok_and(|h| h != home_canon) {
                    let _ = fs::remove_file(manifest_path(Path::new(&old_home)));
                }
                slot.clone()
            }
            None => {
                let meta = ProjectMeta {
                    id: new_uuid(),
                    name: name.to_owned(),
                    home_dir: home_dir.to_owned(),
                    created_at: now_ms(),
                };
                file.projects.push(meta.clone());
                meta
            }
        };
        self.write(&file)?;
        write_manifest(&meta)?;
        Ok(meta)
    }

    /// 删除注册与主目录 project.json；返回被删条目（不存在 → None）。
    pub fn delete(&self, project_id: &str) -> Result<Option<ProjectMeta>, ProjectError> {
        let _guard = self.lock();
        let mut file = self.read()?;
        let Some(pos) = file.projects.iter().position(|p| p.id == project_id) else {
            return Ok(None);
        };
        let removed = file.projects.remove(pos);
        self.write(&file)?;
        let _ = fs::remove_file(manifest_path(Path::new(&removed.home_dir)));
        Ok(Some(removed))
    }

    /// 取互斥锁守卫（中毒时强行恢复继续用，注册表数据以磁盘为准不受影响）。
    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 读整份注册表；文件不存在视为空注册表（首次使用），解析失败报 Corrupt。
    fn read(&self) -> Result<ProjectFile, ProjectError> {
        let raw = match fs::read_to_string(&self.path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ProjectFile {
                    version: 1,
                    projects: Vec::new(),
                });
            }
            Err(e) => return Err(ProjectError::Io(e)),
        };
        serde_json::from_str(&raw).map_err(|e| ProjectError::Corrupt(e.to_string()))
    }

    /// 序列化整份注册表并原子写回（临时文件 + 改名，读者不会看到半个文件）。
    fn write(&self, file: &ProjectFile) -> Result<(), ProjectError> {
        let bytes =
            serde_json::to_vec_pretty(file).map_err(|e| ProjectError::Corrupt(e.to_string()))?;
        write_atomic(&self.path, &bytes).map_err(ProjectError::Io)
    }
}

/// 主目录 project.json 路径（§7.1：`<home>/.craftwave/project.json`）。
fn manifest_path(home: &Path) -> PathBuf {
    home.join(".craftwave").join("project.json")
}

/// 注册清单写入（目录不存在则创建 `.craftwave/`；rootsSnapshot=[homeDir]）。
fn write_manifest(meta: &ProjectMeta) -> Result<(), ProjectError> {
    let path = manifest_path(Path::new(&meta.home_dir));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(ProjectError::Io)?;
    }
    let manifest = ProjectManifest {
        id: meta.id.clone(),
        name: meta.name.clone(),
        home_dir: meta.home_dir.clone(),
        roots_snapshot: vec![meta.home_dir.clone()],
        created_at: meta.created_at,
    };
    let bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|e| ProjectError::Corrupt(e.to_string()))?;
    write_atomic(&path, &bytes).map_err(ProjectError::Io)
}

/// homeDir 规范化（消除软链接、相对路径等形态差异后的标准路径，用作存在性
/// 比较的键；目录尚不存在时按字面路径比较，注册后再创建也仍能查重）。
fn canonical_home(home_dir: &str) -> Result<PathBuf, ProjectError> {
    let path = Path::new(home_dir);
    match fs::canonicalize(path) {
        Ok(canon) => Ok(canon),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(e) => Err(ProjectError::Io(e)),
    }
}

/// 当前 Unix 时间戳（毫秒）；取不到系统时间时返回 0。
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
