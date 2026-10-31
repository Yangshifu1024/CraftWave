//! web 工具审批链 e2e（12 号票）：confirm_each 档 web_fetch 挂起 → 用户
//! allow → 放行全链（真实抓取本地 fixture）。
//!
//! 复用 M1 14 号票审批 e2e 模式（37 号票定稿的 drive_run 剧本驱动）：
//! 剧本假模型 + 真 ApprovalGate（confirm_each 档）+ 真 ToolsetExecutor
//! （装配 NetworkEnv）+ 本地 axum fixture（127.0.0.1 零外网）。两次驱动：
//! 第一次门控挂起（批内零执行——fixture 未收到任何请求）；宿主按
//! resolve_ask 放行语义登记签名（`ApprovalGate::approve`）后重新驱动，
//! 同调用按签名放行 → 真实执行 → run 自然收尾。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rig::completion::request::ToolDefinition;
use rig::message::ToolName;
use rig::run::AgentRun;
use rig::run::spec::RunSpec;
use rig::test_utils::{MockCompletionModel, MockStreamEvent};
use tokio_util::sync::CancellationToken;

use crate::backend::exec::{ExecutorEnv, ToolsetExecutor};
use crate::backend::frames::CollectingSink;
use crate::backend::gate::ApprovalGate;
use crate::backend::run_loop::{RunExit, ToolGate, drive_run};
use crate::backend::turn::TurnDriver;
use crate::safety::ssrf::{HostThrottle, SsrfPolicy};
use crate::safety::tier::PermissionTier;
use crate::tools::plan::PlanState;
use crate::tools::{NetworkEnv, ToolsContext};

/// fixture 命中计数（挂起零执行 vs 放行真执行的断言面）。
type HitCount = Arc<AtomicUsize>;

/// 起本地 axum fixture：/page 返回一小段正文并计数。
async fn spawn_fixture(hits: HitCount) -> std::net::SocketAddr {
    let app = axum::Router::new().route(
        "/page",
        axum::routing::get(move || {
            let hits = hits.clone();
            async move {
                hits.fetch_add(1, Ordering::SeqCst);
                "<html><body><p>审批链抓取成功正文</p></body></html>"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定本地回环端口");
    let addr = listener.local_addr().expect("端口");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

/// 剧本一轮：web_fetch 调用 + 收尾。
fn web_call_turn(call_id: &str, url: &str) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::tool_call(call_id, "web_fetch", serde_json::json!({ "url": url })),
        MockStreamEvent::final_response_with_default_usage(),
    ]
}

/// 文本收尾一轮（工具结果回喂后模型的总结响应）。
fn text_turn(text: &str) -> Vec<MockStreamEvent> {
    vec![
        MockStreamEvent::text(text),
        MockStreamEvent::final_response_with_default_usage(),
    ]
}

/// 剧本模型的工具表（AgentRun 校验模型调用须在 advertised tools 内）。
fn web_tool_table() -> Vec<Arc<ToolDefinition>> {
    vec![Arc::new(ToolDefinition::new(
        ToolName::new("web_fetch").expect("合法工具名"),
        "网页抓取（审批链 e2e）".to_owned(),
        crate::tools::web_fetch::spec().params_schema,
    ))]
}

/// 装配：真执行器（NetworkEnv 回环放行）+ 真门控（confirm_each 档）。
fn assemble(
    tier: PermissionTier,
    plan: Arc<std::sync::Mutex<Option<PlanState>>>,
) -> (ToolsetExecutor, ApprovalGate) {
    let executor = ToolsetExecutor::new(
        ToolsContext::new(
            std::env::temp_dir().join("cw-web-gate-e2e"),
            std::env::temp_dir().join("cw-web-gate-e2e"),
        ),
        tier,
    )
    .with_env(ExecutorEnv {
        events: Arc::new(crate::util::events::RecordingEventSink::new()),
        ask: Arc::new(std::sync::Mutex::new(None)),
        plan: plan.clone(),
        session_id: "sess-e2e".to_owned(),
        run_id: "run-e2e".to_owned(),
        data_root: std::env::temp_dir().join("cw-web-gate-e2e"),
        ask_log: None,
        mcp: None,
        skills: None,
        subagents: None,
        allowed_tools: None,
        network: Some(Arc::new(NetworkEnv {
            policy: SsrfPolicy {
                allow_private_network: true,
            },
            proxy: None,
            throttle: Arc::new(HostThrottle::new(Duration::ZERO)),
            // 13 号票 web_search 三通道环境：本 e2e 只验 web_fetch 审批门，
            // 搜索通道保持零装配（未启用 + 未配 Key = 不可用面）
            search: crate::tools::web_search::SearchEnv {
                searxng: None,
                tavily_api_key: None,
                max_results_default: 10,
                policy: SsrfPolicy {
                    allow_private_network: true,
                },
                proxy: None,
                throttle: Arc::new(HostThrottle::new(Duration::ZERO)),
                ddg_endpoint: None,
                tavily_endpoint: None,
            },
        })),
        store: None,
    });
    let gate = ApprovalGate::new(tier, plan);
    (executor, gate)
}

/// drive_run 的直驱封装（run_spec 的 max_turns=4 足够单工具轮）。
async fn drive<W, T>(
    driver: &TurnDriver<W, T>,
    executor: Arc<dyn crate::backend::run_loop::CancellableExecutor>,
    gate: Arc<ApprovalGate>,
    sink: &mut CollectingSink,
) -> Result<RunExit, rig::run::PromptError>
where
    W: rig::wire::Wire<Op = rig::operation::Completion>,
    T: rig::driver::Transport<W>,
{
    let run = AgentRun::from_spec(
        &RunSpec {
            max_turns: Some(4),
            ..RunSpec::new()
        },
        "抓取网页",
        Some(Vec::new()),
    );
    drive_run(
        driver,
        run,
        "run-e2e",
        "scripted-model",
        CancellationToken::new(),
        executor,
        gate,
        sink,
        None,
        None::<&dyn crate::backend::run_loop::SteerSource>,
        None::<&dyn crate::backend::run_loop::RequestRefresher>,
        Duration::from_secs(30),
    )
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn confirm_each_suspends_then_allow_completes_real_fetch() {
    let hits: HitCount = Arc::new(AtomicUsize::new(0));
    let addr = spawn_fixture(hits.clone()).await;
    let url = format!("http://{addr}/page");
    let args_json = serde_json::json!({ "url": url }).to_string();

    let plan: Arc<std::sync::Mutex<Option<PlanState>>> = Arc::new(std::sync::Mutex::new(None));
    let (executor, gate) = assemble(PermissionTier::ConfirmEach, plan);

    // ---- 第一次驱动：confirm_each 档 web_fetch 应挂起（批内零执行）----
    let model = MockCompletionModel::from_stream_turns(vec![web_call_turn("call-1", &url)]);
    let driver = TurnDriver::new(model.clone(), RunSpec::new(), web_tool_table());
    let mut sink = CollectingSink::default();
    let exit = drive(&driver, Arc::new(executor), Arc::new(gate), &mut sink)
        .await
        .expect("挂起是 Ok 退出态");
    let RunExit::Suspended(info) = &exit else {
        panic!("confirm_each 档 web_fetch 应挂起，实际 {exit:?}");
    };
    assert_eq!(info.pending.name, "web_fetch");
    assert_eq!(info.pending.args_json, args_json);
    // 门控判定链回放：同参数在批准前再次判定仍挂起（Suspend 语义面）
    assert_eq!(hits.load(Ordering::SeqCst), 0, "挂起批内应零执行");

    // ---- 用户 allow：宿主 resolve_ask 放行语义 = 登记已批准签名 ----
    // （重建门控不影响生产语义：生产由 host 持有同一 ApprovalGate；
    // 此处为保持两次驱动同门控实例，重新装配并登记签名）
    let plan2: Arc<std::sync::Mutex<Option<PlanState>>> = Arc::new(std::sync::Mutex::new(None));
    let (executor2, gate2) = assemble(PermissionTier::ConfirmEach, plan2);
    gate2.approve("web_fetch", &args_json);

    // ---- 第二次驱动：同调用按签名放行 → 真实抓取 → 自然收尾 ----
    // 剧本两轮：抓取调用轮 + 工具结果回喂后的文本总结轮
    let model2 = MockCompletionModel::from_stream_turns(vec![
        web_call_turn("call-1", &url),
        text_turn("已抓取网页"),
    ]);
    let driver2 = TurnDriver::new(model2.clone(), RunSpec::new(), web_tool_table());
    let mut sink2 = CollectingSink::default();
    let exit2 = drive(&driver2, Arc::new(executor2), Arc::new(gate2), &mut sink2)
        .await
        .expect("放行后驱动应 Ok");
    assert!(
        matches!(exit2, RunExit::Done(_)),
        "放行后应自然收尾：{exit2:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1, "放行后应真实执行一次抓取");
}

#[tokio::test(flavor = "current_thread")]
async fn plan_tier_filters_web_tools_from_gate() {
    // plan 档：web_fetch（Network）从工具表过滤，门控兜底硬拦
    // （E_TOOL_BLOCKED）——即使模型点名调用也不执行
    let hits: HitCount = Arc::new(AtomicUsize::new(0));
    let addr = spawn_fixture(hits.clone()).await;
    let url = format!("http://{addr}/page");
    let plan: Arc<std::sync::Mutex<Option<PlanState>>> = Arc::new(std::sync::Mutex::new(None));
    let (executor, gate) = assemble(PermissionTier::Plan, plan);
    let specs = crate::tools::filter_for_tier(crate::tools::build_registry(), PermissionTier::Plan);
    assert!(
        !specs.iter().any(|s| s.name == "web_fetch"),
        "plan 档工具表不应包含 web_fetch"
    );
    assert!(
        !specs.iter().any(|s| s.name == "http_request"),
        "plan 档工具表不应包含 http_request"
    );
    // 门控兜底（模型绕过工具表点名调用）：E_TOOL_BLOCKED 硬拦
    let verdict = gate.judge("web_fetch", &serde_json::json!({ "url": url }).to_string());
    match verdict {
        crate::backend::run_loop::GateVerdict::Blocked { code, .. } => {
            assert_eq!(code, "E_TOOL_BLOCKED");
        }
        other => panic!("plan 档应硬拦，实际 {other:?}"),
    }
    let _ = executor; // 执行器装配面在上一用例已全链验证
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}
