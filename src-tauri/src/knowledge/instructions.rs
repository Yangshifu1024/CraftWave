//! 项目指令的信任核验与系统提示注入（挂接点①；实装）。
//!
//! 项目指令（project instructions）= 项目主目录根下的 `AGENTS.md`、
//! `CRAFTWAVE.md`、`CLAUDE.md`——给 AI 看的仓库级操作指引，属于
//! 定义的「项目 `.craftwave/` 资源」信任面（第三方仓库可借它
//! 指使 AI，首次加载必须让用户知情决策，TOFU）。
//!
//! run 开始装配时逐文件处理（存在才处理，清单序固定）：
//! 1. 读文件字节 → 指纹 = 内容 SHA-256（「指纹=资源内容 SHA-256」）；
//! 2. 信任核验：统一存储「记住」记录（scope=project，resourceName=文件名）
//!    或会话级「本次会话允许」标记 → 已信任；
//! 3. 未信任 → 同步弹 `projectTrust` 询问（挂应用级询问桶，用户主动发
//!    消息触发所以等得起）：allow_session → 会话内存标记（同会话免问，
//!    重启失效）；remember → 写统一存储（跨重启免问）；deny → 本 run 不
//!    注入且不产生记录（下一 run 重新询问）；
//! 4. 已信任文件读全文进注入文本（拒绝/不存在的文件零注入——「未信任
//!    资源不注入系统提示」）。
//!
//! worktree 信任：resourceName 取文件名（不含路径），worktree 归属
//! 原项目时同名同内容 → 同指纹命中既有信任，不因副本路径重复询问；内容
//! 变更指纹变 → 重新询问；跨项目不同内容自然不命中。任务内版本固定
//! ：正文在 run 装配时读定，run 内不热换；撤销（revoke 记录）即时
//! 生效于下一 run 的信任核验。

use sha2::{Digest, Sha256};

use super::commands::{ensure_project_trust_ask, project_trust_options};
use super::session_trust::SessionTrustSet;
use super::trust::ProjectTrustStore;
use crate::backend::ask::{AskPayload, SkillTrustScope};
use crate::host::AppState;

/// 项目指令文件清单（信任对象；清单序 = 注入序）。
pub const INSTRUCTION_FILES: &[&str] = &["AGENTS.md", "CRAFTWAVE.md", "CLAUDE.md"];

/// 指纹展示长度（projectTrust 载荷与其他信任面同款前 16 hex）。
const FINGERPRINT_DISPLAY_LEN: usize = 16;

/// 文件内容 → SHA-256 完整 hex（项目资源指纹口径）。
fn content_fingerprint(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// 字节 → 小写 hex。
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// run 开始时的项目指令信任核验与注入文本（start_chat 消费；无项目上下文
/// 或零个已信任文件 → None = 零注入）。
pub async fn ensure_instruction_disclosure(app: &AppState, session_id: &str) -> Option<String> {
    let (_, project_home) = crate::mcp::commands::project_context_of(app)?;
    let trust_store = ProjectTrustStore::new(app.store.root());
    let mut sections: Vec<String> = Vec::new();
    for name in INSTRUCTION_FILES {
        let path = project_home.join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            continue; // 文件不存在：非本项目约定，静默跳过
        };
        let fingerprint = content_fingerprint(&bytes);
        if !ensure_trusted(app, session_id, &trust_store, name, &fingerprint).await {
            continue; // deny/应答异常：本 run 不注入（未信任不注入）
        }
        // 已信任：读全文进注入文本（解码失败按不可得跳过并提示——
        // 「旧版本不可得时停止相关加载」）。
        match String::from_utf8(bytes) {
            Ok(text) => sections.push(format!(
                "以下是项目指令文件 {name} 的内容（仓库约定的 AI 操作指引）：\n\n{}",
                text.trim_end()
            )),
            Err(_) => {
                eprintln!("[craftwave] 项目指令 {name} 不是合法 UTF-8 文本，本次运行不注入");
            }
        }
    }
    if sections.is_empty() {
        None
    } else {
        Some(sections.join("\n\n"))
    }
}

/// 单个指令文件的信任核验（含询问交互）：持久记录 / 会话标记 / 同步询问
/// 三级；返回 true = 本 run 可注入。
async fn ensure_trusted(
    app: &AppState,
    session_id: &str,
    trust_store: &ProjectTrustStore,
    name: &str,
    fingerprint: &str,
) -> bool {
    if trust_store.is_trusted(name, fingerprint)
        || app
            .session_trust
            .contains(session_id, "project", name, fingerprint)
    {
        return true;
    }
    let payload = AskPayload::ProjectTrust {
        resource_name: name.to_owned(),
        scope: SkillTrustScope::Project,
        fingerprint: fingerprint.chars().take(FINGERPRINT_DISPLAY_LEN).collect(),
        options: project_trust_options(),
    };
    match ensure_project_trust_ask(app, payload).await.as_str() {
        "allow_session" => {
            mark_session(app, session_id, name, fingerprint);
            true
        }
        "remember" => {
            let _ = trust_store.remember(name, fingerprint, crate::storage::trust::now_ms());
            true
        }
        _ => false,
    }
}

/// 会话级「本次会话允许」登记（AppState 共享集合）。
fn mark_session(app: &AppState, session_id: &str, name: &str, fingerprint: &str) {
    let set: &SessionTrustSet = &app.session_trust;
    set.mark(session_id, "project", name, fingerprint);
}
