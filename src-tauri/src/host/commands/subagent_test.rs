//! 子代理委派集成测试（10 号票，§12.5 点名场景）：本地 OpenAI 兼容 mock
//! SSE 服务驱动 `run_subagent` 全链——
//! - 四象限各取代表（report / partial(E_SUB_BUDGET) / cancelled）；
//! - 帧序不变量（subStarted 先于全部子帧、subDone 必收尾）；
//! - 事件六键、用量归属（tokens:update 单一出口）、子代理历史落盘与回看；
//! - 取消级联（stop_subagent → 子 runEnded(cancelled) 收齐）；
//! - 公共操作/角色边界（子环境拒绝再委派、白名单外工具拦截、非 Git 项目
//!   写入子代理拒绝）与请求面两形态（cleanContext / 父历史注入）。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;

// ---- 本地 OpenAI 兼容 mock SSE 服务（tests/common/mock_openai.rs 的精简单测版） ----

/// mock 行为开关：tool_calls=true 时每轮都回工具调用（预算耗尽场景）；
/// delay_ms 每轮响应前延迟（取消场景）。
#[derive(Default)]
struct MockMode {
    tool_calls: AtomicBool,
    delay_ms: AtomicUsize,
}

/// chunk 构造（OpenAI chat.completions 流式格式）。
fn chunk(delta: serde_json::Value, finish: Option<&str>) -> String {
    format!(
        "data: {}\n\n",
        serde_json::json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "gpt-4o-mock",
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish.map(serde_json::Value::from).unwrap_or(serde_json::Value::Null)}]
        })
    )
}

/// 启动 mock（随机端口），返回 base_url（含 /v1）。
async fn spawn_mock(mode: Arc<MockMode>) -> String {
    async fn handler(mode: Arc<MockMode>, _body: serde_json::Value) -> axum::response::Response {
        let delay = mode.delay_ms.load(Ordering::SeqCst) as u64;
        let script: Vec<String> = if mode.tool_calls.load(Ordering::SeqCst) {
            // 每轮：工具调用（read）→ 预算耗尽场景（max_turns 顶格触发 rig MaxTurns）
            vec![
                chunk(
                    serde_json::json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": "read", "arguments": ""}}]}),
                    None,
                ),
                chunk(
                    serde_json::json!({"tool_calls": [{"index": 0, "function": {"arguments": "{\"path\":\"README.md\"}"}}]}),
                    None,
                ),
                chunk(serde_json::json!({}), Some("tool_calls")),
                format!(
                    "data: {}\n\n",
                    serde_json::json!({
                        "id": "chatcmpl-mock", "object": "chat.completion.chunk", "created": 1,
                        "model": "gpt-4o-mock", "choices": [],
                        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
                    })
                ),
                "data: [DONE]\n\n".to_string(),
            ]
        } else if delay > 0 {
            // 慢响应（取消场景）：先发一段正文再挂起
            vec![
                chunk(serde_json::json!({"content": "开工"}), None),
                format!(
                    "data: {}\n\n",
                    serde_json::json!({
                        "id": "chatcmpl-mock", "object": "chat.completion.chunk", "created": 1,
                        "model": "gpt-4o-mock", "choices": [],
                        "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5}
                    })
                ),
                "data: [DONE]\n\n".to_string(),
            ]
        } else {
            // 纯文本汇报（report 场景）：`<report>` 语义由角色提示词引导，
            // mock 直接给最终文本（最后 assistant 文本即汇报）
            vec![
                chunk(serde_json::json!({"content": "调研完成："}), None),
                chunk(serde_json::json!({"content": "README 共一行。"}), None),
                chunk(serde_json::json!({}), Some("stop")),
                format!(
                    "data: {}\n\n",
                    serde_json::json!({
                        "id": "chatcmpl-mock", "object": "chat.completion.chunk", "created": 1,
                        "model": "gpt-4o-mock", "choices": [],
                        "usage": {"prompt_tokens": 7, "completion_tokens": 4, "total_tokens": 11}
                    })
                ),
                "data: [DONE]\n\n".to_string(),
            ]
        };
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        let stream =
            futures::stream::iter(script.into_iter().map(Ok::<_, std::convert::Infallible>));
        axum::response::Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(axum::body::Body::from_stream(stream))
            .unwrap()
    }
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let mode = mode.clone();
            async move { handler(mode, body).await }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定随机端口");
    let addr = listener.local_addr().expect("本地地址");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/v1")
}

// ---- 测试装配面 ----

/// 帧收集器（可共享读取）。
#[derive(Default, Clone)]
struct SharedFrames(Arc<Mutex<Vec<crate::backend::frames::SequencedFrame>>>);

impl crate::backend::frames::SequencedSink for SharedFrames {
    fn send_seq(&mut self, frame: crate::backend::frames::SequencedFrame) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(frame);
    }
}

impl SharedFrames {
    fn snapshot(&self) -> Vec<crate::backend::frames::SequencedFrame> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn frame_types(&self) -> Vec<String> {
        self.snapshot()
            .iter()
            .map(|f| frame_type(&f.frame))
            .collect()
    }
}

/// 帧的 type 键（断言面）。
fn frame_type(frame: &ChatFrame) -> String {
    serde_json::to_value(frame)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_owned))
        .unwrap_or_default()
}

/// headless AppState + mock 供应商配置。
fn app_with_mock(
    base_url: &str,
) -> (
    Arc<AppState>,
    tempfile::TempDir,
    Arc<crate::util::events::RecordingEventSink>,
) {
    let dir = tempfile::tempdir().expect("临时数据目录");
    let events = Arc::new(crate::util::events::RecordingEventSink::new());
    let mut state = AppState::new_in(dir.path().to_path_buf(), events.clone());
    state.resource_skills_dir = None;
    let app = Arc::new(state);
    let config = crate::providers::types::AppConfig {
        providers: vec![crate::providers::types::ProviderConfig {
            id: "prov-1".to_owned(),
            name: "Mock".to_owned(),
            api_format: crate::providers::types::ApiFormat::OpenaiChat,
            base_url: Some(base_url.to_owned()),
            api_key: Some("mock-key".to_owned()),
            models: vec![crate::providers::types::ProviderModel {
                id: "model-1".to_owned(),
                model: "gpt-4o-mock".to_owned(),
                max_tokens: 32768,
                context_window: 128000,
                reasoning_effort: None,
                vision: None,
            }],
            headers: None,
        }],
        active_model_id: Some("model-1".to_owned()),
        ..crate::providers::types::AppConfig::default()
    };
    crate::providers::service::save_config(&app.config, config).expect("保存配置");
    (app, dir, events)
}

/// 委派上下文（父枢纽接共享帧收集器）。
fn ctx_with_frames(session_id: &str, parent_run_id: &str) -> (SubagentContext, SharedFrames) {
    let frames = SharedFrames::default();
    let hub = Arc::new(crate::agent::FrameHub::new(Box::new(frames.clone())));
    (
        SubagentContext {
            session_id: session_id.to_owned(),
            parent_run_id: parent_run_id.to_owned(),
            parent_hub: hub,
            tier: crate::safety::tier::PermissionTier::AutoEdit,
            model_id: Some("model-1".to_owned()),
            auto_confirm: false,
        },
        frames,
    )
}

/// 等待某事件键出现（异步产事件的轮询面；async 轮询让出执行权——
/// current_thread runtime 下委派任务靠同一线程推进）。
async fn wait_event(events: &crate::util::events::RecordingEventSink, key: &str, timeout_ms: u64) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    while tokio::time::Instant::now() < deadline {
        if !events.payloads_of(key).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("等待事件 {key} 超时");
}

// ---- 全链场景 ----

/// explore 委派端到端（report 象限）：subStarted 先于子帧、子 runEnded(done)
/// 先于 subReport/subDone、<report> 回父工具结果、事件六键到位、历史可回看。
#[tokio::test]
async fn subagent_full_chain_report() {
    let base_url = spawn_mock(Arc::new(MockMode::default())).await;
    let (app, dir, events) = app_with_mock(&base_url);
    let (ctx, frames) = ctx_with_frames("sess-r", "run-parent");
    let spawner = SubagentSpawnerImpl {
        app: (*app).share_handles(),
        ctx: ctx.clone(),
    };
    let parent_token = tokio_util::sync::CancellationToken::new();
    let result = spawner
        .spawn(
            r#"{"role":"explore","task":"调研 README 结构","budgetSteps":40}"#,
            &parent_token,
        )
        .await;

    // 工具结果：<report> 结构化汇报回父
    let crate::backend::run_loop::ToolExecution::Completed {
        ok,
        ui_summary,
        model_text,
        ..
    } = result
    else {
        panic!("委派应完成，实际 {result:?}")
    };
    assert!(ok);
    assert!(ui_summary.contains("explore"));
    assert!(
        model_text.contains("<report>") && model_text.contains("调研完成"),
        "汇报全文回父：{model_text}"
    );

    // 帧序不变量：subStarted 最先、subDone 收尾、中间是 subFrame（含子
    // runStarted 与子 runEnded(done)）、subReport 在 subDone 之前
    let types = frames.frame_types();
    assert_eq!(types.first().unwrap(), "subStarted", "帧序：{types:?}");
    assert_eq!(
        types.last().unwrap(),
        "subDone",
        "subDone 必收尾：{types:?}"
    );
    assert_eq!(
        types.iter().filter(|t| *t == "subStarted").count(),
        1,
        "subStarted 仅一次"
    );
    let sub_started_idx = types.iter().position(|t| t == "subStarted").unwrap();
    let sub_done_idx = types.iter().position(|t| t == "subDone").unwrap();
    let run_ended_idx = types
        .iter()
        .position(|t| t == "subFrame")
        .map(|_| {
            types[sub_started_idx..]
                .iter()
                .rposition(|t| t == "subFrame")
                .map(|off| sub_started_idx + off)
                .unwrap()
        })
        .unwrap();
    assert!(run_ended_idx < sub_done_idx, "子 runEnded 在 subDone 之前");
    // 之间只允许子帧透传（subFrame）与汇报帧（subReport）
    for t in &types[sub_started_idx + 1..sub_done_idx] {
        assert!(
            t == "subFrame" || t == "subReport",
            "subStarted 与 subDone 之间只能是子帧/汇报帧：{types:?}"
        );
    }
    // subReport 帧在 subDone 前
    assert!(
        types.contains(&"subReport".to_owned())
            && types.iter().position(|t| t == "subReport").unwrap() < sub_done_idx
    );

    // 事件六键（sub:error 在 report 场景不发）
    wait_event(&events, keys::SUB_SPAWN, 2000).await;
    wait_event(&events, keys::SUB_STEP, 2000).await;
    wait_event(&events, keys::SUB_USAGE, 2000).await;
    wait_event(&events, keys::SUB_REPORT, 2000).await;
    wait_event(&events, keys::SUB_DONE, 2000).await;
    assert!(
        events.payloads_of(keys::SUB_ERROR).is_empty(),
        "report 场景无 error 事件"
    );
    let spawn_payload = &events.payloads_of(keys::SUB_SPAWN)[0];
    assert_eq!(spawn_payload["role"], "explore");
    assert_eq!(spawn_payload["sessionId"], "sess-r");
    assert_eq!(spawn_payload["runId"], "run-parent");
    let done_payload = &events.payloads_of(keys::SUB_DONE)[0];
    assert_eq!(done_payload["outcome"], "report");

    // 用量归属（Q40）：会话累计已计入（tokens:update），只计一次
    let tokens = &events.payloads_of(keys::TOKENS_UPDATE);
    assert!(!tokens.is_empty(), "子代理用量计入会话累计");
    assert!(tokens[0]["usage"]["input"].as_u64().unwrap() > 0);
    // 子 run 不占主 run 注册表（主任务次数仅主 run，Q40）
    assert!(app.runs.get("sess-r").is_none());

    // 历史（FR-07-06）：meta + JSONL 落盘，load_subagent_history 可回看
    let sub_id = spawn_payload["subId"].as_str().unwrap().to_owned();
    let messages = load_subagent_history_core(&app, "sess-r", &sub_id).expect("回看子代理历史");
    assert!(
        messages.len() >= 2,
        "至少一条 assistant 汇报 + 一条 report 条目"
    );
    let report_entry = messages
        .iter()
        .find(|m| m.id.ends_with(":report"))
        .expect("含 report 条目");
    let report_text = report_entry
        .parts
        .iter()
        .filter_map(|p| match p {
            crate::storage::WirePart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert!(report_text.contains("调研完成"));
    // 未知 subId → E_NOT_FOUND
    let err = load_subagent_history_core(&app, "sess-r", "nope").unwrap_err();
    assert_eq!(err.code, crate::util::errors::E_NOT_FOUND);
    drop(dir);
}

/// 预算耗尽（partial 象限）：max_turns 顶格 → rig MaxTurns →
/// E_SUB_BUDGET 工具结果 + subDone(partial) + 事件带 outcome=partial。
#[tokio::test]
async fn subagent_budget_exhausted_partial() {
    let mode = Arc::new(MockMode::default());
    mode.tool_calls.store(true, Ordering::SeqCst);
    let base_url = spawn_mock(mode).await;
    let (app, dir, events) = app_with_mock(&base_url);
    let (ctx, frames) = ctx_with_frames("sess-b", "run-b");
    let spawner = SubagentSpawnerImpl {
        app: (*app).share_handles(),
        ctx,
    };
    let parent_token = tokio_util::sync::CancellationToken::new();
    let result = spawner
        .spawn(
            r#"{"role":"explore","task":"反复调研","budgetSteps":2}"#,
            &parent_token,
        )
        .await;

    let crate::backend::run_loop::ToolExecution::Completed {
        ok,
        error,
        model_text,
        ..
    } = result
    else {
        panic!("预算耗尽也是收齐的工具结果")
    };
    assert!(!ok);
    let error = error.expect("partial 带 E_SUB_BUDGET 错误面");
    assert_eq!(error.code, crate::util::errors::E_SUB_BUDGET);
    assert!(
        model_text.contains(crate::util::errors::E_SUB_BUDGET) && model_text.contains("再次委派"),
        "模型通道说明预算耗尽与续派指引：{model_text}"
    );
    let types = frames.frame_types();
    assert_eq!(
        types.last().unwrap(),
        "subDone",
        "partial 也以 subDone 收尾"
    );
    assert!(
        events
            .payloads_of(keys::SUB_DONE)
            .iter()
            .any(|p| p["outcome"] == "partial"),
        "事件带 outcome=partial"
    );
    assert!(
        events.payloads_of(keys::SUB_ERROR).is_empty(),
        "预算耗尽不是 error 事件（outcome 已表达）"
    );
    // 注册表摘除 + 名额释放（活跃数归零）
    assert_eq!(app.subs.active_count(), 0);
    drop(dir);
}

/// stop_subagent 级联（cancelled 象限）：慢响应期间点名取消 →
/// 子 runEnded(cancelled) 帧收齐 + subDone(cancelled) + 注册表摘除。
#[tokio::test]
async fn subagent_stop_cascades_and_settles_clean() {
    let mode = Arc::new(MockMode::default());
    mode.delay_ms.store(10_000, Ordering::SeqCst);
    let base_url = spawn_mock(mode).await;
    let (app, dir, events) = app_with_mock(&base_url);
    let (ctx, frames) = ctx_with_frames("sess-c", "run-c");
    let spawner = Arc::new(SubagentSpawnerImpl {
        app: (*app).share_handles(),
        ctx,
    });
    let parent_token = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn({
        let spawner = spawner.clone();
        let token = parent_token.clone();
        async move {
            spawner
                .spawn(r#"{"role":"explore","task":"慢调研"}"#, &token)
                .await
        }
    });
    // 等子代理出现（sub:spawn 事件 = 注册完成）
    wait_event(&events, keys::SUB_SPAWN, 5000).await;
    assert_eq!(app.subs.active_count(), 1);
    let sub_id = events.payloads_of(keys::SUB_SPAWN)[0]["subId"]
        .as_str()
        .unwrap()
        .to_owned();

    // stop_subagent：令牌置位即返回
    stop_subagent_core(&app, &sub_id).expect("取消命中活跃子代理");
    let result = tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("子 run 收尾不悬空")
        .expect("任务不 panic");
    // 工具结果 = 取消（已启动过）
    let crate::backend::run_loop::ToolExecution::Cancelled { executed, .. } = result else {
        panic!("取消场景应返回 Cancelled：{result:?}")
    };
    assert!(executed, "子 run 已启动后被收割");
    // 帧序：subDone 收尾，且子 runEnded(cancelled) 帧收齐（在 subDone 之前）
    let types = frames.frame_types();
    assert_eq!(types.last().unwrap(), "subDone");
    let sub_done_idx = types.iter().position(|t| t == "subDone").unwrap();
    let frames_before_done = &types[..sub_done_idx];
    assert!(
        frames_before_done.iter().any(|t| t == "subFrame"),
        "子帧透传存在"
    );
    // 提取最后一个 subFrame 的子帧 = runEnded(cancelled)
    let last_sub: &ChatFrame = &frames.snapshot()[sub_done_idx - 1].frame;
    let ChatFrame::SubFrame { frame, .. } = last_sub else {
        panic!("subDone 前一帧应为 subFrame")
    };
    let ChatFrame::RunEnded { reason, .. } = &**frame else {
        panic!("子代理最后透传帧应为 runEnded：{frame:?}")
    };
    assert_eq!(*reason, crate::backend::frames::EndReason::Cancelled);
    assert_eq!(
        events.payloads_of(keys::SUB_DONE)[0]["outcome"],
        "cancelled"
    );
    assert_eq!(app.subs.active_count(), 0, "注册表摘除干净");
    // 未知 subId 取消 → E_NOT_FOUND
    let err = stop_subagent_core(&app, "ghost").unwrap_err();
    assert_eq!(err.code, crate::util::errors::E_NOT_FOUND);
    drop(dir);
}

/// 参数错误面：未知角色 / 空 task → 工具级失败（不启动子 run、不发帧）。
#[tokio::test]
async fn subagent_rejects_bad_args() {
    let base_url = spawn_mock(Arc::new(MockMode::default())).await;
    let (app, dir, events) = app_with_mock(&base_url);
    let (ctx, frames) = ctx_with_frames("sess-x", "run-x");
    let spawner = SubagentSpawnerImpl {
        app: (*app).share_handles(),
        ctx,
    };
    let token = tokio_util::sync::CancellationToken::new();
    for (args, expects) in [
        (r#"{"role":"ceo","task":"x"}"#, "未知子代理角色"),
        (r#"{"role":"explore","task":"   "}"#, "task 不能为空"),
        (r#"{"role":"explore"}"#, "参数解析失败"),
    ] {
        let result = spawner.spawn(args, &token).await;
        let crate::backend::run_loop::ToolExecution::Completed { ok, model_text, .. } = result
        else {
            panic!("{args} 应为工具级失败")
        };
        assert!(!ok);
        assert!(model_text.contains(expects), "{args} → {model_text}");
    }
    assert!(frames.snapshot().is_empty(), "失败委派不发任何帧");
    assert!(events.payloads_of(keys::SUB_SPAWN).is_empty());
    assert_eq!(app.subs.active_count(), 0);
    drop(dir);
}

/// 非 Git 项目写入子代理拒绝（Q7：修改由主代理负责，子代理只读）。
#[tokio::test]
async fn subagent_write_role_rejected_on_non_git_project() {
    let base_url = spawn_mock(Arc::new(MockMode::default())).await;
    let (app, dir, events) = app_with_mock(&base_url);
    let (ctx, frames) = ctx_with_frames("sess-g", "run-g");
    let spawner = SubagentSpawnerImpl {
        app: (*app).share_handles(),
        ctx,
    };
    let token = tokio_util::sync::CancellationToken::new();
    // 临时会话的 project_root = <data>/workspace（非 git）
    let workspace = app.store.root().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    assert!(!crate::agent::project_is_git(&workspace));
    let result = spawner
        .spawn(r#"{"role":"implementer","task":"改点东西"}"#, &token)
        .await;
    let crate::backend::run_loop::ToolExecution::Completed { model_text, .. } = result else {
        panic!("应为工具级失败")
    };
    assert!(model_text.contains("非 Git 项目子代理只读"), "{model_text}");
    assert!(frames.snapshot().is_empty(), "拒绝路径不发 subStarted");
    assert!(events.payloads_of(keys::SUB_SPAWN).is_empty());
    drop(dir);
}

/// Git 项目的写入子代理：worktree 创建成功、无改动回收后清理干净
/// （worktree 目录与 lease 消失），子 run 正常 report。
#[tokio::test]
async fn subagent_write_role_uses_worktree_and_cleans_up() {
    let base_url = spawn_mock(Arc::new(MockMode::default())).await;
    let (app, dir, events) = app_with_mock(&base_url);
    // 把临时会话工作区变成 git 仓库（基线提交）
    let workspace = app.store.root().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let repo = git2::Repository::init(&workspace).unwrap();
    std::fs::write(workspace.join("README.md"), "base\n").unwrap();
    {
        let mut index = repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
    }
    let (ctx, _frames) = ctx_with_frames("sess-w", "run-w");
    let spawner = SubagentSpawnerImpl {
        app: (*app).share_handles(),
        ctx,
    };
    let token = tokio_util::sync::CancellationToken::new();
    let result = spawner
        .spawn(
            r#"{"role":"implementer","task":"检查基线即可，无需改动"}"#,
            &token,
        )
        .await;
    let crate::backend::run_loop::ToolExecution::Completed { ok, model_text, .. } = result else {
        panic!("委派应完成")
    };
    assert!(ok);
    // mock 不改文件 → 回收报告为空差量，worktree 已清理（成功汇回验证后清理）
    assert!(model_text.contains("无文件改动需要回收"), "{model_text}");
    let worktrees = app.store.root().join("worktrees");
    assert!(
        std::fs::read_dir(&worktrees)
            .map(|entries| entries.count() == 0)
            .unwrap_or(true),
        "worktrees 目录应已清空（无 lease、无 worktree 残留）"
    );
    assert!(
        events.payloads_of(keys::SUB_DONE)[0]["outcome"] == "report",
        "正常 report 收尾"
    );
    drop(dir);
}

// ---- 角色边界与执行入口（公共操作限制的工具面落实） ----

/// 子环境拒绝再委派（Q13）：工具表过滤 + 绕过工具表直接调用也在执行入口
/// 拒绝（E_TOOL_BLOCKED）；白名单外工具同样拦截（工具可见面 = 执行许可）。
#[tokio::test]
async fn child_env_rejects_delegation_and_outside_tools() {
    let ctx = crate::tools::ToolsContext::new(
        std::env::temp_dir().join("cw-subagent-test"),
        std::env::temp_dir().join("cw-subagent-test"),
    );
    let _ = std::fs::create_dir_all(&ctx.project_root);
    let base_env = |allowed: Option<Vec<String>>| {
        let ctx = crate::tools::ToolsContext::new(
            std::env::temp_dir().join("cw-subagent-test"),
            std::env::temp_dir().join("cw-subagent-test"),
        );
        ToolsetExecutor::new(ctx, crate::safety::tier::PermissionTier::AutoEdit).with_env(
            ExecutorEnv {
                events: Arc::new(crate::util::events::RecordingEventSink::new()),
                ask: Arc::new(Mutex::new(None)),
                plan: Arc::new(Mutex::new(None)),
                session_id: "s".to_owned(),
                run_id: "r".to_owned(),
                data_root: std::env::temp_dir().join("cw-subagent-test"),
                mcp: None,
                skills: None,
                // 子代理边界：无委派面（Q13 执行入口拒绝）
                subagents: None,
                allowed_tools: allowed,
            },
        )
    };
    let token = tokio_util::sync::CancellationToken::new();
    // ① 无白名单但无委派面：绕过工具表直接调用 subagent → 执行入口拒绝
    let result = base_env(None)
        .execute(
            "call-1",
            "subagent",
            r#"{"role":"explore","task":"x"}"#,
            &token,
        )
        .await;
    let crate::backend::run_loop::ToolExecution::Completed {
        ok,
        error,
        ui_summary,
        ..
    } = result
    else {
        panic!("应为工具级失败")
    };
    assert!(!ok);
    assert_eq!(error.unwrap().code, crate::util::errors::E_TOOL_BLOCKED);
    assert!(ui_summary.contains("子代理不能再委派"));
    // ② 白名单外工具（模型点名绕过可见面）→ 拦截；subagent 同样被白名单拦
    let executor = base_env(Some(vec!["read".to_owned(), "grep".to_owned()]));
    let result = executor
        .execute(
            "call-2",
            "write",
            r#"{"path":"a.txt","content":"x"}"#,
            &token,
        )
        .await;
    let crate::backend::run_loop::ToolExecution::Completed { error, .. } = result else {
        panic!("应为工具级失败")
    };
    assert_eq!(error.unwrap().code, crate::util::errors::E_TOOL_BLOCKED);
    let result = executor
        .execute(
            "call-2b",
            "subagent",
            r#"{"role":"explore","task":"x"}"#,
            &token,
        )
        .await;
    let crate::backend::run_loop::ToolExecution::Completed { error, .. } = result else {
        panic!("应为工具级失败")
    };
    assert_eq!(error.unwrap().code, crate::util::errors::E_TOOL_BLOCKED);
    // ③ 白名单内工具正常放行（read 只读）
    let result = executor
        .execute("call-3", "read", r#"{"path":"README.md"}"#, &token)
        .await;
    let crate::backend::run_loop::ToolExecution::Completed { ok, .. } = result else {
        panic!("白名单内工具应被执行")
    };
    assert!(
        !ok,
        "文件不存在按 read 的 E_NOT_FOUND 回喂（执行许可已放行）"
    );
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("cw-subagent-test"));
}

// ---- 请求面与历史转换（纯逻辑单测） ----

/// cleanContext 与父历史注入两形态的消息构造（§5.9 末行）。
#[test]
fn build_subagent_history_two_shapes() {
    let role = find_role("explore").unwrap();
    let task = "调研";
    // cleanContext=true（生产默认）：[系统提示, task]
    let clean = build_subagent_history(task, role, &[], true);
    assert_eq!(clean.len(), 2);
    assert!(matches!(
        &*clean[0],
        rig::completion::Message::System { .. }
    ));
    assert!(matches!(&*clean[1], rig::completion::Message::User { .. }));

    // cleanContext=false：父历史尾至多 6 条注入（8 条取尾 6）
    let parent_entry = |i: usize| crate::storage::MessageEntry {
        id: format!("m{i}"),
        parent_id: None,
        role: crate::storage::MessageRole::User,
        parts: vec![crate::storage::WirePart::Text {
            text: format!("msg{i}"),
        }],
        created_at: 0,
        run_id: None,
        status: None,
    };
    let tail: Vec<crate::storage::MessageEntry> = (0..8).map(parent_entry).collect();
    let injected = build_subagent_history(task, role, &tail, false);
    // 系统提示 + 6 条父历史 + task
    assert_eq!(injected.len(), 2 + 6);
    assert!(matches!(
        &*injected[0],
        rig::completion::Message::System { .. }
    ));
    assert!(matches!(
        &*injected[1],
        rig::completion::Message::User { .. }
    ));
    // 注入的是尾 6 条（m2..m7）
    let texts = injected[1..7]
        .iter()
        .map(|m| match &**m {
            rig::completion::Message::User { content } => content
                .iter()
                .filter_map(|c| match c {
                    rig::message::UserContent::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
                .collect::<String>(),
            _ => String::new(),
        })
        .collect::<Vec<_>>();
    assert_eq!(texts, (2..8).map(|i| format!("msg{i}")).collect::<Vec<_>>());
    // 尾条 = task 消息
    assert!(matches!(
        &*injected[7],
        rig::completion::Message::User { .. }
    ));
}

/// 子历史 JSONL → WireMessage 转换：toolCall/toolResult 同条目配对、
/// parent 链串行、End 行产出 report 条目、坏行忽略。
#[test]
fn load_sub_history_messages_conversion() {
    let dir = tempfile::tempdir().unwrap();
    let lines = [
        r#"{"type":"message","turn":1,"text":"第一轮结论","reasoning":"想一想"}"#,
        r#"{"type":"tool","turn":1,"callId":"call_1","name":"grep","argsJson":"{\"pattern\":\"x\"}","ok":true,"summary":"2 hits"}"#,
        r#"{"type":"message","turn":2,"text":"最终汇报全文"}"#,
        r#"{"type":"end","outcome":"report","report":"最终汇报全文"}"#,
        "{broken line", // 崩溃残留的坏行：忽略
    ];
    std::fs::write(dir.path().join("history.jsonl"), lines.join("\n") + "\n").unwrap();
    let messages = load_sub_history_messages(dir.path(), "sub-1").unwrap();
    // turn1 条目（toolCall+toolResult+reasoning+text）→ turn2 条目 → report 条目
    assert_eq!(messages.len(), 3, "转换结果：{messages:?}");
    assert_eq!(messages[0].id, "sub-1:a1");
    assert!(messages[0].parent_id.is_none());
    let has_call = messages[0]
        .parts
        .iter()
        .any(|p| matches!(p, crate::storage::WirePart::ToolCall { name, .. } if name == "grep"));
    let has_result = messages[0].parts.iter().any(
        |p| matches!(p, crate::storage::WirePart::ToolResult { ok, summary, .. } if *ok && summary == "2 hits"),
    );
    assert!(has_call && has_result, "toolCall 与 toolResult 同条目配对");
    assert_eq!(messages[1].id, "sub-1:a2");
    assert_eq!(
        messages[1].parent_id.as_deref(),
        Some("sub-1:a1"),
        "parent 链串行"
    );
    assert_eq!(messages[2].id, "sub-1:report");
    assert_eq!(messages[2].parent_id.as_deref(), Some("sub-1:a2"));
    assert!(matches!(
        &messages[2].parts[0],
        crate::storage::WirePart::Text { text } if text == "最终汇报全文"
    ));
}
