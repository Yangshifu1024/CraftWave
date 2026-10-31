//! 宿主内部状态保护——禁区清单与写目标判定。
//!
//! 名词解释：内部状态 = CraftWave 自身的配置、凭据、MCP 配置、索引、锁面
//! 等宿主私有文件。模型虽写不到数据目录本体（roots 收窄后 safe_join
//! 已挡 FileWrite 侧），但 command 的重定向/tee 可写任意路径——禁区清单
//! 就是给这类写目标划的硬边界。
//!
//! 职责边界：本模块只做**确定性硬拒**——可识别写
//! 目标命中禁区即拒绝（`E_INTERNAL_STATE`），不挂起、任何权限档与白名单
//! 不豁免；灰区 command（目标含未解析变量等不可判定形态）不做硬拒，交围
//! 栏字面量掩码保守 Confirm。读不禁（已声明风险：读 Key 类旁路由审批档
//! 兜底 + full_access 自担，不越权扩到读路径）。
//!
//! 禁区清单覆盖「配置/凭据引用/MCP 配置/索引/锁文件」的文件级
//! 展开，常量集中一处供测试枚举（判定覆盖面以下述清单为准，不扩大
//! 不缩小）：
//! - 数据侧（`~/.craftwave/` 内部数据，保留既有 `workspace/` 代理工作区
//!   例外——workspace 不在清单内即例外）：config.json（含 apiKey 明文）、
//!   projects/、sessions/、histories/、ui-state.json、stats/、quota/、
//!   trust/、logs/、cache/、全局 mcp.json；
//! - 项目侧（`<项目主目录>/.craftwave/` 全部）：project.json、项目
//!   mcp.json；
//! - 文件名面（任意目录层级）：`.segmeta.json`（段元数据，索引/锁面）。

use std::path::Path;
use std::path::PathBuf;

use super::fence::ast::Canonicalizer;

/// 数据侧目录禁区（相对数据目录根 `~/.craftwave/`）：项目注册表、会话
/// 存储、历史分段、用量统计、配额快照、信任记录、日志、派生索引缓存。
pub const DATA_SIDE_DIRS: &[&str] = &[
    "projects",
    "sessions",
    "histories",
    "stats",
    "quota",
    "trust",
    "logs",
    "cache",
];

/// 数据侧文件禁区（相对数据目录根）：应用配置（含 apiKey 明文，0600）、
/// 前端 UI 状态、全局 MCP 服务器配置。
pub const DATA_SIDE_FILES: &[&str] = &["config.json", "ui-state.json", "mcp.json"];

/// 文件名禁区（任意目录层级）：会话历史段元数据（索引 + 链式哈希锁面）。
pub const FORBIDDEN_FILE_NAMES: &[&str] = &[".segmeta.json"];

/// 项目侧目录禁区（相对项目主目录）：`<home>/.craftwave/` 全部（项目
/// 身份 project.json 与项目作用域 mcp.json 所在）。
pub const PROJECT_SIDE_DIRS: &[&str] = &[".craftwave"];

/// 内部状态禁区判定面：构造时把清单展开为绝对路径（canonical 尽力——失败
/// 回退词法路径，构造输入本就是宿主自有根、一般真实存在），之后对写目标
/// 反复判定。克隆即同源语义（路径集合为 owned 数据）。
#[derive(Debug, Clone)]
pub struct InternalStateGuard {
    /// 目录禁区（绝对路径）：目标等于该目录或落在其下即命中。
    dirs: Vec<PathBuf>,
    /// 文件禁区（绝对路径）：目标与该路径完全相等即命中。
    files: Vec<PathBuf>,
    /// 文件名禁区：目标最终文件名命中即禁区（任意层级）。
    names: Vec<&'static str>,
}

impl InternalStateGuard {
    /// 从数据目录根与项目主目录构造（两根须为绝对路径；canonicalize 失败
    /// 时按词法路径收编——禁区面宁保守，不因根未建立而放弃防护）。
    pub fn new(data_root: &Path, project_root: &Path) -> Self {
        // canonical 尽力：数据根在 macOS 上无符号链接歧义（/Users/…），项目
        // 主目录可能经 /var ↔ /private/var（临时会话回退 workspace 根），
        // 与写目标的 canonical 判定同域才可靠。
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let data = canon(data_root);
        let project = canon(project_root);
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for d in DATA_SIDE_DIRS {
            dirs.push(data.join(d));
        }
        for f in DATA_SIDE_FILES {
            files.push(data.join(f));
        }
        for d in PROJECT_SIDE_DIRS {
            dirs.push(project.join(d));
        }
        Self {
            dirs,
            files,
            names: FORBIDDEN_FILE_NAMES.to_vec(),
        }
    }

    /// 单个写目标的禁区判定：`raw` 为命令里的原文片段（命中消息引用用），
    /// `lexical` 为词法解析后的绝对路径（相对路径已按基准展开）。判定两遍：
    /// 词法路径直接比对 + canonical（解析 symlink）后比对——工作区内
    /// symlink 指入禁区同样拦下。命中返回 `Some(人话原因)`（工具结果
    /// `E_INTERNAL_STATE` 的 message，模型与用户都可读）。
    pub fn hit(&self, raw: &str, lexical: &Path, fs: &dyn Canonicalizer) -> Option<String> {
        // 文件名面：段元数据等固定名文件在任何层级都受保护。
        if let Some(name) = lexical.file_name().and_then(|n| n.to_str())
            && self.names.contains(&name)
        {
            return Some(self.describe(raw, "内部索引/锁面文件"));
        }
        if self.zone_of(lexical).is_some() {
            return Some(self.describe(raw, "宿主内部状态目录/配置文件"));
        }
        // canonical 面：symlink 指入禁区（词法在工作区内、真实落点在禁区）。
        if let Some(canonical) = fs.canonical(lexical)
            && canonical != lexical
            && self.zone_of(&canonical).is_some()
        {
            return Some(self.describe(raw, "符号链接指向宿主内部状态"));
        }
        None
    }

    /// 路径落在哪个禁区（目录前缀或文件全等）；返回禁区显示路径。
    fn zone_of(&self, p: &Path) -> Option<&Path> {
        let dir = self.dirs.iter().find(|d| p == *d || p.starts_with(*d));
        if let Some(d) = dir {
            return Some(d);
        }
        self.files.iter().find(|f| p == *f).map(|f| f.as_path())
    }

    /// 命中原因的人话描述（E_INTERNAL_STATE 的 message）。
    fn describe(&self, raw: &str, zone_kind: &str) -> String {
        format!(
            "试图写入宿主内部状态（{zone_kind}），禁止修改：{raw}。\
             该路径属于 CraftWave 自身的配置与数据，任何权限档与白名单都不能放行。"
        )
    }
}

#[cfg(test)]
#[path = "internal_state_test.rs"]
mod internal_state_test;
