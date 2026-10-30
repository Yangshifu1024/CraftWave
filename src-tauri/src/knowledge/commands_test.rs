//! knowledge/commands.rs 的 headless 测试（09 号票）：#48–51 四命令链路
//! （多来源合并 / 启停持久化 / 重扫感知新增 / 正文预览与错误码）+
//! ensure_skills_snapshot 的信任三路径（deny / allow_session / remember）与
//! Q21 整体版本重核、内置豁免、拒绝后 E_PROJECT_UNTRUSTED 面。
//!
//! 进程级注册表单例的互斥面：技能测试共享 [`REGISTRY_TEST_LOCK`]
//! （knowledge/mod.rs 的 cfg(test) 锁），避免并行测试互相改写 global。

use std::sync::Arc;

use super::commands::{
    SkillInspectArgs, SkillToggleArgs, ensure_skills_snapshot, skill_inspect_core, skill_list_core,
    skill_reload_core, skill_toggle_core,
};
use super::trust::ProjectTrustStore;
use crate::backend::ask::{AskAnswer, AskPayload};
use crate::host::AppState;
use crate::storage::SessionMeta;
use crate::util::events::{RecordingEventSink, keys};

/// headless AppState（临时数据目录 + 录音事件汇）：内置根指向一个空临时
/// 目录（隔离 dev 源码树 resources/skills 的真实内置技能，保证断言确定性）。
fn headless_app() -> (Arc<AppState>, tempfile::TempDir, Arc<RecordingEventSink>) {
    headless_app_with(None)
}

/// headless AppState 变体：可预置内置技能根（None = 空临时目录；resource_
/// skills_dir 在 Arc 包装前赋值——AppState 字段面非锁保护的，构造期注入）。
fn headless_app_with(
    builtin_root: Option<std::path::PathBuf>,
) -> (Arc<AppState>, tempfile::TempDir, Arc<RecordingEventSink>) {
    let dir = tempfile::tempdir().expect("临时目录");
    let events = Arc::new(RecordingEventSink::new());
    let mut state = AppState::new_in(dir.path().to_path_buf(), events.clone());
    state.resource_skills_dir = Some(builtin_root.unwrap_or_else(|| {
        // None = 指向一个空目录（隔离 dev 源码树 resources/skills 的真实
        // 内置技能，保证断言确定性）。
        let empty = tempfile::tempdir().expect("空内置根");
        let path = empty.path().to_path_buf();
        std::mem::forget(empty); // 临时目录保活到进程退出（测试量可忽略）
        path
    }));
    let app = Arc::new(state);
    (app, dir, events)
}

/// 写一个技能目录（frontmatter + 正文）。
fn write_skill(root: &std::path::Path, dir_name: &str, name: &str, description: &str, body: &str) {
    let dir = root.join(dir_name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}"),
    )
    .unwrap();
}

/// 造项目 + 会话并标记活跃（项目作用域锚定面）：返回项目主目录。
fn anchor_project_session(app: &AppState, dir: &tempfile::TempDir) -> std::path::PathBuf {
    let home = dir.path().join("proj-home");
    std::fs::create_dir_all(home.join(".craftwave")).unwrap();
    let meta = app
        .projects
        .save(None, "演示项目", home.to_str().unwrap())
        .expect("项目注册");
    app.store
        .index()
        .upsert(SessionMeta {
            id: "sess-1".to_owned(),
            project_id: Some(meta.id.clone()),
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
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some("sess-1".to_owned());
    home
}

/// 四命令链路：三来源合并可见、项目覆盖全局（验收第 1 条）、启停持久化
/// （验收第 4 条的后端半边）、reload 感知新增目录、inspect 正文预览与
/// E_NOT_FOUND。
#[test]
fn skill_commands_end_to_end() {
    let _guard = super::REGISTRY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempfile::tempdir().unwrap();
    // 三来源 fixture（不依赖 app）：内置 + 项目 + 全局（同名覆盖）
    let builtin_root = dir.path().join("builtin-skills");
    let home = dir.path().join("proj-home");
    std::fs::create_dir_all(home.join(".craftwave")).unwrap();
    write_skill(&builtin_root, "pack", "pack", "内置技能", "内置正文");
    std::fs::write(builtin_root.join("pack").join("extra.txt"), b"attached").unwrap();
    write_skill(
        &home.join(".craftwave/skills"),
        "demo",
        "demo",
        "全局版简介",
        "全局版正文",
    );
    write_skill(
        &home.join(".craftwave/skills"),
        "pack",
        "pack",
        "项目覆盖版",
        "项目覆盖版正文",
    );
    write_skill(
        &dir.path().join("skills"),
        "global-only",
        "global-only",
        "全局独有",
        "全局独有正文",
    );
    // AppState 构造期注入内置根，随后锚定项目会话
    let events = Arc::new(RecordingEventSink::new());
    let mut state = AppState::new_in(dir.path().to_path_buf(), events);
    state.resource_skills_dir = Some(builtin_root);
    let app = Arc::new(state);
    anchor_project_session(&app, &dir);

    // skill_list：同名覆盖后三技能可见（pack 内置被项目版覆盖）、来源正确、
    // 项目覆盖全局（验收第 1 条）
    let listed = skill_list_core(&app);
    assert_eq!(listed.len(), 3, "{listed:?}");
    let by_name = |n: &str| listed.iter().find(|e| e.name == n).unwrap();
    assert_eq!(
        by_name("pack").source,
        crate::knowledge::skills::SkillSource::Project
    );
    assert_eq!(by_name("pack").description, "项目覆盖版");
    assert_eq!(
        by_name("demo").source,
        crate::knowledge::skills::SkillSource::Project
    );
    assert_eq!(
        by_name("global-only").source,
        crate::knowledge::skills::SkillSource::Global
    );
    assert!(listed.iter().all(|e| e.enabled), "初始全启用");

    // skill_toggle：禁用 + 持久化（重启语义 = 新实例读同一 state 文件）
    skill_toggle_core(&app, "global-only", false).expect("禁用成功");
    let listed = skill_list_core(&app);
    assert!(
        !listed
            .iter()
            .find(|e| e.name == "global-only")
            .unwrap()
            .enabled
    );
    let state_bytes = std::fs::read(dir.path().join("skills-state.json")).expect("启停状态落盘");
    let state: crate::knowledge::skills::SkillsState =
        serde_json::from_slice(&state_bytes).unwrap();
    assert_eq!(state.disabled, vec!["global-only".to_owned()]);
    // 不存在的名字 → E_NOT_FOUND
    let err = skill_toggle_core(&app, "missing", true).expect_err("不存在");
    assert_eq!(err.code, "E_NOT_FOUND");
    let args = SkillToggleArgs {
        name: "missing".to_owned(),
        enabled: true,
    };
    let _ = args; // 契约形状编译期验证（反序列化面在命令宏内）

    // skill_reload：感知新增目录（FR 集成面）+ 禁用态保留
    write_skill(
        &home.join(".craftwave/skills"),
        "added",
        "added",
        "后来新增",
        "新增正文",
    );
    skill_reload_core(&app);
    let listed = skill_list_core(&app);
    assert!(
        listed.iter().any(|e| e.name == "added"),
        "reload 后新增可见"
    );
    assert!(
        !listed
            .iter()
            .find(|e| e.name == "global-only")
            .unwrap()
            .enabled,
        "reload 不丢禁用态"
    );

    // skill_inspect：正文全文（frontmatter 剥离）+ 不存在 E_NOT_FOUND
    let inspect = skill_inspect_core(&app, "demo").expect("预览");
    assert_eq!(inspect.markdown, "全局版正文");
    let err = skill_inspect_core(&app, "missing").expect_err("不存在");
    assert_eq!(err.code, "E_NOT_FOUND");
    let _ = SkillInspectArgs {
        name: "x".to_owned(),
    };
}

/// 后台应答者：等 projectTrust 询问出现后投递指定选项。同一测试内可能有多
/// 轮询问——从「最新」往回找尚未处理过的询问（旧 askId 的槽已关闭，应答投
/// 给它会丢失并挂死等待方）。
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

/// 信任三路径 + Q21 版本重核（验收第 5 条后端面）：
/// - deny → 快照 rejected 含该技能、disclosure 零注入（拒绝 → 不注入清单）；
/// - allow_session → 本会话内免问（第二次调用不弹询问）且 trust/project.json
///   不产生记录；
/// - remember → trust/project.json 落盘（跨 run 免问）；
/// - Q21：SKILL.md 或随附资源变化（指纹变）→ 记住过的技能重新询问。
#[tokio::test(flavor = "current_thread")]
#[allow(clippy::await_holding_lock)]
async fn trust_three_paths_and_version_recheck() {
    // 锁 guard 有意跨 await 持有（current_thread runtime 内串行化技能单例的
    // 并发测试改写；应答任务不经本锁，无死锁环）。
    let _guard = super::REGISTRY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (app, dir, events) = headless_app();
    let home = anchor_project_session(&app, &dir);
    write_skill(
        &home.join(".craftwave/skills"),
        "guard",
        "guard",
        "需信任技能",
        "受保护正文",
    );

    // —— deny：拒绝后 rejected + 零注入 ——
    let answer_app = app.clone();
    let answer_events = events.clone();
    tokio::spawn(async move {
        answer_project_trust(answer_app, answer_events, "deny").await;
    });
    let run = ensure_skills_snapshot(&app, "sess-1").await;
    assert!(run.snapshot.entries.is_empty(), "拒绝后不进清单");
    assert!(run.snapshot.is_rejected("guard"), "拒绝后进 rejected 面");
    assert!(run.disclosure.is_none(), "拒绝 → 不注入清单");
    // 询问载荷形状（§9.8 字段级）
    let opened = events
        .payloads_of(keys::ASK_OPENED)
        .into_iter()
        .find(|p| p["payload"]["kind"] == "projectTrust")
        .expect("deny 路径询问已发出");
    assert_eq!(opened["payload"]["resourceName"], "guard");
    assert_eq!(opened["payload"]["scope"], "project");
    assert_eq!(opened["payload"]["options"][2]["id"], "deny");

    // —— allow_session：本会话免问、不落盘 ——
    let answer_app = app.clone();
    let answer_events = events.clone();
    tokio::spawn(async move {
        answer_project_trust(answer_app, answer_events, "allow_session").await;
    });
    let run = ensure_skills_snapshot(&app, "sess-1").await;
    assert_eq!(run.snapshot.entries.len(), 1, "允许后进快照");
    assert_eq!(run.snapshot.entries[0].body, "受保护正文");
    assert!(
        run.disclosure
            .expect("注入文本")
            .contains("guard：需信任技能")
    );
    let trust = ProjectTrustStore::new(dir.path());
    assert!(trust.entries().is_empty(), "allow_session 不落盘");
    // 同会话第二次：免问（无新询问）直接快照
    let opened_before = events.payloads_of(keys::ASK_OPENED).len();
    let run = ensure_skills_snapshot(&app, "sess-1").await;
    assert_eq!(run.snapshot.entries.len(), 1);
    assert_eq!(
        events.payloads_of(keys::ASK_OPENED).len(),
        opened_before,
        "allow_session 同会话免问"
    );

    // —— remember：落盘免问（新会话也免） ——
    let answer_app = app.clone();
    let answer_events = events.clone();
    tokio::spawn(async move {
        answer_project_trust(answer_app, answer_events, "remember").await;
    });
    let run = ensure_skills_snapshot(&app, "sess-2").await;
    assert_eq!(run.snapshot.entries.len(), 1, "新会话经 remember 后可用");
    let trust = ProjectTrustStore::new(dir.path());
    assert_eq!(trust.entries().len(), 1);
    assert_eq!(trust.entries()[0].resource_name, "guard");
    assert_eq!(trust.entries()[0].decision, "remember");
    // 再次（新会话 sess-3）：记录命中零询问
    let opened_before = events.payloads_of(keys::ASK_OPENED).len();
    let run = ensure_skills_snapshot(&app, "sess-3").await;
    assert_eq!(run.snapshot.entries.len(), 1);
    assert_eq!(
        events.payloads_of(keys::ASK_OPENED).len(),
        opened_before,
        "remember 后跨会话免问"
    );

    // —— Q21：附属资源变化（指纹变）→ 重新询问 ——
    std::fs::write(
        home.join(".craftwave/skills/guard/attached.sh"),
        b"echo changed",
    )
    .unwrap();
    let answer_app = app.clone();
    let answer_events = events.clone();
    tokio::spawn(async move {
        answer_project_trust(answer_app, answer_events, "deny").await;
    });
    let run = ensure_skills_snapshot(&app, "sess-3").await;
    assert!(run.snapshot.entries.is_empty(), "版本变化后未重核 → 不可用");
    assert!(run.snapshot.is_rejected("guard"), "指纹变化重新走信任");
}

/// 内置技能豁免信任：resource_skills_dir 注入后无询问直接进快照；
/// 禁用的内置技能不进快照。
#[tokio::test(flavor = "current_thread")]
#[allow(clippy::await_holding_lock)]
async fn builtin_skills_skip_trust() {
    // 锁 guard 有意跨 await 持有（current_thread runtime 内串行化，见
    // trust_three_paths 测试内注释）。
    let _guard = super::REGISTRY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let builtin_root = tempfile::tempdir().unwrap();
    write_skill(
        builtin_root.path(),
        "hello",
        "hello",
        "内置示例",
        "内置正文",
    );
    write_skill(
        builtin_root.path(),
        "muted",
        "muted",
        "禁用的内置",
        "不该出现",
    );
    let (app, dir, events) = headless_app_with(Some(builtin_root.path().to_path_buf()));
    let _ = anchor_project_session(&app, &dir);
    skill_toggle_core(&app, "muted", false).expect("禁用内置");

    let run = ensure_skills_snapshot(&app, "sess-1").await;
    assert_eq!(run.snapshot.entries.len(), 1);
    assert_eq!(run.snapshot.entries[0].name, "hello");
    assert_eq!(run.snapshot.entries[0].body, "内置正文");
    assert!(
        run.disclosure
            .expect("内置技能也注入")
            .contains("hello：内置示例")
    );
    assert!(
        !events
            .payloads_of(keys::ASK_OPENED)
            .iter()
            .any(|p| p["payload"]["kind"] == "projectTrust"),
        "内置技能豁免信任"
    );
}

/// 未信任技能的 skill 工具调用按 E_PROJECT_UNTRUSTED 拒绝（验收第 5 条
/// 「拒绝后 E_PROJECT_UNTRUSTED」）：直接驱动执行器 skill 分支。
#[tokio::test(flavor = "current_thread")]
async fn skill_call_rejected_when_untrusted() {
    use crate::backend::exec::{ExecutorEnv, ToolsetExecutor};
    use crate::backend::run_loop::CancellableExecutor;
    use crate::tools::ToolsContext;
    use std::sync::Mutex;

    let (app, dir, _events) = headless_app();
    let snapshot = Arc::new(crate::knowledge::skills::snapshot_for_run(
        Vec::new(),
        vec!["guard".to_owned()],
    ));
    let ctx = ToolsContext::new(dir.path().to_path_buf(), dir.path().to_path_buf());
    let executor = ToolsetExecutor::new(ctx, crate::safety::tier::PermissionTier::AutoEdit)
        .with_env(ExecutorEnv {
            events: app.events.clone(),
            ask: Arc::new(Mutex::new(None)),
            plan: Arc::new(Mutex::new(None)),
            session_id: "sess-1".to_owned(),
            run_id: "run-1".to_owned(),
            data_root: dir.path().to_path_buf(),
            mcp: None,
            skills: Some(snapshot),
            subagents: None,
            allowed_tools: None,
            network: None,
            store: None,
        });
    let token = tokio_util::sync::CancellationToken::new();
    let exec = executor
        .execute("call-1", "skill", r#"{"name":"guard"}"#, &token)
        .await;
    let crate::backend::run_loop::ToolExecution::Completed {
        ok,
        error,
        model_text,
        ..
    } = exec
    else {
        panic!("skill 调用应为 Completed");
    };
    assert!(!ok);
    assert_eq!(error.expect("错误面").code, "E_PROJECT_UNTRUSTED");
    assert!(model_text.contains("E_PROJECT_UNTRUSTED"));

    // 未知技能 → E_INVALID_ARG（completed_failure 的码面）+ 可用清单提示
    let exec = executor
        .execute("call-2", "skill", r#"{"name":"missing"}"#, &token)
        .await;
    let crate::backend::run_loop::ToolExecution::Completed {
        ok,
        error,
        model_text,
        ..
    } = exec
    else {
        panic!("skill 调用应为 Completed");
    };
    assert!(!ok);
    assert_eq!(error.expect("错误面").code, "E_INVALID_ARG");
    assert!(model_text.contains("未知技能"));
}

/// 已信任技能经执行器加载：正文全文 + 头部标注回喂 + 边车落盘（验收
/// 第 2 条「skill 工具调用按需加载全文」的执行链半边）。
#[tokio::test(flavor = "current_thread")]
async fn skill_call_returns_body_from_snapshot() {
    use crate::backend::exec::{ExecutorEnv, ToolsetExecutor};
    use crate::backend::run_loop::CancellableExecutor;
    use crate::tools::ToolsContext;
    use std::sync::Mutex;

    let (app, dir, _events) = headless_app();
    let snapshot = Arc::new(crate::knowledge::skills::snapshot_for_run(
        vec![crate::knowledge::skills::SkillSnapshotEntry {
            name: "demo".to_owned(),
            description: "简介".to_owned(),
            body: "正文全文 BODY".to_owned(),
            fingerprint: "fp".to_owned(),
            untrusted: false,
        }],
        Vec::new(),
    ));
    let ctx = ToolsContext::new(dir.path().to_path_buf(), dir.path().to_path_buf());
    let executor = ToolsetExecutor::new(ctx, crate::safety::tier::PermissionTier::AutoEdit)
        .with_env(ExecutorEnv {
            events: app.events.clone(),
            ask: Arc::new(Mutex::new(None)),
            plan: Arc::new(Mutex::new(None)),
            session_id: "sess-1".to_owned(),
            run_id: "run-1".to_owned(),
            data_root: dir.path().to_path_buf(),
            mcp: None,
            skills: Some(snapshot),
            subagents: None,
            allowed_tools: None,
            network: None,
            store: None,
        });
    let token = tokio_util::sync::CancellationToken::new();
    let exec = executor
        .execute("call-1", "skill", r#"{"name":"demo"}"#, &token)
        .await;
    let crate::backend::run_loop::ToolExecution::Completed {
        ok,
        ui_summary,
        error,
        model_text,
    } = exec
    else {
        panic!("skill 调用应为 Completed");
    };
    assert!(ok);
    assert!(error.is_none());
    assert_eq!(ui_summary, "已加载技能 demo");
    assert!(model_text.starts_with("以下是技能 demo 的正文："));
    assert!(model_text.contains("正文全文 BODY"));
    // 边车：正文进 data 供卡片恢复（FR-04-07 面）
    let outcomes =
        crate::storage::toolres::load_outcomes(dir.path(), "sess-1", &["call-1".to_owned()]);
    let payload = outcomes.get("call-1").expect("skill 结果应写边车");
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["data"]["name"], "demo");
    assert_eq!(payload["data"]["body"], "正文全文 BODY");

    // args 透传面：SkillToolArgs 的可选 args 字段（§8.3 参数摘要）不阻塞加载
    let exec = executor
        .execute(
            "call-2",
            "skill",
            r#"{"name":"demo","args":"附加参数"}"#,
            &token,
        )
        .await;
    let crate::backend::run_loop::ToolExecution::Completed { ok, .. } = exec else {
        panic!("skill 调用应为 Completed");
    };
    assert!(ok);

    // 坏参数 → 工具级失败回喂
    let exec = executor
        .execute("call-3", "skill", "not-json", &token)
        .await;
    let crate::backend::run_loop::ToolExecution::Completed { ok, .. } = exec else {
        panic!("skill 调用应为 Completed");
    };
    assert!(!ok);
}

/// ProjectTrust 载荷形状编译期验证（AskPayload 新变体的序列化面）。
#[test]
fn project_trust_payload_shape() {
    let payload = AskPayload::ProjectTrust {
        resource_name: "guard".to_owned(),
        scope: crate::backend::ask::SkillTrustScope::Project,
        fingerprint: "abcdef1234567890".to_owned(),
        options: Vec::new(),
    };
    let json = serde_json::to_value(&payload).unwrap();
    assert_eq!(json["kind"], "projectTrust");
    assert_eq!(json["resourceName"], "guard");
    assert_eq!(json["scope"], "project");
    let round: AskPayload = serde_json::from_value(json).unwrap();
    assert_eq!(round, payload);
}
