//! 技能系统（存储、`skill` 工具、信任与版本
//! 字段级细则见本模块各结构体注释）。
//!
//! 名词解释：技能（skill）= 教智能体干专项活的一个目录——一份 `SKILL.md`
//! （YAML frontmatter 声明 name/description，正文是给模型看的操作指引）
//! 加任意随附资源文件（脚本、模板等，按相对路径随正文可用）。模型平时只在
//! 系统提示里看到「技能名 + 一句话简介」的清单（渐进披露，：不撑
//! 上下文），需要时经 `skill` 工具按需加载正文全文。
//!
//! 三来源发现（目录清单）：内置（随应用资源分发）< 全局
//! `<data>/skills/` < 项目 `<项目主目录>/.craftwave/skills/`，同名（frontmatter
//! `name`）后者胜。frontmatter 索引常驻内存（重扫才重建）；启停状态
//! 持久化在 `<data>/skills-state.json`。
//!
//! 版本一致性与整体信任：run 开始时快照「启用技能清单 + 正文
//! 全文 + 整目录版本指纹」，run 内 `skill` 工具只读快照——活跃 run 不混用
//! 旧 description 与新正文；重扫只影响管理页与下一 run。指纹覆盖 SKILL.md
//! 与全部附属资源（算法见 [`version_fingerprint`]），变更后下一 run 重新
//! 核验信任（信任判定与询问面在 host 装配层，见 commands.rs 注释）。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::safety::tier::Capability;
use crate::tools::{ToolAnnotations, ToolOrigin, ToolSpec};

// ---- 数据形状 ----

/// 技能来源（覆盖优先级 内置 < 全局 < 项目；skill_list 的 source 面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum SkillSource {
    /// 随应用资源分发（内置技能豁免信任）。
    Builtin,
    /// 全局数据目录 `<data>/skills/`（scope=global）。
    Global,
    /// 项目数据目录 `<项目主目录>/.craftwave/skills/`。
    Project,
}

// SkillSource 的方法。
impl SkillSource {
    /// 信任面作用域（projectTrust 载荷的 scope 字段；内置不询问无此值）。
    pub fn trust_scope(self) -> Option<&'static str> {
        match self {
            SkillSource::Builtin => None,
            SkillSource::Global => Some("global"),
            SkillSource::Project => Some("project"),
        }
    }
}

/// 技能索引条目（frontmatter 索引，常驻内存；`skill_list` 的行形状去掉
/// enabled 后的本体）。`dir` 供 `skill_inspect` 读正文预览用。
#[derive(Debug, Clone, PartialEq)]
pub struct SkillEntry {
    /// 技能名（frontmatter `name`，覆盖后唯一）。
    pub name: String,
    /// 来源（同名覆盖的胜出来源）。
    pub source: SkillSource,
    /// 一句话简介（frontmatter `description`）。
    pub description: String,
    /// 技能目录绝对路径（inspect 按需读盘——预览最新内容）。
    pub dir: PathBuf,
    /// 整目录版本指纹（[`version_fingerprint`]）。
    pub fingerprint: String,
}

/// 启停状态文件（`skills-state.json`）：禁用名清单。
/// `version` 缺省/旧文件补 1（读取面兜底），新写入恒 1。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillsState {
    /// 格式版本（演进防呆；当前恒 1）。
    #[serde(default = "default_state_version")]
    pub version: u32,
    /// 禁用技能名清单（按覆盖后的名字，不分来源）。
    #[serde(default)]
    pub disabled: Vec<String>,
}

/// version 字段的 serde 缺省（旧文件无 version 字段时补当前版本）。
fn default_state_version() -> u32 {
    1
}

// SkillsState 的手写 Default：version 恒 1（新写入的文件形状由源头保证）。
impl Default for SkillsState {
    fn default() -> Self {
        Self {
            version: 1,
            disabled: Vec::new(),
        }
    }
}

/// 单个 SKILL.md 的解析产物：frontmatter 字段 + 正文全文。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedSkill {
    /// frontmatter `name`。
    pub name: String,
    /// frontmatter `description`。
    pub description: String,
    /// frontmatter 结束符之后的正文全文（`skill` 工具回喂的面）。
    pub body: String,
}

// ---- frontmatter 解析 ----

/// frontmatter 防御上限（字节）：超限视为非法文件跳过（防超大文件拖垮扫描）。
const FRONTMATTER_MAX: usize = 64 * 1024;

/// 解析 SKILL.md 文本：`---\n{YAML}\n---\n` 头部取 name/description
/// （额外字段忽略，兼容 Claude 技能目录格式），其后为正文全文。
///
/// 规则：
/// - 首行必须恰为 `---`（允许 UTF-8 BOM 前缀与行尾 `\r`）；frontmatter 结束
///   行必须存在且为 `---`（或 `...`，YAML 文档结束符），否则 None；
/// - name 校验：非空、≤64 字符、`[a-zA-Z0-9_-]`（Claude 规范子集）；
/// - description 校验：trim 后非空（一行放不下可在 YAML 里折行，解析后
///   折叠为单倍空格）；
/// - name 不要求等于目录名（以 frontmatter 为准，宽于 Claude 校验——
///   覆盖与启停都按 frontmatter name 寻址，目录名只是存放位置）。
pub fn parse_skill_md(text: &str) -> Option<ParsedSkill> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    let first = lines.next()?.trim_end_matches('\r');
    if first != "---" {
        return None;
    }
    // 收集 frontmatter 行直到结束符；结束符缺失 → None。
    let mut yaml = String::new();
    let mut body_start: Option<usize> = None;
    let mut consumed = first.len() + 1; // + 换行
    for line in lines {
        let trimmed_end = line.trim_end_matches('\r');
        consumed += line.len() + 1;
        if trimmed_end == "---" || trimmed_end == "..." {
            body_start = Some(consumed);
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
        if yaml.len() > FRONTMATTER_MAX {
            return None;
        }
    }
    let body_start = body_start?;
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&yaml).ok()?;
    let name = scalar_string(value.get("name"))?;
    if !valid_skill_name(&name) {
        return None;
    }
    let description = scalar_string(value.get("description"))?;
    if description.trim().is_empty() {
        return None;
    }
    // 正文 = frontmatter 结束行之后（跳过一个紧跟的空行更贴合直觉；
    // 原样保留其余字节）。
    let body = text[body_start.min(text.len())..].trim_start_matches('\n');
    Some(ParsedSkill {
        name,
        description,
        body: body.to_owned(),
    })
}

/// YAML 标量 → String（仅接受字符串/数字标量；数组、映射等复杂值 → None）。
/// 多行折叠标量（YAML 折行）里的换行折叠为单空格，保持「一句话简介」形态。
fn scalar_string(value: Option<&serde_yaml_ng::Value>) -> Option<String> {
    let text = match value? {
        serde_yaml_ng::Value::String(s) => s.clone(),
        serde_yaml_ng::Value::Number(n) => n.to_string(),
        serde_yaml_ng::Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    Some(collapse_whitespace(&text))
}

/// 折叠连续空白（含换行）为单空格并去首尾。
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 技能名合法性：非空、≤64 字符、`[a-zA-Z0-9_-]`。
pub fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

// ---- 版本指纹（整体信任）----

/// 参与指纹的防御上限（路径边界）：每技能目录 ≤512 文件、总量 ≤64 MiB。
const FINGERPRINT_MAX_FILES: usize = 512;
const FINGERPRINT_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// 计算一个技能目录的整体版本指纹（算法逐字）：
/// `sha256("craftwave-skill-v1|" + name + "|" + Σ(相对路径 + "=" +
/// sha256(文件字节)))`，Σ 按相对路径字典序拼接、`|` 分隔。SKILL.md 与全部
/// 附属资源任一变化（新增/删除/改动）都改变指纹。
///
/// 越界（超文件数/超总量、symlink）返回 None——调用方把该技能按不可信
/// 未知源处理（不入索引，宁缺勿炸）。
pub fn version_fingerprint(name: &str, dir: &Path) -> Option<String> {
    let mut files = Vec::new();
    let mut total: u64 = 0;
    collect_files(dir, dir, &mut files, &mut total)?;
    files.sort();
    let mut aggregate = String::new();
    for (rel, content_hash) in files {
        aggregate.push('|');
        aggregate.push_str(&rel);
        aggregate.push('=');
        aggregate.push_str(&content_hash);
    }
    let tuple = format!("craftwave-skill-v1|{name}{aggregate}");
    Some(hex(&Sha256::digest(tuple.as_bytes())))
}

/// 递归收集（相对路径, 内容哈希）对；symlink 不追随（目录/文件任一为
/// symlink 即跳过）；超防御上限返回 None（短路终止）。
fn collect_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, String)>,
    total: &mut u64,
) -> Option<()> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.into_iter().flatten() {
        let path = entry.path();
        // symlink 判定：entry metadata 不追随链接（file_type 即链接本体）。
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_files(root, &path, out, total)?;
            if out.len() > FINGERPRINT_MAX_FILES || *total > FINGERPRINT_MAX_BYTES {
                return None;
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        *total += bytes.len() as u64;
        if out.len() >= FINGERPRINT_MAX_FILES || *total > FINGERPRINT_MAX_BYTES {
            return None;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        out.push((rel, hex(&Sha256::digest(&bytes))));
    }
    Some(())
}

/// 字节 → 小写 hex（与 mcp/trust 同款实现，域内独立避免跨域依赖）。
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---- 目录扫描与三来源合并 ----

/// 单来源根扫描：根下一级子目录各含 SKILL.md → 条目（frontmatter name 为
/// 准）；指纹算不出的目录跳过（宁缺勿炸）。
pub fn scan_source(root: &Path, source: SkillSource) -> Vec<SkillEntry> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.into_iter().flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let dir = entry.path();
        let Ok(text) = std::fs::read_to_string(dir.join("SKILL.md")) else {
            continue;
        };
        let Some(parsed) = parse_skill_md(&text) else {
            continue;
        };
        let Some(fingerprint) = version_fingerprint(&parsed.name, &dir) else {
            continue;
        };
        out.push(SkillEntry {
            name: parsed.name,
            source,
            description: parsed.description,
            dir,
            fingerprint,
        });
    }
    out
}

/// 三来源合并（覆盖优先级 内置 < 全局 < 项目）：按扫描序插入，同名
/// 后者覆盖前者。顺序 = 传入顺序（调用方保证 内置→全局→项目）。
pub fn merge_sources(sources: &[Vec<SkillEntry>]) -> Vec<SkillEntry> {
    let mut out: Vec<SkillEntry> = Vec::new();
    for list in sources {
        for entry in list.clone() {
            match out.iter_mut().find(|e| e.name == entry.name) {
                Some(existing) => *existing = entry,
                None => out.push(entry),
            }
        }
    }
    out
}

// ---- 注册表（进程级单例，仿 mcp/tools::McpToolRegistry）----

/// 注册表三来源根（configure 一次设置；project 随活跃会话刷新）。
#[derive(Debug, Clone, Default)]
pub struct RegistryRoots {
    /// 内置技能根（打包态 Resource/resources/skills；测试注入 fixture）。
    pub builtin: Option<PathBuf>,
    /// 全局技能根（生产 = `<data>/skills/`）。
    pub global: Option<PathBuf>,
    /// 项目技能根（生产 = `<项目主目录>/.craftwave/skills/`；临时会话 None）。
    pub project: Option<PathBuf>,
    /// 启停状态文件（生产 = `<data>/skills-state.json`）。
    pub state_file: Option<PathBuf>,
}

/// 技能注册表（进程级单例 + Arc 共享）：frontmatter 索引 + 启停状态。
///
/// 单例化理由（与 [`crate::mcp::tools::McpToolRegistry`] 同精神）：
/// `crate::tools::build_registry`（skill 工具的动态 description）、
/// `lookup_spec`、命令面与 run 装配快照都查全表，而部分调用点无 AppState
/// 入参——进程级单例一个入口覆盖。测试经 [`SkillRegistry::install_global`]
/// 注入独立实例。
#[derive(Default)]
pub struct SkillRegistry {
    inner: Mutex<RegistryInner>,
}

/// 注册表内部状态（合并清单 + 三来源根；会话级信任集已随 收敛到
/// [`crate::knowledge::session_trust::SessionTrustSet`]，本注册表不再持有）。
#[derive(Default)]
struct RegistryInner {
    roots: RegistryRoots,
    entries: Vec<SkillEntry>,
}

// SkillRegistry 的构造与查询方法。
impl SkillRegistry {
    /// 进程级单例（惰性建空表）。
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<SkillRegistry> = OnceLock::new();
        GLOBAL.get_or_init(Self::default)
    }

    /// 锁句柄（内部统一取锁路径）。
    fn lock(&self) -> std::sync::MutexGuard<'_, RegistryInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 把另一实例的内容整体搬进进程级单例（测试注入面）。
    pub fn install_from(other: &SkillRegistry) {
        let snapshot = {
            let src = other.lock();
            RegistryInner {
                roots: src.roots.clone(),
                entries: src.entries.clone(),
            }
        };
        *SkillRegistry::global().lock() = snapshot;
    }

    /// 设置三来源根并立即重扫（生产 setup 一次 + 项目根随会话刷新时调用）。
    pub fn configure(&self, roots: RegistryRoots) {
        {
            let mut inner = self.lock();
            inner.roots = roots;
        }
        self.reload();
    }

    /// 重扫三来源并重建索引（`skill_reload` 命令面； ）。
    pub fn reload(&self) {
        let (builtin, global, project) = {
            let inner = self.lock();
            (
                inner.roots.builtin.clone(),
                inner.roots.global.clone(),
                inner.roots.project.clone(),
            )
        };
        let merged = merge_sources(&[
            builtin
                .map(|root| scan_source(&root, SkillSource::Builtin))
                .unwrap_or_default(),
            global
                .map(|root| scan_source(&root, SkillSource::Global))
                .unwrap_or_default(),
            project
                .map(|root| scan_source(&root, SkillSource::Project))
                .unwrap_or_default(),
        ]);
        self.lock().entries = merged;
    }

    /// 合并清单快照（skill_list / 动态 description / run 装配共用）。
    pub fn list(&self) -> Vec<SkillEntry> {
        self.lock().entries.clone()
    }

    /// 按名查找（覆盖后清单）。
    pub fn find(&self, name: &str) -> Option<SkillEntry> {
        self.lock().entries.iter().find(|e| e.name == name).cloned()
    }

    /// 禁用判定（skills-state.json；文件缺失/损坏按全启用）。
    pub fn is_disabled(&self, name: &str) -> bool {
        let Some(path) = self.lock().roots.state_file.clone() else {
            return false;
        };
        read_state(&path).disabled.iter().any(|n| n == name)
    }

    /// 全部禁用名（skill_list 的 enabled 列拼装用）。
    pub fn disabled_names(&self) -> Vec<String> {
        let Some(path) = self.lock().roots.state_file.clone() else {
            return Vec::new();
        };
        read_state(&path).disabled
    }

    /// 启停切换：写入 skills-state.json（原子写）。
    /// 名字在注册表不存在时返回 false（命令层转 E_NOT_FOUND）。
    pub fn set_enabled(&self, name: &str, enabled: bool) -> bool {
        if self.find(name).is_none() {
            return false;
        }
        let Some(path) = self.lock().roots.state_file.clone() else {
            return false;
        };
        let mut state = read_state(&path);
        if enabled {
            state.disabled.retain(|n| n != name);
        } else if !state.disabled.iter().any(|n| n == name) {
            state.disabled.push(name.to_owned());
        }
        write_state(&path, &state).is_ok()
    }

    /// 启用技能清单（按注册表序过滤禁用；动态 description 与渐进披露
    /// 注入共用）。
    pub fn enabled_entries(&self) -> Vec<SkillEntry> {
        let disabled = self.disabled_names();
        self.list()
            .into_iter()
            .filter(|e| !disabled.iter().any(|n| n == &e.name))
            .collect()
    }
}

/// 读 skills-state.json（缺失/损坏 = 默认全启用； 宁缺勿炸）。
fn read_state(path: &Path) -> SkillsState {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// 原子写 skills-state.json（util/atomic， 同款）。
fn write_state(path: &Path, state: &SkillsState) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建数据目录失败：{e}"))?;
    }
    let json = serde_json::to_value(state).map_err(|e| format!("序列化失败：{e}"))?;
    crate::util::atomic::write_json(path, &json).map_err(|e| format!("写启停状态失败：{e}"))
}

// ---- skill 工具规格（M2 表 Meta 行）----

/// skill 工具参数（`{ name, args? }`，args 可选 string 透传）。
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SkillToolArgs {
    /// 技能名（frontmatter name）。
    pub name: String,
    /// 透传给技能正文语境的自由参数（可选；正文自行解释）。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub args: Option<String>,
}

/// 渐进披露的系统提示注入文本：仅「技能名 + description」一行
/// 清单（注入层末位），断言不含正文——测试钉住。入参 = run 快照的
/// 可用技能（启用 + 已信任，：description 与正文同版本）。
pub fn disclosure_text(entries: &[SkillSnapshotEntry]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }
    let mut out =
        String::from("可用技能（需要时先用 skill 工具加载技能正文，再按正文指引行动）：\n");
    for entry in entries {
        out.push_str(&format!("- {}：{}\n", entry.name, entry.description));
    }
    Some(out)
}

/// skill 工具的注册表规格（M2 表）：Meta 能力（全档放行）、参数
/// `{ name, args? }`；description 动态列出当前启用技能名清单（build_registry
/// 每次现读注册表——toggle/reload 后下一 run 生效）。无启用技能时给兜底
/// 文案（工具仍暴露，模型调用回「无启用技能」错误以便自纠）。
pub fn skill_spec() -> ToolSpec {
    let names: Vec<String> = SkillRegistry::global()
        .enabled_entries()
        .iter()
        .map(|e| e.name.clone())
        .collect();
    let description = if names.is_empty() {
        "加载一个技能的完整操作指引（当前没有可用的技能；不要调用本工具，除非用户明确要求加载某个技能）。"
            .to_owned()
    } else {
        format!(
            "加载一个技能的完整操作指引。当前可用技能：{}。调用前先读系统提示中的技能简介，按需加载正文后再行动。",
            names.join("、")
        )
    };
    ToolSpec::build(
        "skill",
        description,
        Capability::Meta,
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "技能名（系统提示清单中的名称）" },
                "args": { "type": "string", "description": "可选的自由参数，透传给技能语境" }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        ToolAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: false,
        },
        ToolOrigin::Static,
    )
}

// ---- run 快照（版本固定；host 装配层构造、executor 消费）----

/// run 内技能快照条目：run 开始时从注册表拍定，run 内 `skill` 工具只读
/// 快照（正文全文进内存——活跃 run 不混用旧 description 与新正文，
/// 旧版本天然可得，「不可得」分支不触发）。
#[derive(Debug, Clone, PartialEq)]
pub struct SkillSnapshotEntry {
    /// 技能名。
    pub name: String,
    /// 一句话简介（与注入清单同版本）。
    pub description: String,
    /// SKILL.md 正文全文（`skill` 工具回喂面）。
    pub body: String,
    /// 整目录版本指纹（信任核验与展示用）。
    pub fingerprint: String,
    /// 信任判定未通过（询问被拒/未决）：不注入清单；`skill` 调用回
    /// E_PROJECT_UNTRUSTED。
    pub untrusted: bool,
}

/// 从注册表构造 run 快照：启用且已信任（host 层已核验）的技能进 `entries`；
/// `rejected` 为未信任技能名（用于区分 E_PROJECT_UNTRUSTED 与未知技能）。
pub fn snapshot_for_run(entries: Vec<SkillSnapshotEntry>, rejected: Vec<String>) -> SkillSnapshot {
    SkillSnapshot { entries, rejected }
}

/// run 内技能快照（经 ExecutorEnv 注入执行器）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SkillSnapshot {
    /// 可用技能（启用 + 已信任 + 正文全文）。
    pub entries: Vec<SkillSnapshotEntry>,
    /// 存在但未信任的技能名（调用按 E_PROJECT_UNTRUSTED 拒绝）。
    pub rejected: Vec<String>,
}

// SkillSnapshot 的查询方法。
impl SkillSnapshot {
    /// 按名查可用技能。
    pub fn find(&self, name: &str) -> Option<&SkillSnapshotEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// 名字是否在「未信任拒绝」名单。
    pub fn is_rejected(&self, name: &str) -> bool {
        self.rejected.iter().any(|n| n == name)
    }

    /// `skill` 工具的执行结果面：头部标注 + 正文全文。供 executor
    /// 组装模型通道文本（超长截断同 由调用方处理）。
    pub fn body_text(&self, name: &str) -> Option<String> {
        let entry = self.find(name)?;
        Some(format!(
            "以下是技能 {} 的正文：\n\n{}",
            entry.name, entry.body
        ))
    }
}
