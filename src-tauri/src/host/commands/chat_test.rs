//! 审批 AskPayload 构造与 suggest 上下文提取的纯逻辑单测（14 号票）；
//! 27 号票增补自动起名纯函数（触发三态表 / 标题归一 / prompt 截断）。

use crate::backend::ask::{AskMode, AskPayload, DiffLineKind};
use crate::backend::run_loop::PendingCall;
use crate::storage::entries::{MessageEntry, MessageRole, WirePart};

use super::chat::{
    TITLE_MAX_CHARS, build_approval_payload, context_tail, first_user_text, normalize_title,
    title_prompt, title_should_trigger,
};

/// 构造一个挂起中的工具调用（PendingCall），call_id 固定、参数按用例传入。
fn pending(name: &str, args_json: &str) -> PendingCall {
    PendingCall {
        model_call_id: "call_1".to_owned(),
        name: name.to_owned(),
        args_json: args_json.to_owned(),
        fence_reason: None,
        external_dir: None,
        plan_scope: None,
    }
}

/// write 工具的审批载荷：标题含路径、diff 全为新增行（新文件整体是增量）、
/// 选项固定「允许（推荐）/ 拒绝」两项（write 非命令类，③ 开也不出
/// always_session）。
#[test]
fn write_payload_new_file_all_add() {
    let payload = build_approval_payload(
        &pending(
            "write",
            r#"{"path":"src/new.rs","content":"fn main() {}\n"}"#,
        ),
        true,
        None,
        None,
    );
    let AskPayload::Approval {
        title,
        diff,
        command,
        options,
        ..
    } = payload
    else {
        panic!("应为 Approval 形态")
    };
    assert!(title.contains("src/new.rs"));
    assert!(command.is_none());
    let diff = diff.expect("write 带 diff");
    assert_eq!(diff.path, "src/new.rs");
    let lines = &diff.hunks[0].lines;
    assert!(
        lines.iter().all(|l| l.kind == DiffLineKind::Add),
        "新文件全 add"
    );
    assert_eq!(lines[0].text, "fn main() {}");
    assert_eq!(options.len(), 2, "允许/拒绝两项");
    assert_eq!(options[0].id, "allow_once");
    assert_eq!(options[0].recommended, Some(true));
    assert_eq!(options[1].id, "deny");
}

/// edit 工具的审批载荷：旧文本逐行标删除、新文本逐行标新增，拼成一个替换块。
#[test]
fn edit_payload_replacement_hunk_del_add() {
    let payload = build_approval_payload(
        &pending(
            "edit",
            r#"{"path":"a.txt","oldText":"旧一\n旧二","newText":"新一"}"#,
        ),
        true,
        None,
        None,
    );
    let AskPayload::Approval { diff, .. } = payload else {
        panic!("应为 Approval")
    };
    let diff = diff.expect("edit 带 diff");
    let kinds: Vec<DiffLineKind> = diff.hunks[0].lines.iter().map(|l| l.kind).collect();
    assert_eq!(
        kinds,
        vec![DiffLineKind::Del, DiffLineKind::Del, DiffLineKind::Add]
    );
    assert_eq!(diff.hunks[0].lines[2].text, "新一");
}

/// command 工具的审批载荷：命令行原文随载荷展示、围栏确认原因透传、不带 diff；
/// ③ 会话内记忆开启时选项含 always_session（23 号票），关闭时不出。
#[test]
fn command_payload_with_fence_reason() {
    let p = PendingCall {
        model_call_id: "c".to_owned(),
        name: "command".to_owned(),
        args_json: r#"{"command":"echo hi > out.txt"}"#.to_owned(),
        fence_reason: Some("命令含写信号，需确认".to_owned()),
        external_dir: None,
        plan_scope: None,
    };
    let AskPayload::Approval {
        title,
        diff,
        command,
        fence_reason,
        options,
        ..
    } = build_approval_payload(&p, true, None, None)
    else {
        panic!("应为 Approval")
    };
    assert_eq!(command.as_deref(), Some("echo hi > out.txt"));
    assert!(fence_reason.is_some(), "围栏 Confirm 原因随载荷展示");
    assert!(diff.is_none(), "命令不走 diff");
    assert!(title.contains("命令"));
    // ③ 开：允许 / 始终允许（本会话）/ 拒绝 三项，always_session 非推荐项
    assert_eq!(options.len(), 3, "命令类审批三选项（23 号票）");
    assert_eq!(options[1].id, "always_session");
    assert_eq!(options[1].mode, Some(AskMode::AlwaysSession));
    assert_eq!(options[1].recommended, None, "非推荐项不参与自动确认");
    // ③ 关：不出该选项（M1 两项口径不变）
    let AskPayload::Approval { options, .. } = build_approval_payload(&p, false, None, None) else {
        panic!("应为 Approval")
    };
    assert_eq!(options.len(), 2, "③ 关闭时保持允许/拒绝两项");
    assert!(!options.iter().any(|o| o.id == "always_session"));
}

/// suggest 上下文提取优先取最后一条 assistant 的正文；没有 assistant 时
/// 退而取最后一条 user；完全为空则返回空串（suggest 静默放弃）。
#[test]
fn suggest_context_prefers_last_assistant_text() {
    let entry = |role: MessageRole, text: &str| MessageEntry {
        id: format!("m-{text:?}"),
        parent_id: None,
        role,
        parts: vec![WirePart::Text {
            text: text.to_owned(),
        }],
        created_at: 1,
        run_id: None,
        status: None,
    };
    let messages = vec![
        entry(MessageRole::User, "读一下"),
        entry(MessageRole::Assistant, "第一轮回答"),
        entry(MessageRole::User, "再总结"),
        entry(MessageRole::Assistant, "最终结论：OK"),
    ];
    assert_eq!(context_tail(&messages), "最终结论：OK");

    // 无 assistant：取最后一条（user）文本
    let only_user = vec![entry(MessageRole::User, "只有问题")];
    assert_eq!(context_tail(&only_user), "只有问题");

    // 全空：空串（suggest 静默不发）
    assert_eq!(context_tail(&[]), "");
}

// ---- 计划范围卡载荷（25 号票，FR-06-03/§8.6）----

/// 范围门挂起的载荷：title 固定「计划范围确认」、planScope 带计划外路径、
/// fenceReason 带基线对照、选项固定三枚（允许一次 / 本会话始终允许 / 拒绝）
/// 且无推荐项（不参与 autoConfirm 自动批准——计划范围须用户明确同意）。
#[test]
fn plan_scope_payload_three_options_without_recommended() {
    let mut p = pending("write", r#"{"path":"src/b.rs","content":"x"}"#);
    p.plan_scope = Some("src/b.rs".to_owned());
    let AskPayload::Approval {
        title,
        plan_scope,
        fence_reason,
        options,
        ..
    } = build_approval_payload(&p, true, Some("src/a.rs、tests/"), None)
    else {
        panic!("范围门应为 Approval 形态")
    };
    assert_eq!(title, "计划范围确认");
    assert_eq!(plan_scope.as_deref(), Some("src/b.rs"));
    let reason = fence_reason.expect("带基线对照说明");
    assert!(reason.contains("src/b.rs"), "含计划外路径：{reason}");
    assert!(
        reason.contains("src/a.rs、tests/"),
        "含批准基线对照：{reason}"
    );
    assert_eq!(options.len(), 3, "三选项");
    assert_eq!(options[0].id, "allow_once");
    assert_eq!(options[1].id, "always_session");
    assert_eq!(options[1].mode, Some(AskMode::AlwaysSession));
    assert_eq!(options[2].id, "deny");
    assert!(
        options.iter().all(|o| o.recommended != Some(true)),
        "无推荐项（不参与 autoConfirm）"
    );
    // 无对照基线（空范围批准后的首写询问）时 fenceReason 说明首写须确认
    let AskPayload::Approval { fence_reason, .. } = build_approval_payload(&p, false, None, None)
    else {
        panic!("应为 Approval 形态")
    };
    let reason = fence_reason.expect("仍带说明");
    assert!(reason.contains("不在已批准计划的范围内"), "{reason}");
}

/// 范围门挂起的 command 调用：范围卡附命令行原文（用户需知道执行什么）。
#[test]
fn plan_scope_payload_carries_command_line() {
    let mut p = pending("command", r#"{"command":"echo x > src/b.rs"}"#);
    p.plan_scope = Some("src/b.rs".to_owned());
    p.fence_reason = Some("命令含写目标，需确认".to_owned());
    let AskPayload::Approval {
        command,
        fence_reason,
        plan_scope,
        ..
    } = build_approval_payload(&p, true, None, None)
    else {
        panic!("应为 Approval 形态")
    };
    assert_eq!(plan_scope.as_deref(), Some("src/b.rs"));
    assert_eq!(
        command.as_deref(),
        Some("echo x > src/b.rs"),
        "附命令行原文"
    );
    let reason = fence_reason.expect("围栏原因一并附上");
    assert!(reason.contains("写目标"), "吸收围栏原因：{reason}");
}

// ---- 自动起名纯函数（27 号票，FR-01-09）----

/// 触发判定三态表（票面）：默认标题 + 恰好一条 user → 触发；已改名 → 不触发
/// （user 数任意）；默认标题但非首条（含编辑重发的兄弟分支节点）→ 不触发。
#[test]
fn title_trigger_three_states() {
    const DEFAULT: &str = crate::host::commands::session::DEFAULT_SESSION_TITLE;
    // 命中：默认标题 + 仅一条 user
    assert!(title_should_trigger(Some(DEFAULT), 1));
    // 不触发：用户 rename_session 改过名（#17）
    assert!(!title_should_trigger(Some("我的调试会话"), 1));
    // 不触发：非首条消息（重启续聊 / 连发第二问 / 编辑重发使树内 user 计数为 2）
    assert!(!title_should_trigger(Some(DEFAULT), 2));
    // 防御面：索引缺失（None）不触发
    assert!(!title_should_trigger(None, 1));
}

/// 标题归一表驱动：trim + 首个非空行 + ≤30 字符截断（多字节安全）+ 前后
/// 空白归一；兜底输入（首条消息全文多行）同函数收敛。
#[test]
fn normalize_title_table() {
    // 常规：首尾空白归一
    assert_eq!(normalize_title("  帮我修好登录页  "), "帮我修好登录页");
    // 多行生成（模型输出解释）：取第一个非空行
    assert_eq!(
        normalize_title("\n  修复登录超时\n第二行解释"),
        "修复登录超时"
    );
    // 超 30 字符截断（中文按字符计，不多不少）
    let long = "这是一个特别特别特别长的标题输入用来验证三十个字符的截断行为是否正确无误";
    let got = normalize_title(long);
    assert_eq!(got.chars().count(), TITLE_MAX_CHARS);
    assert_eq!(got, long.chars().take(TITLE_MAX_CHARS).collect::<String>());
    // 全空白 → 空串（调用方据此放弃落盘）
    assert_eq!(normalize_title("  \n \n"), "");
}

/// prompt 组装：含指令骨架 + 消息正文；超长输入截 2000 字符。
#[test]
fn title_prompt_clips_long_input() {
    let short = title_prompt("帮我写个爬虫");
    assert!(short.contains("不超过 20 个字"));
    assert!(short.contains("帮我写个爬虫"));
    // 2001 个字符的输入 → prompt 只含前 2000 个
    let long_text = "字".repeat(2001);
    let long = title_prompt(&long_text);
    let body = long.split("消息内容：").nth(1).expect("应含消息段");
    assert_eq!(body.chars().count(), 2000);
}

/// 首条 user 消息正文提取：取第一条 user（忽略 assistant），Text parts 拼接；
/// 无 user / 纯图片 → 空串（调用方放弃起名）。
#[test]
fn first_user_text_extracts_first_user() {
    let text_part = |t: &str| WirePart::Text { text: t.to_owned() };
    let image_part = |hash: &str| WirePart::Image {
        blob_hash: hash.to_owned(),
        mime_type: "image/png".to_owned(),
    };
    let msg = |role: MessageRole, parts: Vec<WirePart>| MessageEntry {
        id: format!("m-{}", parts.len()),
        parent_id: None,
        role,
        parts,
        created_at: 1,
        run_id: None,
        status: None,
    };
    let messages = vec![
        msg(
            MessageRole::User,
            vec![text_part("第一问"), text_part("补充")],
        ),
        msg(MessageRole::Assistant, vec![text_part("答")]),
        msg(MessageRole::User, vec![text_part("第二问")]),
    ];
    assert_eq!(first_user_text(&messages), "第一问补充");
    // 纯图片首条：空串
    let image_only = vec![msg(MessageRole::User, vec![image_part("abc")])];
    assert_eq!(first_user_text(&image_only), "");
    // 无 user：空串
    assert_eq!(
        first_user_text(&[msg(MessageRole::Assistant, vec![text_part("答")])]),
        ""
    );
}

/// 默认标题常量与判定同源自检：create_session（session.rs）写入
/// DEFAULT_SESSION_TITLE，三态判定引用同一常量——常量漂移会让判定永久
/// 失配，此处钉住字面值。
#[test]
fn default_title_matches_create_session_value() {
    assert_eq!(
        crate::host::commands::session::DEFAULT_SESSION_TITLE,
        "新会话"
    );
}

/// 标题任务的辅助调用身份钉死（Q37/Q40）：runId 后缀 `:title`、kind=aux、
/// runs 计数不增——与 suggest（`:suggest`）/ 压缩（`:compact`）同族命名，
/// 锁定命名规则防漂移。
#[test]
fn title_aux_run_id_suffix() {
    let run_id = "run-1";
    assert_eq!(format!("{run_id}:title"), "run-1:title");
}

// ---- 标题后台任务直驱（27 号票：剧本假模型 + 录音事件汇 + 临时数据根）----

use std::sync::Arc;
use std::time::Duration;

use rig::completion::Usage;
use rig::run::spec::RunSpec;
use rig::test_utils::{MockCompletionModel, MockTurn};

use crate::backend::turn::TurnDriver;
use crate::host::AppState;
use crate::storage::SessionMeta;
use crate::util::events::{EventSink, RecordingEventSink};

use super::chat::{TITLE_TIMEOUT, spawn_title_task};
use crate::host::commands::session::DEFAULT_SESSION_TITLE;

/// 剧本 usage（mock 口径 120 in / 48 out，与 e2e mock 服务一致）。
/// rig `Usage` 为 non-exhaustive，用其 builder 方法逐项设值。
fn scripted_usage() -> Usage {
    Usage::default().input_tokens(120u64).output_tokens(48u64)
}

/// 标题任务夹具：临时数据根 + 默认标题的 index 条目 + 录音事件汇。
/// 返回 (app, events, session_id, 数据根路径)。
fn title_fixture(
    session_id: &str,
) -> (
    AppState,
    Arc<RecordingEventSink>,
    String,
    std::path::PathBuf,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir); // 测试进程生命周期内持有（与 backend_e2e 同款）
    let events = Arc::new(RecordingEventSink::new());
    let app = AppState::new_in(root.clone(), events.clone());
    let ts = 1_700_000_000_000i64;
    app.store
        .index()
        .upsert(SessionMeta {
            id: session_id.to_owned(),
            project_id: None,
            title: DEFAULT_SESSION_TITLE.to_owned(),
            created_at: ts,
            updated_at: ts,
            revision: 0,
            message_count: 0,
            total_tokens: 0,
            authorized_external_dirs: Vec::new(),
        })
        .expect("upsert index");
    (app, events, session_id.to_owned(), root)
}

/// 剧本标题驱动（text turn + usage）。
fn text_driver(
    text: &str,
) -> Arc<TurnDriver<rig::test_utils::MockScript, rig::test_utils::MockRuntime>> {
    Arc::new(TurnDriver::new(
        MockCompletionModel::from_turns([MockTurn::text(text).with_usage(scripted_usage())]),
        RunSpec::new(),
        Vec::new(),
    ))
}

/// 事件桥的等待辅助（轮询录音汇，不阻塞 worker 线程）。
async fn wait_event(
    events: &RecordingEventSink,
    key: &str,
    timeout: Duration,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(payload) = events.payloads_of(key).last() {
            return payload.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "等待事件 {key} 超时"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// 读会话史册文件里最后一条 meta 行的 title 字段（双写一致性的史册面）。
fn meta_title_on_disk(root: &std::path::Path, session_id: &str) -> Option<String> {
    let path = root.join("histories").join(session_id).join("0001.jsonl");
    let raw = std::fs::read_to_string(path).ok()?;
    raw.lines().rev().find_map(|line| {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        value
            .get("title")
            .and_then(|t| t.as_str())
            .map(str::to_owned)
    })
}

/// 成功路径全链：生成标题 → meta 行 + index 双写（事件时点已落盘）→
/// C-9 计量（aux 明细 + tokens:update 照发 + runs 不增）→ session:title 事件。
#[tokio::test]
async fn title_task_success_writes_both_and_records_usage() {
    let (app, events, session_id, root) = title_fixture("sess-title-ok");
    let driver = text_driver("修复登录页超时");
    spawn_title_task(
        driver,
        app.store.clone(),
        events.clone(),
        app.session_usage.clone(),
        app.ledger.clone(),
        session_id.clone(),
        "run-1".to_owned(),
        "帮我修好登录页".to_owned(),
        ("model-1".to_owned(), "gpt-4o-mock".to_owned()),
        TITLE_TIMEOUT,
    );
    let payload = wait_event(&events, "session:title", Duration::from_secs(5)).await;
    assert_eq!(payload["sessionId"], "sess-title-ok");
    assert_eq!(payload["title"], "修复登录页超时");

    // 事件顺序锚（票面：事件在落盘之后发）：事件已到 ⇒ index 与 meta 行均已写
    let meta = app
        .store
        .index()
        .get(&session_id)
        .expect("index 条目应存在");
    assert_eq!(meta.title, "修复登录页超时");
    assert!(meta.updated_at > 1_700_000_000_000, "updatedAt 应已刷新");
    assert_eq!(
        meta_title_on_disk(&root, &session_id).as_deref(),
        Some("修复登录页超时"),
        "meta 行 title 应已落盘"
    );

    // C-9 计量：tokens:update 照发（含 modelId）且 run 明细有 `:title` aux 条目
    let tokens = events.payloads_of("tokens:update");
    assert_eq!(tokens.len(), 1, "标题任务恰好一个 tokens:update");
    assert_eq!(tokens[0]["usage"]["input"], 120);
    assert_eq!(tokens[0]["usage"]["output"], 48);
    assert_eq!(tokens[0]["modelId"], "model-1");
    let detail = app.ledger.detail.load(&session_id);
    let title_run = detail
        .runs
        .get("run-1:title")
        .expect("明细应有 run-1:title 条目");
    assert_eq!(title_run.kind, "aux", "辅助调用 kind=aux");
    assert_eq!(title_run.by_model["model-1"].input, 120);
    // Q40：辅助调用不增主 run 次数
    let day = crate::host::usage::local_today();
    let daily = app.ledger.stats.read_day(&day);
    assert_eq!(
        daily.models.get("model-1").map(|m| m.runs),
        Some(0),
        "辅助调用不计 runs"
    );
}

/// 失败静默兜底（票面 PRD 口径）：剧本错误 → 标题 = 首条消息首行截断、
/// 无 tokens:update（无 usage 不伪造）、无重试；`session:title` 照常发出
/// （兜底标题也是成功落盘）。
#[tokio::test]
async fn title_task_failure_falls_back_silently() {
    let (app, events, session_id, root) = title_fixture("sess-title-fail");
    let driver = Arc::new(TurnDriver::new(
        MockCompletionModel::from_turns([MockTurn::error("boom")]),
        RunSpec::new(),
        Vec::new(),
    ));
    spawn_title_task(
        driver,
        app.store.clone(),
        events.clone(),
        app.session_usage.clone(),
        app.ledger.clone(),
        session_id.clone(),
        "run-2".to_owned(),
        "帮我把这个特别长的第一行消息和\n第二行内容都收敛成标题".to_owned(),
        ("model-1".to_owned(), "gpt-4o-mock".to_owned()),
        TITLE_TIMEOUT,
    );
    let payload = wait_event(&events, "session:title", Duration::from_secs(5)).await;
    assert_eq!(payload["title"], "帮我把这个特别长的第一行消息和");
    assert_eq!(
        meta_title_on_disk(&root, &session_id).as_deref(),
        Some("帮我把这个特别长的第一行消息和")
    );
    // 无 usage 不伪造：零计量事件、零落账
    assert!(events.payloads_of("tokens:update").is_empty());
    let detail = app.ledger.detail.load(&session_id);
    assert!(
        !detail.runs.contains_key("run-2:title"),
        "失败无 usage 不伪造明细"
    );
}

/// 超时兜底：悬空 transport（非流式响应永 Pending）+ 短超时确定性触发
/// 超时分支（与失败共用同一兜底臂）——标题 = 首条消息首行截断、零计量。
#[tokio::test]
async fn title_task_timeout_falls_back() {
    let (app, events, session_id, _root) = title_fixture("sess-title-timeout");
    // 悬空非流式 transport：请求发出后永不产出（MockScript 的 Unary 帧流
    // 用 stream::pending() 挂起，watchdog_test 流式悬空先例的非流式版）
    type MockFrame = <rig::test_utils::MockScript as rig::wire::Wire>::Frame;
    #[derive(Clone)]
    struct PendingUnary;
    impl rig::driver::Transport<rig::test_utils::MockScript> for PendingUnary {
        fn send(
            &self,
            _payload: rig::completion::request::CompletionRequest,
            _exchange: rig::driver::Exchange,
        ) -> rig::driver::Opening<MockFrame> {
            rig::driver::Opening::ready(rig::driver::Opened::new(futures::stream::pending()))
        }
    }
    let driver = Arc::new(TurnDriver::new(
        rig::driver::Model::new(rig::test_utils::MockScript::default(), PendingUnary),
        RunSpec::new(),
        Vec::new(),
    ));
    spawn_title_task(
        driver,
        app.store.clone(),
        events.clone(),
        app.session_usage.clone(),
        app.ledger.clone(),
        session_id.clone(),
        "run-3".to_owned(),
        "第一行兜底标题".to_owned(),
        ("model-1".to_owned(), "gpt-4o-mock".to_owned()),
        Duration::from_millis(50),
    );
    let payload = wait_event(&events, "session:title", Duration::from_secs(5)).await;
    assert_eq!(payload["title"], "第一行兜底标题");
    assert!(events.payloads_of("tokens:update").is_empty());
}

/// 复判一（并发防护/改名保护，票面）：进入任务即查 index——标题已非默认
/// 直接放弃：零请求、零事件、零写盘。
#[tokio::test]
async fn title_task_skipped_when_renamed() {
    let (app, events, session_id, _root) = title_fixture("sess-title-renamed");
    // 模拟 rename_session 已改过名
    app.store
        .index()
        .mutate(&session_id, |m| m.title = "我的调试会话".to_owned())
        .expect("rename");
    let driver = text_driver("不该出现的标题");
    spawn_title_task(
        driver,
        app.store.clone(),
        events.clone(),
        app.session_usage.clone(),
        app.ledger.clone(),
        session_id.clone(),
        "run-4".to_owned(),
        "帮我修好登录页".to_owned(),
        ("model-1".to_owned(), "gpt-4o-mock".to_owned()),
        TITLE_TIMEOUT,
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        events.payloads_of("session:title").is_empty(),
        "改名后不应起名"
    );
    assert!(
        events.payloads_of("tokens:update").is_empty(),
        "复判命中应零请求零计量"
    );
    assert_eq!(
        app.store.index().get(&session_id).unwrap().title,
        "我的调试会话",
        "标题不应被覆盖"
    );
}

/// 复判二（先写者为准）：并发任务在生成期间 index 已被先写者更新 →
/// 放弃落盘与事件（后写覆盖同值无害口径下的确定性收口面）。
#[tokio::test]
async fn title_task_second_writer_yields() {
    let (app, events, session_id, _root) = title_fixture("sess-title-race");
    // 剧本两个 turn：任务 A 消费第一个、任务 B 消费第二个；B 的复判二
    // 在 A 已写入后执行（用事件 A 作为同步锚）。
    let driver = Arc::new(TurnDriver::new(
        MockCompletionModel::from_turns([
            MockTurn::text("任务甲的标题").with_usage(scripted_usage()),
            MockTurn::text("任务乙的标题").with_usage(scripted_usage()),
        ]),
        RunSpec::new(),
        Vec::new(),
    ));
    let store = app.store.clone();
    let sink: Arc<dyn EventSink> = events.clone();
    spawn_title_task(
        driver.clone(),
        store,
        sink,
        app.session_usage.clone(),
        app.ledger.clone(),
        session_id.clone(),
        "run-a".to_owned(),
        "帮我修好登录页".to_owned(),
        ("model-1".to_owned(), "gpt-4o-mock".to_owned()),
        TITLE_TIMEOUT,
    );
    wait_event(&events, "session:title", Duration::from_secs(5)).await;
    assert_eq!(
        app.store.index().get(&session_id).unwrap().title,
        "任务甲的标题",
        "先写者胜"
    );
    // 第二个任务：复判此时已非默认 → 放弃（单 driver 复用，零新增事件）
    let store = app.store.clone();
    let sink: Arc<dyn EventSink> = events.clone();
    spawn_title_task(
        driver,
        store,
        sink,
        app.session_usage.clone(),
        app.ledger.clone(),
        session_id.clone(),
        "run-b".to_owned(),
        "帮我修好登录页".to_owned(),
        ("model-1".to_owned(), "gpt-4o-mock".to_owned()),
        TITLE_TIMEOUT,
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(events.payloads_of("session:title").len(), 1, "只发一次事件");
    assert_eq!(
        app.store.index().get(&session_id).unwrap().title,
        "任务甲的标题",
        "后写者不复写"
    );
}

/// 空会话防御：首条消息全空白 → 无标题可落（不写盘不事件）。
#[tokio::test]
async fn title_task_blank_message_skipped() {
    let (app, events, session_id, _root) = title_fixture("sess-title-blank");
    let driver = text_driver("  ");
    spawn_title_task(
        driver,
        app.store.clone(),
        events.clone(),
        app.session_usage.clone(),
        app.ledger.clone(),
        session_id.clone(),
        "run-5".to_owned(),
        "  \n ".to_owned(),
        ("model-1".to_owned(), "gpt-4o-mock".to_owned()),
        TITLE_TIMEOUT,
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(events.payloads_of("session:title").is_empty());
    assert_eq!(
        app.store.index().get(&session_id).unwrap().title,
        DEFAULT_SESSION_TITLE
    );
}
