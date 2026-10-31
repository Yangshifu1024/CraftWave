//! 技能命令实现 + run 装配面的信任核验与快照。
//!
//! 四命令全部是管理面（设置页技能组消费）；信任询问（`ensure_skills_snapshot`）
//! 是 run 装配面——start_chat 在 run 开始时经它拍「启用技能 + 正文全文 +
//! 指纹」快照（版本固定），对未信任技能同步弹 `projectTrust` 询问
//! （最小版，挂 `AppState.app_asks` 应用级槽表；完整机制）。
//!
//! 项目作用域锚定（收敛点，同 MCP project 作用域）：设置页
//! 与 run 装配都没有会话参数入参，项目技能根锚定「当前活跃会话的项目」
//! （`mcp::commands::project_context_of` 同一解析面）；无活跃会话/临时会话
//! → 项目来源为空。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use super::skills::{
    RegistryRoots, SkillEntry, SkillRegistry, SkillSnapshot, SkillSnapshotEntry, SkillSource,
    disclosure_text, parse_skill_md,
};
use super::trust::ProjectTrustStore;
use crate::backend::ask::{AskAnswer, AskOption, AskPayload, SkillTrustScope};
use crate::host::AppState;
use crate::util::errors::IpcError;

// —— 命令契约类型（–51；derive TS 进生成物）——

/// `skill_list` 行契约（逐字段 + 补充面 trustStatus）。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SkillListEntry {
    /// 技能名（frontmatter name，覆盖后唯一）。
    pub name: String,
    /// 来源（"builtin" | "global" | "project"）。
    pub source: SkillSource,
    /// 是否启用（禁用清单之外）。
    pub enabled: bool,
    /// 一句话简介（frontmatter description）。
    pub description: String,
    /// 信任状态（撤销 UI 数据面）："trusted"（记住此版本）/
    /// "pending"（待决策）/ "revoked"（已显式撤销）；内置技能豁免信任恒
    /// "trusted"。
    pub trust_status: String,
}

/// `skill_revoke_trust` 参数契约（补充面， 撤销 UI 落盘面）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SkillRevokeTrustArgs {
    /// 技能名。
    pub name: String,
}

/// `skill_toggle` 参数契约（逐字）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SkillToggleArgs {
    /// 技能名。
    pub name: String,
    /// 目标状态（true = 启用）。
    pub enabled: bool,
}

/// `skill_inspect` 参数契约（逐字）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SkillInspectArgs {
    /// 技能名。
    pub name: String,
}

/// `skill_inspect` 返回契约（逐字）：SKILL.md 正文（frontmatter
/// 已剥离——预览给用户看正文，元数据在列表行）。不走工具、不进上下文。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SkillInspectResult {
    /// 正文全文（Markdown）。
    pub markdown: String,
}

// —— 三来源根解析（run 装配与命令面共用）——

/// 内置技能根解析（目录清单）：打包态 Resource 目录优先（lib.rs setup
/// 经 AppHandle 解析好传入），开发态回退源码树 `src-tauri/resources/skills/`
/// （CARGO_MANIFEST_DIR 编译期常量，headless 测试与 dev 一致可用）；两者都
/// 不存在 → None（无内置来源，不报错）。
pub fn resolve_builtin_root(packaged: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(dir) = packaged
        && std::path::Path::new(&dir).is_dir()
    {
        return Some(dir);
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("skills");
    std::path::Path::new(&dev).is_dir().then_some(dev)
}

/// 构造当前上下文的三来源根（数据目录 + 活跃会话的项目）。
fn registry_roots_of(app: &AppState) -> RegistryRoots {
    let data_dir = app.store.root().to_path_buf();
    RegistryRoots {
        builtin: resolve_builtin_root(app.resource_skills_dir.clone()),
        global: Some(data_dir.join("skills")),
        project: crate::mcp::commands::project_context_of(app)
            .map(|(_, home)| home.join(".craftwave").join("skills")),
        state_file: Some(data_dir.join("skills-state.json")),
    }
}

/// 按当前上下文配置注册表并重扫（run 装配与命令面统一入口——项目根随活跃
/// 会话切换，每次都全量重扫保证索引最新；技能目录小，成本可忽略）。
pub fn configure_registry(app: &AppState) {
    SkillRegistry::global().configure(registry_roots_of(app));
}

// —— run 装配面：信任核验 + 版本快照——

/// run 开始时的技能快照与注入文本（start_chat 消费）。
pub struct RunSkills {
    /// run 内 `skill` 工具的只读快照（正文全文进内存）。
    pub snapshot: SkillSnapshot,
    /// 渐进披露注入文本（启用且已信任技能的「名 + description」清单；
    /// None = 零注入）。不含正文，测试钉住。
    pub disclosure: Option<String>,
}

/// 逐技能核验信任并拍快照：
///
/// - 内置技能豁免信任（与应用同源分发）；
/// - 全局/项目技能：统一存储（scope=project）同指纹「记住」记录 或 会话级
///   「本次会话允许」内存标记（[`SessionTrustSet`]，统一承载）→
///   已信任；
/// - 未信任 → 同步弹 `projectTrust` 询问（仿 MCP TOFU 挂 `app_asks`，用户
///   主动发消息触发所以等得起）：allow_session → 会话内存标记（同会话下一
///   run 免问）；remember → 写统一存储（此后跨重启免问）；deny → 本 run
///   按未信任处理（不产生记录，下一 run 重新询问）；
/// - 已信任技能读正文全文进快照；正文读取失败（文件被删/损坏）→
///   「旧版本不可得则停止相关加载」：跳过该技能并提示（eprintln）。
///
/// 多个未信任技能按清单序逐个询问（未信任技能通常 0–1 个）。
pub async fn ensure_skills_snapshot(app: &AppState, session_id: &str) -> RunSkills {
    configure_registry(app);
    let registry = SkillRegistry::global();
    let trust_store = ProjectTrustStore::new(app.store.root());
    let mut entries: Vec<SkillSnapshotEntry> = Vec::new();
    let mut rejected: Vec<String> = Vec::new();
    for entry in registry.enabled_entries() {
        match trust_of(app, session_id, &trust_store, &entry).await {
            SkillTrustVerdict::Trusted => {}
            SkillTrustVerdict::Denied => {
                rejected.push(entry.name);
                continue;
            }
        }
        // 已信任：读正文全文进快照（版本固定）。
        match std::fs::read_to_string(entry.dir.join("SKILL.md"))
            .ok()
            .and_then(|text| parse_skill_md(&text))
        {
            Some(parsed) => entries.push(SkillSnapshotEntry {
                name: entry.name,
                description: entry.description,
                body: parsed.body,
                fingerprint: entry.fingerprint,
                untrusted: false,
            }),
            None => {
                // 旧版本不可得（SKILL.md 在索引后被删/损坏）：停止相关加载
                // 并提示，本 run 不注入、调用按未知技能回错误。
                eprintln!(
                    "[craftwave] 技能 {} 的 SKILL.md 读取失败，本次运行不加载该技能",
                    entry.name
                );
            }
        }
    }
    let disclosure = disclosure_text(&entries);
    RunSkills {
        snapshot: super::skills::snapshot_for_run(entries, rejected),
        disclosure,
    }
}

/// 单技能信任判定（含询问交互）。
enum SkillTrustVerdict {
    /// 已信任（内置豁免 / 持久记录 / 会话标记 / 本次允许）。
    Trusted,
    /// 拒绝（本 run 不可用）。
    Denied,
}

/// 单技能信任判定：内置豁免 → 持久记录 → 会话标记 → 同步询问。
async fn trust_of(
    app: &AppState,
    session_id: &str,
    trust_store: &ProjectTrustStore,
    entry: &SkillEntry,
) -> SkillTrustVerdict {
    if entry.source == SkillSource::Builtin {
        return SkillTrustVerdict::Trusted;
    }
    // 作用域字符串（projectTrust 载荷与 SessionTrustSet 键同源）。
    let scope = match entry.source.trust_scope() {
        Some("global") => {
            if trust_store.is_trusted(&entry.name, &entry.fingerprint)
                || app
                    .session_trust
                    .contains(session_id, "global", &entry.name, &entry.fingerprint)
            {
                return SkillTrustVerdict::Trusted;
            }
            SkillTrustScope::Global
        }
        Some(_) => {
            if trust_store.is_trusted(&entry.name, &entry.fingerprint)
                || app.session_trust.contains(
                    session_id,
                    "project",
                    &entry.name,
                    &entry.fingerprint,
                )
            {
                return SkillTrustVerdict::Trusted;
            }
            SkillTrustScope::Project
        }
        // 内置技能（trust_scope None）已在入口豁免；防御分支按已信任收口。
        None => return SkillTrustVerdict::Trusted,
    };
    let payload = AskPayload::ProjectTrust {
        resource_name: entry.name.clone(),
        scope,
        fingerprint: entry.fingerprint.chars().take(16).collect(),
        options: project_trust_options(),
    };
    match ensure_project_trust_ask(app, payload).await.as_str() {
        "allow_session" => {
            let scope_str = match entry.source.trust_scope() {
                Some(s) => s,
                None => return SkillTrustVerdict::Trusted,
            };
            app.session_trust
                .mark(session_id, scope_str, &entry.name, &entry.fingerprint);
            SkillTrustVerdict::Trusted
        }
        "remember" => {
            let _ = trust_store.remember(
                &entry.name,
                &entry.fingerprint,
                crate::storage::trust::now_ms(),
            );
            SkillTrustVerdict::Trusted
        }
        _ => SkillTrustVerdict::Denied,
    }
}

/// projectTrust 三选项（本次会话允许 / 记住此版本 / 拒绝；与 mcpTrust
/// 同 id 面，前端按同一 Approval 应答协议处理）。技能与项目指令共用
/// （统一询问面）。
pub(crate) fn project_trust_options() -> Vec<AskOption> {
    vec![
        AskOption {
            id: "allow_session".to_owned(),
            label: "本次会话允许".to_owned(),
            recommended: None,
            mode: None,
        },
        AskOption {
            id: "remember".to_owned(),
            label: "记住此版本".to_owned(),
            recommended: None,
            mode: None,
        },
        AskOption {
            id: "deny".to_owned(),
            label: "拒绝".to_owned(),
            recommended: None,
            mode: None,
        },
    ]
}

/// 弹 `projectTrust` 询问并等待应答（技能与项目指令共用，
/// 抽取为统一询问面）：挂 `app_asks` 应用级槽表 + ask:opened（sessionId=""
/// 应用级桶，任意会话 Tab 都能应答）→ await resolve_ask 投递。返回选中的
/// optionId（deny/异常应答/通道断开统一归 "deny"）。单一出口清槽 +
/// ask:closed。
pub(crate) async fn ensure_project_trust_ask(app: &AppState, payload: AskPayload) -> String {
    let (tx, rx) = oneshot::channel::<AskAnswer>();
    let ask_id = uuid::Uuid::new_v4().to_string();
    {
        let mut asks = app
            .app_asks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        asks.insert(
            ask_id.clone(),
            crate::backend::ask::AppAskSlot {
                payload: payload.clone(),
                tx,
            },
        );
    }
    app.events.emit(
        crate::util::events::keys::ASK_OPENED,
        serde_json::json!({
            "sessionId": "",
            "askId": ask_id,
            "payload": payload,
        }),
    );
    let answer = rx.await;
    let option_id = match answer {
        Ok(AskAnswer::Approval { option_id, .. }) => option_id,
        _ => String::from("deny"),
    };
    app.app_asks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&ask_id);
    app.events.emit(
        crate::util::events::keys::ASK_CLOSED,
        serde_json::json!({ "askId": ask_id, "answered": true }),
    );
    option_id
}

// —— 四命令 ——

/// `skill_list`：多来源合并清单（合并 + 启停标注）。
#[tauri::command]
pub(crate) async fn skill_list(
    app: tauri::State<'_, AppState>,
) -> Result<Vec<SkillListEntry>, IpcError> {
    Ok(skill_list_core(&app))
}

/// skill_list 主体（headless 测试直驱）。
pub fn skill_list_core(app: &AppState) -> Vec<SkillListEntry> {
    configure_registry(app);
    let registry = SkillRegistry::global();
    let disabled = registry.disabled_names();
    let trust_store = ProjectTrustStore::new(app.store.root());
    registry
        .list()
        .into_iter()
        .map(|entry| {
            // 信任状态：内置豁免恒 trusted；其余按统一存储的
            // (project, 指纹) 状态。
            let trust_status = if entry.source == SkillSource::Builtin {
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
        .collect()
}

/// `skill_toggle`：启停切换（禁用技能持久化）。
#[tauri::command]
pub(crate) async fn skill_toggle(
    app: tauri::State<'_, AppState>,
    args: SkillToggleArgs,
) -> Result<(), IpcError> {
    skill_toggle_core(&app, &args.name, args.enabled)
}

/// skill_toggle 主体（headless 测试直驱）。
pub fn skill_toggle_core(app: &AppState, name: &str, enabled: bool) -> Result<(), IpcError> {
    configure_registry(app);
    if !SkillRegistry::global().set_enabled(name, enabled) {
        return Err(IpcError::not_found(format!("技能不存在：{name}")));
    }
    Ok(())
}

/// `skill_revoke_trust`（补充面， 撤销 UI 落盘面）：显式撤销
/// 技能信任（写 revoke 记录，覆盖旧 remember）——下次 run 装配重新询问
/// （安全权限撤销立即生效， 口径：不受 run 快照冻结延迟——快照只冻结
/// 正文版本，信任判定每 run 重做）。技能不存在 → E_NOT_FOUND；内置技能
/// 豁免信任，撤销无意义 → E_INVALID_ARG。
#[tauri::command]
pub(crate) async fn skill_revoke_trust(
    app: tauri::State<'_, AppState>,
    args: SkillRevokeTrustArgs,
) -> Result<(), IpcError> {
    skill_revoke_trust_core(&app, &args.name)
}

/// skill_revoke_trust 主体（headless 测试直驱）。
pub fn skill_revoke_trust_core(app: &AppState, name: &str) -> Result<(), IpcError> {
    configure_registry(app);
    let entry = SkillRegistry::global()
        .find(name)
        .ok_or_else(|| IpcError::not_found(format!("技能不存在：{name}")))?;
    if entry.source == SkillSource::Builtin {
        return Err(IpcError::invalid_arg(format!(
            "内置技能 {} 与应用同源分发，豁免信任，无需撤销",
            name
        )));
    }
    let trust_store = ProjectTrustStore::new(app.store.root());
    trust_store
        .revoke(name, &entry.fingerprint, crate::storage::trust::now_ms())
        .map_err(IpcError::invalid_arg)
}

/// `skill_reload`：重扫全部来源并重建索引（活跃 run 的快照
/// 不受影响——：重扫只更新管理页与下一 run 使用的版本）。
#[tauri::command]
pub(crate) async fn skill_reload(app: tauri::State<'_, AppState>) -> Result<(), IpcError> {
    skill_reload_core(&app);
    Ok(())
}

/// skill_reload 主体（headless 测试直驱）。
pub fn skill_reload_core(app: &AppState) {
    configure_registry(app);
    SkillRegistry::global().reload();
}

/// `skill_inspect`：正文预览（frontmatter 剥离；直接读盘最新
/// 内容，不走工具、不进上下文）。
#[tauri::command]
pub(crate) async fn skill_inspect(
    app: tauri::State<'_, AppState>,
    args: SkillInspectArgs,
) -> Result<SkillInspectResult, IpcError> {
    skill_inspect_core(&app, &args.name)
}

/// skill_inspect 主体（headless 测试直驱）。
pub fn skill_inspect_core(app: &AppState, name: &str) -> Result<SkillInspectResult, IpcError> {
    configure_registry(app);
    let entry = SkillRegistry::global()
        .find(name)
        .ok_or_else(|| IpcError::not_found(format!("技能不存在：{name}")))?;
    let text = std::fs::read_to_string(entry.dir.join("SKILL.md"))
        .map_err(|e| IpcError::io(format!("读取 SKILL.md 失败：{e}")))?;
    let parsed = parse_skill_md(&text).ok_or_else(|| {
        IpcError::new("E_INVALID_ARG", format!("技能 {name} 的 SKILL.md 格式非法"))
    })?;
    Ok(SkillInspectResult {
        markdown: parsed.body,
    })
}
