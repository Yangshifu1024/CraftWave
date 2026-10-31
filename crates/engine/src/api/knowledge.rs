//! 技能管理域 API：多来源清单、启停、信任撤销、重扫与正文预览。
//!
//! 平移来源：`host/commands/knowledge_cmds.rs`（四命令 + 信任撤销补充面；
//! run 装配面的 `ensure_skills_snapshot` 信任核验随 start_chat 装配树走）。
//! 项目作用域锚定（收敛点，同 MCP project 作用域）：设置页没有会话参数，
//! 项目技能根锚定「当前活跃会话的项目」（`project_context_of` 同一解析面）；
//! 无活跃会话/临时会话 → 项目来源为空。
//!
//! 内置技能根：打包态路径由 GPUI 壳启动时解析并写入
//! `AppState.resource_skills_dir`（壳资源布局属壳装配职责——旧壳的
//! CARGO_MANIFEST_DIR 源码树回退不再适用；dev 态壳应自行传
//! `<repo>/resources/skills`）。

use serde::{Deserialize, Serialize};

use crate::app_state::AppState;
use crate::knowledge::skills::{
    RegistryRoots, SkillRegistry,
};
use crate::knowledge::trust::ProjectTrustStore;
use crate::util::errors::{E_INVALID_ARG, IpcError};

// ---- 命令契约类型（与旧壳一字不差） ----

/// `skill_list` 行契约（逐字段 + 补充面 trustStatus）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SkillListEntry {
    /// 技能名（frontmatter name，覆盖后唯一）。
    pub name: String,
    /// 来源（"builtin" | "global" | "project"）。
    pub source: crate::knowledge::skills::SkillSource,
    /// 是否启用（禁用清单之外）。
    pub enabled: bool,
    /// 一句话简介（frontmatter description）。
    pub description: String,
    /// 信任状态（撤销 UI 数据面）："trusted"（记住此版本）/
    /// "pending"（待决策）/ "revoked"（已显式撤销）；内置技能豁免信任恒
    /// "trusted"。
    pub trust_status: String,
}

/// `skill_revoke_trust` 参数契约（撤销 UI 落盘面）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SkillRevokeTrustArgs {
    /// 技能名。
    pub name: String,
}

/// `skill_toggle` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SkillToggleArgs {
    /// 技能名。
    pub name: String,
    /// 目标状态（true = 启用）。
    pub enabled: bool,
}

/// `skill_inspect` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SkillInspectArgs {
    /// 技能名。
    pub name: String,
}

/// `skill_inspect` 返回契约：SKILL.md 正文（frontmatter
/// 已剥离——预览给用户看正文，元数据在列表行）。不走工具、不进上下文。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SkillInspectResult {
    /// 正文全文（Markdown）。
    pub markdown: String,
}

// ---- 三来源根解析（命令面；run 装配面随 start_chat 树走） ----

/// 构造当前上下文的三来源根（数据目录 + 活跃会话的项目）。
fn registry_roots_of(app: &AppState) -> RegistryRoots {
    let data_dir = app.store.root().to_path_buf();
    RegistryRoots {
        builtin: app.resource_skills_dir.clone().filter(|d| d.is_dir()),
        global: Some(data_dir.join("skills")),
        project: crate::knowledge::instructions::project_context_of(app)
            .map(|(_, home)| home.join(".craftwave").join("skills")),
        state_file: Some(data_dir.join("skills-state.json")),
    }
}

/// 按当前上下文配置注册表并重扫（命令面统一入口——项目根随活跃
/// 会话切换，每次都全量重扫保证索引最新；技能目录小，成本可忽略）。
pub fn configure_registry(app: &AppState) {
    SkillRegistry::global().configure(registry_roots_of(app));
}

// ---- 五命令 ----

/// `skill_list`：多来源合并清单（合并 + 启停标注 + 信任状态）。
pub fn skill_list(app: &AppState) -> Result<Vec<SkillListEntry>, IpcError> {
    configure_registry(app);
    let registry = SkillRegistry::global();
    let disabled = registry.disabled_names();
    let trust_store = ProjectTrustStore::new(app.store.root());
    Ok(registry
        .list()
        .into_iter()
        .map(|entry| {
            // 信任状态：内置豁免恒 trusted；其余按统一存储的
            // (project, 指纹) 状态。
            let trust_status = if entry.source == crate::knowledge::skills::SkillSource::Builtin {
                "trusted".to_owned()
            } else {
                trust_store
                    .status_of_project(&entry.fingerprint)
                    .as_str()
                    .to_owned()
            };
            SkillListEntry {
                enabled: !disabled.iter().any(|n| n == &entry.name),
                name: entry.name,
                source: entry.source,
                description: entry.description,
                trust_status,
            }
        })
        .collect())
}

/// `skill_toggle`：启停切换（禁用技能持久化）。技能不存在 → E_NOT_FOUND。
pub fn skill_toggle(app: &AppState, args: SkillToggleArgs) -> Result<(), IpcError> {
    configure_registry(app);
    if !SkillRegistry::global().set_enabled(&args.name, args.enabled) {
        return Err(IpcError::not_found(format!("技能不存在：{}", args.name)));
    }
    Ok(())
}

/// `skill_revoke_trust`：显式撤销技能信任（写 revoke 记录，覆盖旧
/// remember）——下次 run 装配重新询问（安全权限撤销立即生效：快照只冻结
/// 正文版本，信任判定每 run 重做）。技能不存在 → E_NOT_FOUND；内置技能
/// 豁免信任，撤销无意义 → E_INVALID_ARG。
pub fn skill_revoke_trust(app: &AppState, args: SkillRevokeTrustArgs) -> Result<(), IpcError> {
    configure_registry(app);
    let entry = SkillRegistry::global()
        .find(&args.name)
        .ok_or_else(|| IpcError::not_found(format!("技能不存在：{}", args.name)))?;
    if entry.source == crate::knowledge::skills::SkillSource::Builtin {
        return Err(IpcError::new(
            E_INVALID_ARG,
            format!(
                "内置技能 {} 与应用同源分发，豁免信任，无需撤销",
                args.name
            ),
        ));
    }
    let trust_store = ProjectTrustStore::new(app.store.root());
    trust_store
        .revoke(
            &args.name,
            &entry.fingerprint,
            crate::storage::trust::now_ms(),
        )
        .map_err(IpcError::invalid_arg)
}

/// `skill_reload`：重扫全部来源并重建索引（活跃 run 的快照
/// 不受影响——重扫只更新管理页与下一 run 使用的版本）。
pub fn skill_reload(app: &AppState) -> Result<(), IpcError> {
    configure_registry(app);
    SkillRegistry::global().reload();
    Ok(())
}

/// `skill_inspect`：正文预览（frontmatter 剥离；直接读盘最新
/// 内容，不走工具、不进上下文）。格式非法 → E_INVALID_ARG。
pub fn skill_inspect(app: &AppState, args: SkillInspectArgs) -> Result<SkillInspectResult, IpcError> {
    configure_registry(app);
    let entry = SkillRegistry::global()
        .find(&args.name)
        .ok_or_else(|| IpcError::not_found(format!("技能不存在：{}", args.name)))?;
    let text = std::fs::read_to_string(entry.dir.join("SKILL.md"))
        .map_err(|e| IpcError::io(format!("读取 SKILL.md 失败：{e}")))?;
    let parsed = crate::knowledge::skills::parse_skill_md(&text).ok_or_else(|| {
        IpcError::new(
            E_INVALID_ARG,
            format!("技能 {} 的 SKILL.md 格式非法", args.name),
        )
    })?;
    Ok(SkillInspectResult {
        markdown: parsed.body,
    })
}
