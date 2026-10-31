//! run 装配面的技能信任核验与版本快照（平移自旧壳
//! `host/commands/knowledge_cmds.rs` 的 `ensure_skills_snapshot`；
//! 四命令管理面已在 `api::knowledge`）。
//!
//! start_chat 在 run 开始时经 [`ensure_skills_snapshot`] 拍「启用技能 +
//! 正文全文 + 指纹」快照（版本固定），对未信任技能同步弹 `projectTrust`
//! 询问（最小版，挂 `AppState.app_asks` 应用级槽表；用户主动发消息触发
//! 所以等得起）。

use crate::app_state::AppState;
use crate::backend::ask::{AskPayload, SkillTrustScope};
use crate::knowledge::skills::{SkillEntry, SkillRegistry, SkillSnapshot, SkillSnapshotEntry, SkillSource, disclosure_text, parse_skill_md};
use crate::knowledge::trust::ProjectTrustStore;

/// run 开始时的技能快照与注入文本（start_chat 消费）。
pub(crate) struct RunSkills {
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
///   「本次会话允许」内存标记（`SessionTrustSet`，统一承载）→
///   已信任；
/// - 未信任 → 同步弹 `projectTrust` 询问（仿 MCP TOFU 挂 `app_asks`）：
///   allow_session → 会话内存标记（同会话下一 run 免问）；remember → 写统一
///   存储（此后跨重启免问）；deny → 本 run 按未信任处理（不产生记录，
///   下一 run 重新询问）；
/// - 已信任技能读正文全文进快照；正文读取失败（文件被删/损坏）→
///   「旧版本不可得则停止相关加载」：跳过该技能并提示（eprintln）。
///
/// 多个未信任技能按清单序逐个询问（未信任技能通常 0–1 个）。
pub(crate) async fn ensure_skills_snapshot(app: &AppState, session_id: &str) -> RunSkills {
    super::super::knowledge::configure_registry(app);
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
        snapshot: crate::knowledge::skills::snapshot_for_run(entries, rejected),
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
        options: crate::knowledge::instructions::project_trust_options(),
    };
    match crate::knowledge::instructions::ensure_project_trust_ask(app, payload).await.as_str() {
        "allow_session" => {
            let Some(scope_str) = entry.source.trust_scope() else {
                return SkillTrustVerdict::Trusted;
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
