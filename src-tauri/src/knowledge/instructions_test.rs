//! knowledge/instructions.rs 的 headless 测试（挂接点①）：
//! 项目指令（AGENTS.md/CRAFTWAVE.md/CLAUDE.md）首用信任三路径（allow_session
//! / remember / deny）、内容变更重问、deny 后不注入、无项目上下文零注入。

use std::sync::Arc;

use super::instructions::ensure_instruction_disclosure;
use super::trust::ProjectTrustStore;
use crate::backend::ask::AskAnswer;
use crate::host::AppState;
use crate::storage::SessionMeta;
use crate::util::events::{RecordingEventSink, keys};

/// headless AppState（临时数据目录 + 录音事件汇）。
fn headless_app() -> (Arc<AppState>, tempfile::TempDir, Arc<RecordingEventSink>) {
    let dir = tempfile::tempdir().expect("临时目录");
    let events = Arc::new(RecordingEventSink::new());
    let state = AppState::new_in(dir.path().to_path_buf(), events.clone());
    (Arc::new(state), dir, events)
}

/// 造项目（每测试一次；DuplicateHome 防重复注册）：返回项目主目录。
fn ensure_project(app: &AppState, dir: &tempfile::TempDir) -> std::path::PathBuf {
    let home = dir.path().join("proj-home");
    std::fs::create_dir_all(&home).unwrap();
    app.projects
        .save(None, "演示项目", home.to_str().unwrap())
        .expect("项目注册");
    home
}

/// 造会话并标记活跃（同一项目可挂多个会话）。
fn anchor_session(app: &AppState, dir: &tempfile::TempDir, session_id: &str) {
    let home = dir.path().join("proj-home");
    let project = app
        .projects
        .list()
        .expect("项目清单")
        .into_iter()
        .find(|p| p.home_dir == home.to_string_lossy())
        .expect("项目已注册（先 ensure_project）");
    app.store
        .index()
        .upsert(SessionMeta {
            id: session_id.to_owned(),
            project_id: Some(project.id.clone()),
            title: "测试会话".to_owned(),
            created_at: 1,
            updated_at: 1,
            revision: 1,
            message_count: 0,
            total_tokens: 0,
            authorized_external_dirs: Vec::new(),
        })
        .unwrap();
    *app.active_session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(session_id.to_owned());
}

/// 后台应答者：等 projectTrust 询问出现后投递指定选项（从最新往回找尚未
/// 处理过的询问——同一测试内可能有多轮询问）。
async fn answer_project_trust(
    app: Arc<AppState>,
    events: Arc<RecordingEventSink>,
    option_id: &str,
) {
    let mut handled: Vec<String> = Vec::new();
    for _ in 0..500 {
        let opened = events
            .payloads_of(keys::ASK_OPENED)
            .into_iter()
            .rev()
            .find(|p| {
                p["payload"]["kind"] == "projectTrust"
                    && p["payload"]["resourceName"] == "AGENTS.md"
                    && !handled.contains(&p["askId"].as_str().unwrap_or_default().to_owned())
            });
        if let Some(payload) = opened {
            let ask_id = payload["askId"].as_str().expect("askId").to_owned();
            handled.push(ask_id.clone());
            app.resolve_app_ask(
                &ask_id,
                AskAnswer::Approval {
                    option_id: option_id.to_owned(),
                    edited_command: None,
                },
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("projectTrust 询问未出现");
}

// 断言（信任三路径 + 内容变更重问，验收第 1/2 条后端面 + 红线「指纹变更
// 重问」）：deny → 注入 None + 不产生记录（下一 run 重新询问），询问载荷
// 字段级（resourceName/scope/fingerprint/options）；allow_session →
// 注入 + 会话内存免问（trust 文件无记录，新会话重问）；remember → 注入 +
// trust/project.json 落盘（新会话也免问）；内容变更（指纹不符）→ 重新
// 询问且变更后 deny 不注入（不能沿用旧信任读新正文， 同精神）；
// 无项目上下文（临时会话）零注入零询问。
#[tokio::test(flavor = "current_thread")]
async fn instruction_trust_change_reask_and_scope() {
    let (app, dir, events) = headless_app();
    let home = ensure_project(&app, &dir);
    anchor_session(&app, &dir, "sess-1");
    std::fs::write(home.join("AGENTS.md"), "# 仓库指令\n使用 pnpm。").unwrap();

    // —— deny：不注入、不落盘、下一 run 重新询问 ——
    {
        let answer_app = app.clone();
        let answer_events = events.clone();
        tokio::spawn(async move {
            answer_project_trust(answer_app, answer_events, "deny").await;
        });
        let disclosure = ensure_instruction_disclosure(&app, "sess-1").await;
        assert!(disclosure.is_none(), "拒绝后不注入");
    }
    let trust = ProjectTrustStore::new(dir.path());
    assert!(trust.entries().is_empty(), "deny 不产生记录");
    // 询问载荷形状（字段级：resourceName/scope/fingerprint/options）
    let opened = events
        .payloads_of(keys::ASK_OPENED)
        .into_iter()
        .find(|p| p["payload"]["kind"] == "projectTrust")
        .expect("deny 路径询问已发出");
    assert_eq!(opened["payload"]["resourceName"], "AGENTS.md");
    assert_eq!(opened["payload"]["scope"], "project");
    assert_eq!(opened["payload"]["options"][2]["id"], "deny");

    // —— allow_session：本会话免问、不落盘 ——
    {
        let answer_app = app.clone();
        let answer_events = events.clone();
        tokio::spawn(async move {
            answer_project_trust(answer_app, answer_events, "allow_session").await;
        });
        let disclosure = ensure_instruction_disclosure(&app, "sess-1")
            .await
            .expect("允许后注入");
        assert!(disclosure.contains("AGENTS.md"), "注入带文件名标注");
        assert!(disclosure.contains("使用 pnpm。"), "注入含正文全文");
    }
    assert!(
        ProjectTrustStore::new(dir.path()).entries().is_empty(),
        "allow_session 不落盘（会话内存面）"
    );
    // 同会话第二次：免问（无新询问）直接注入
    let opened_before = events.payloads_of(keys::ASK_OPENED).len();
    let disclosure = ensure_instruction_disclosure(&app, "sess-1")
        .await
        .expect("同会话免问");
    assert_eq!(
        events.payloads_of(keys::ASK_OPENED).len(),
        opened_before,
        "allow_session 同会话免问"
    );
    assert!(disclosure.contains("AGENTS.md"));
    // 新会话：会话标记不延续 → 重新询问
    anchor_session(&app, &dir, "sess-2");
    {
        let answer_app = app.clone();
        let answer_events = events.clone();
        tokio::spawn(async move {
            answer_project_trust(answer_app, answer_events, "deny").await;
        });
        assert!(
            ensure_instruction_disclosure(&app, "sess-2")
                .await
                .is_none(),
            "新会话重问（deny 后不注入）"
        );
    }

    // —— remember：落盘免问（新会话也免）——
    {
        let answer_app = app.clone();
        let answer_events = events.clone();
        tokio::spawn(async move {
            answer_project_trust(answer_app, answer_events, "remember").await;
        });
        assert!(
            ensure_instruction_disclosure(&app, "sess-2")
                .await
                .is_some(),
            "remember 后注入"
        );
    }
    let trust = ProjectTrustStore::new(dir.path());
    assert_eq!(trust.entries().len(), 1);
    assert_eq!(trust.entries()[0].resource_name, "AGENTS.md");
    assert_eq!(trust.entries()[0].decision, "remember");
    // 再次（新会话 sess-3）：记录命中零询问
    anchor_session(&app, &dir, "sess-3");
    let opened_before = events.payloads_of(keys::ASK_OPENED).len();
    assert!(
        ensure_instruction_disclosure(&app, "sess-3")
            .await
            .is_some()
    );
    assert_eq!(
        events.payloads_of(keys::ASK_OPENED).len(),
        opened_before,
        "remember 后跨会话免问"
    );

    // —— 内容变更（指纹不符）→ 重新询问（「内容变更 → 重新询问」）；
    // 变更后 deny → 不注入（不能沿用旧信任读新正文， 同精神）。
    std::fs::write(home.join("AGENTS.md"), "# v2 恶意指令").unwrap();
    {
        let answer_app = app.clone();
        let answer_events = events.clone();
        tokio::spawn(async move {
            answer_project_trust(answer_app, answer_events, "deny").await;
        });
        assert!(
            ensure_instruction_disclosure(&app, "sess-1")
                .await
                .is_none(),
            "变更后未重核 → 不注入"
        );
    }
    let opened = events.payloads_of(keys::ASK_OPENED);
    assert!(opened.len() >= 2, "变更触发第二次询问");

    // —— 无项目上下文（临时会话）：零注入零询问（项目指令是项目作用域
    // 资源）。
    let (app, _dir, events) = headless_app();
    assert!(
        ensure_instruction_disclosure(&app, "sess-t")
            .await
            .is_none()
    );
    assert!(events.payloads_of(keys::ASK_OPENED).is_empty(), "零询问");
}
