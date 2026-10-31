//! start_chat 装配树（平移自旧壳 `host/commands/chat.rs` `start_chat_core`）。
//!
//! 装配阶段（次序即语义次序，各步「为什么」见行内注释）：
//! ① 会话偏好起点快照 → 模型解析 → 图片 blob 装配 → wire 参数校验；
//! ② 系统前缀编排（customPrompt → 项目指令 → ai_language → 压缩 summary
//!    → 技能清单）+ 压缩策略与摘要驱动；
//! ③ 工具集（静态段 + MCP 动态段，档位过滤）；
//! ④ 同会话互斥预检 → 帧枢纽就位 → run 开始前压缩（触发点①）→ 预算复查；
//! ⑤ wire 切分（压缩边界 + image 双形态）→ 系统前缀注入；
//! ⑥ RunManager 注册 + run:start 事件；
//! ⑦ 驱动/执行器/审批门/每拍刷新源/steer 源装配；
//! ⑧ 自动起名判定（后台标题任务）；
//! ⑨ spawn run 任务（`run_task::drive_main_run`）。
//!
//! 泛型接缝：模型解析经 [`ModelResolver`](super::model::ModelResolver)——
//! 生产 = `dialect_resolver()`（AppConfig 查找 + 三方言驱动）；测试 =
//! rig 剧本假模型。装配逻辑一行不改。

use std::sync::{Arc, Mutex};

use rig::run::spec::RunSpec;

use crate::agent::FrameHub;
use crate::app_state::AppState;
use crate::agent::RunManager;
use crate::backend::exec::{ExecutorEnv, ToolsetExecutor};
use crate::backend::gate::ApprovalGate;
use crate::backend::run_loop::{RequestRefresher, SteerSource};
use crate::backend::turn::TurnDriver;
use crate::backend::wire as bwire;
use crate::backend::estimator::is_over_wire_budget;
use crate::storage::{MessageRole, StoredEntry};
use crate::util::errors::IpcError;
use crate::util::events::keys;

use super::RunConfig;
use super::compaction::{compaction_policy, context_overflow, maybe_compact_before_run};
use super::model::{
    ModelResolver, ModelSlot, RunTransport, RunWire, effective_thinking_directive,
    load_image_blobs, to_rig_tool_definitions,
};
use super::run_task::{RunTaskDeps, drive_main_run};
use super::sinks::{HostAskLogger, HostLlmLogger, now_ms};
use super::skills::ensure_skills_snapshot;
use super::subagent::SubagentSpawnerImpl;
use super::tasks::{
    HostRequestRefresher, HostSteerSource, EffectiveRuntime, context_tail, first_user_text,
    spawn_title_task, title_should_trigger, TITLE_TIMEOUT,
};

/// user 条目幂等兜底：末条 user 不在史册时以史册链尾为父追加（一次 canonical
/// 落盘即 fsync）。正常协议下前端发送前已 save，此处立即跳过。
fn persist_user_fallback(app: &AppState, session_id: &str, messages: &[crate::storage::MessageEntry]) {
    let Some(user) = messages.iter().rev().find(|m| m.role == MessageRole::User) else {
        return;
    };
    let known = app
        .store
        .load_session_tree(session_id)
        .ok()
        .map(|tree| tree.messages.iter().any(|m| m.id == user.id))
        .unwrap_or(false);
    if known {
        return;
    }
    let mut entry = user.clone();
    if entry.parent_id.is_none()
        && let Ok(tree) = app.store.load_session_tree(session_id)
        && let Some(leaf) = tree.active_leaf_id
    {
        entry.parent_id = Some(leaf);
    }
    if let Err(e) = app
        .store
        .append_canonical(session_id, &[StoredEntry::Message(entry)])
    {
        #[cfg(debug_assertions)]
        eprintln!("[craftwave] user 条目兜底落盘失败（{session_id}）：{e:?}");
        #[cfg(not(debug_assertions))]
        let _ = e;
    }
}

/// start_chat 装配树本体（泛型入口；生产包装见 `super::start_chat`）。
///
/// 帧出口：入参 sink（生产 = GPUI 壳的 SequencedSink 实现，测试 =
/// 收集器）作为 [`FrameHub`] 的初始下发端——seq 递增、环形缓冲（512 帧）
/// 与下发收进枢纽；压缩编排帧与 run 帧共用同一出口，重载后 `attach_run`
/// 可重放（含 compactDone）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_chat_assembled<W: RunWire, T: RunTransport<W>>(
    app: &AppState,
    runs: &RunManager,
    session_id: String,
    messages: Vec<crate::storage::MessageEntry>,
    run_config: RunConfig,
    sink: Box<dyn crate::backend::frames::SequencedSink>,
    auto_confirm: bool,
    resolve: ModelResolver<W, T>,
) -> Result<super::StartChatResult, IpcError> {
    if session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    // ① 会话偏好起点快照：prefs 文件优先于
    //    RunConfig（前端两者同步写，文件缺失/字段未设时回落 RunConfig——
    //    旧前端载荷兼容面）。模型解析与 thinking 初值都按此快照生效。
    let prefs_initial = crate::storage::prefs::load_prefs(app.store.root(), &session_id);
    let initial_model_id = prefs_initial.model_id.or(run_config.model_id.clone());
    let initial_thinking = prefs_initial.thinking.or(run_config.thinking);
    // ① 模型解析
    let model = resolve(app, initial_model_id.as_deref())?;
    // ①a 图片 blob 装配：收集消息里全部 image part 的哈希，
    //    从会话 imgblob 目录读出（mime 按魔数嗅探恢复）。读取失败跳过该表项
    //    （vision 模型下对应图片降级为占位文本，不拒绝请求——历史图片的
    //    blob 可能已被会话删除流程清走）；日志纪律：只记哈希，不记字节。
    let image_blobs = load_image_blobs(app.store.root(), &session_id, &messages);
    // ①b wire 参数校验（末条 user 为 prompt；image part 按 vision 双
    //    形态校验——非 vision 模型的本次新附图在此拒绝）——压缩编排前
    //    先验，让参数错误优先于任何有副作用的流程
    let vision_policy = bwire::VisionPolicy {
        vision: model.vision,
        blobs: &image_blobs,
    };
    bwire::split_prompt_visioned(&messages, None, &vision_policy)
        .map_err(|e| IpcError::invalid_arg(e.to_string()))?;
    // ② 系统级前缀与压缩策略（注入次序：customPrompt → 项目指令 →
    //    ai_language → 压缩 summary → 技能清单）。save_config 即时生效于
    //    下次 run（config.load 命中缓存热替换后的值）。
    let config_snapshot = app.config.load()?;
    let custom_prompt = config_snapshot
        .agent
        .as_ref()
        .and_then(|a| a.custom_prompt.clone());
    // ai_language 注入层：模型回复语言偏好，非空用自定义值、空跟随界面语言。
    let ai_language_layer = bwire::ai_language_layer(
        config_snapshot.ui.ai_language.as_deref(),
        config_snapshot.ui.language.as_str(),
    );
    // 技能快照与渐进披露（run 开始固定版本——重扫/启停只
    // 影响下一 run；未信任技能询问在本步同步完成，拒绝者不进清单）。
    let run_skills = ensure_skills_snapshot(app, &session_id).await;
    // 项目指令信任核验与注入：AGENTS.md 等
    // 首次加载弹 projectTrust 询问，deny/未信任不注入（本步同步完成，
    // 与技能信任同一装配时机）。
    let instructions =
        crate::knowledge::instructions::ensure_instruction_disclosure(app, &session_id).await;
    let disclosure = run_skills.disclosure.clone();
    let policy = compaction_policy(app, &model);
    // 摘要驱动（专用小模型未配则当前 run 模型——无会话覆盖时即
    // active model；与 run 模型同源解析，独立客户端实例）
    let summary_model_id = app.config.load()?.compaction.model_id.clone();
    let summary_resolved = match summary_model_id.as_deref() {
        Some(id) => resolve(app, Some(id))?,
        None => resolve(app, initial_model_id.as_deref())?,
    };
    let summary_driver = Arc::new(TurnDriver::new(
        summary_resolved.driver,
        RunSpec::new(),
        Vec::new(),
    ));
    // ③ 工具集（三处同源过滤）——静态段 + MCP 动态段（
    // 动态段快照随 run 装配固化，tools/list_changed 的热更新下一 run 生效）；
    // 预算检查含工具表
    let mut registry_specs = crate::tools::build_registry();
    registry_specs.extend(app.mcp.registry().snapshot());
    let tool_specs = crate::tools::filter_for_tier(registry_specs, run_config.tier);
    let tools = to_rig_tool_definitions(tool_specs)?;

    // ④ 同会话互斥预检（E_SESSION_RUNNING 快速失败；正式互斥仍由
    //    RunManager::start 兜底）→ 帧枢纽就位 → run 开始前压缩
    //    （触发点①：首个模型调用前完成摘要与落盘，新边界立即生效于本次 run
    //    全部请求；compactDone 帧经枢纽入环，重载后 attach 可重放）
    if runs.get(&session_id).is_some() {
        return Err(IpcError::session_running(format!(
            "session already has an active run: {session_id}"
        )));
    }
    let run_id = uuid::Uuid::new_v4().to_string();
    // user 条目幂等兜底提前到压缩流程最前：输入先落盘再谈压缩——
    // 压缩失败/E_CONTEXT_OVERFLOW 暂停时「输入已保留」在史册面成立
    persist_user_fallback(app, &session_id, &messages);
    // 当前生效模型槽：run 装配时置初值，每拍刷新
    // （模型切换时更新），EventfulSink 的 usage 帧落账读取——
    // run 内切模型按拍真实归属。子代理/压缩各有独立身份面。
    let current_model: ModelSlot =
        Arc::new(Mutex::new(Some((model.id.clone(), model.name.clone()))));
    // 会话累计恢复：进程内首次触达该会话时从明细文件灌入内存表——
    // 后续 tokens:update 全量校正事件的快照从首个增量起即完整。
    {
        let mut map = app
            .session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !map.contains_key(&session_id) {
            let restored = app.ledger.detail.load(&session_id).session_total;
            map.insert(session_id.clone(), restored);
        }
    }
    let hub = Arc::new(FrameHub::new(sink));
    let (boundary, estimated_after) = maybe_compact_before_run(
        app,
        &session_id,
        &run_id,
        &messages,
        &tools,
        &policy,
        &summary_driver,
        (&summary_resolved.id, &summary_resolved.name),
        &mut crate::agent::HubSink { hub: hub.clone() },
        custom_prompt.as_deref(),
    )
    .await?;
    // wire 硬预算复查（压缩后仍 > 窗口 ×0.9 → 暂停保留输入，不静默裁历史）
    if is_over_wire_budget(estimated_after, policy.context_window) {
        return Err(context_overflow(estimated_after, &policy));
    }
    // ⑤ wire 切分（压缩边界 + image 双形态）：cutEntryId 前跳过/
    //    后展开，摘要交前缀层；image part 按视觉策略映射（vision 多模态块 /
    //    非 vision 占位）
    let (history, prompt, compaction_summary) =
        bwire::split_prompt_visioned(&messages, boundary.as_ref(), &vision_policy)
            .map_err(|e| IpcError::invalid_arg(e.to_string()))?;
    // 系统前缀注入层（次序：customPrompt → 项目指令 → 压缩 summary →
    // 技能清单）。项目指令插入第二位（与
    // customPrompt 同类——用户/仓库侧给 AI 的常驻指引）。
    let history = bwire::with_system_prefix_layers(
        history,
        [
            custom_prompt.as_deref(),
            instructions.as_deref(),
            ai_language_layer.as_deref(),
            compaction_summary.as_deref(),
            disclosure.as_deref(),
        ],
    );

    // ⑥ 注册 run（同会话互斥 E_SESSION_RUNNING；帧枢纽随句柄挂载）+ run:start
    //    事件（手动 start_chat 与自动投递都发——前端
    //    以它区分「自己 adapter 发起的 run」与「后端自动投递的新 run」并接入
    //    观察态续流）
    let handle = runs.start(&session_id, run_id.clone(), now_ms(), hub)?;
    app.events.emit(
        keys::RUN_START,
        serde_json::json!({ "sessionId": session_id, "runId": run_id }),
    );

    // ⑦ spawn run 任务（帧生产 + 审批挂起恢复 + 取消 + 终态摘除）。
    //    thinking 起点参数：按会话显式档 > per-model
    //    reasoningEffort > 不传的优先级链翻译为 rig 请求参数。
    //    Anthropic 方言同源给出请求级 max_tokens（协议必填 + 预算校验的
    //    output cap，按 thinking 预算上调）；OpenAI 系维持不带 max_tokens。
    let initial_directive = effective_thinking_directive(initial_thinking, &model);
    let mut spec = RunSpec {
        max_turns: Some(run_config.max_turns as usize),
        options: crate::backend::turn::reasoning_directive_to_options(&initial_directive),
        ..RunSpec::new()
    };
    if let Some(cap) = crate::backend::turn::directive_max_tokens(
        &initial_directive,
        model.max_tokens,
        model.api_format,
    ) {
        spec.max_tokens = Some(cap);
    }
    // 驱动共享句柄：每拍请求刷新源（HostRequestRefresher）持有
    // 同一 Arc 热更新模型客户端与请求参数；suggest 后台小调用也经 Arc 复用
    // 终态时的最新模型。
    let driver = Arc::new(TurnDriver::new(model.driver, spec.clone(), tools.clone()));
    // 模型调用日志钩子：Info 级每次收口记 host/model/耗时/
    // usage 元数据；verbose（Debug 级）追加请求/响应原文——宿主实现内
    // 过图片摘要 + 密钥脱敏闸后落会话日志。
    driver.set_llm_logger(Arc::new(HostLlmLogger {
        session_log: app.session_log.clone(),
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        api_host: HostLlmLogger::host_of(&model.base_url),
        blob_dir: crate::storage::imgblob::imgblob_dir(app.store.root(), &session_id),
        current_model: current_model.clone(),
    }));
    // 每拍请求刷新源：运行中切换模型/thinking 在同一 run
    // 的下一次模型请求生效。起点快照 = ①步合并值；当前快照同起点。
    let refresher: Arc<dyn RequestRefresher> = Arc::new(HostRequestRefresher {
        app: app.share_handles(),
        session_id: session_id.clone(),
        driver: driver.clone(),
        initial: EffectiveRuntime {
            model_id: initial_model_id.clone(),
            thinking: initial_thinking,
        },
        current: Mutex::new(EffectiveRuntime {
            model_id: initial_model_id.clone(),
            thinking: initial_thinking,
        }),
        current_model: current_model.clone(),
        resolve: resolve.clone(),
    });
    let model_name = model.name;
    // 会话内记忆启用：命令类审批「始终允许（本会话）」选项
    // 的展示开关；恢复循环按应答登记会话免批桶（句柄提前克隆进 run 任务）。
    // 审批策略快照按 run 读取（save_config 后下一次 start_chat 应用）。
    let approval_config = app
        .config
        .load()
        .map(|c| c.approval.clone())
        .unwrap_or_default();
    let session_memory_enabled = approval_config.session_memory_enabled;
    // 执行器装配：会话 roots（含外部授权目录）——项目会话 = 项目主目录 +
    // 数据目录下 `workspace/` 子目录 + 会话已授权外部目录，临时会话两根同为
    // workspace 子目录——数据目录本体（config/sessions 等存储内部文件）退出
    // 模型文件工具可见面；收窄单一落点在 resolve_session_roots，这里只负责
    // 两根首次使用即建（canonicalize 前置）。宿主侧边车（spill/logs）仍挂
    // AppState.store 根，不经本 roots。
    let executor: Arc<dyn crate::backend::run_loop::CancellableExecutor> = Arc::new({
        let roots = crate::session_roots::resolve_session_roots(app, &session_id);
        let _ = std::fs::create_dir_all(&roots.project_root);
        let _ = std::fs::create_dir_all(&roots.workspace_root);
        ToolsetExecutor::new(
            crate::tools::ToolsContext::new(roots.project_root, roots.workspace_root)
                .with_extra_roots(roots.extra_dirs),
            run_config.tier,
        )
        // 同批无冲突调用并发上限（tools 组）与 batch_read
        // 单批文件数上限（tools.batchReadLimit）：按 run 装配时
        // 读取配置生效（save_config 后下一次 start_chat 应用）。
        .with_max_parallel(app.config.load()?.tools.max_parallel as usize)
        .with_batch_read_limit(app.config.load()?.tools.batch_read_limit)
        // read(kind="image") 的 vision 前置校验面：按本次
        // run 解析出的模型标记开启——主动读图与新附图同一拒绝口径
        .with_vision(model.vision)
        .with_env(ExecutorEnv {
            events: app.events.clone(),
            ask: handle.ask.clone(),
            plan: handle.plan.clone(),
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            data_root: app.store.root().to_path_buf(),
            // 问卷 ask 开关日志：经接缝落 kind=approval 行
            ask_log: Some(Arc::new(HostAskLogger {
                session_log: app.session_log.clone(),
            })),
            // 产物登记：主代理环境装配存储器——
            // write/edit 成功落盘后写 <sid>.artifacts.json（子代理环境不装配）
            store: Some(app.store.clone()),
            // MCP 动态工具调用面：归一名反查 + 结果未知语义在
            // mcp/pool 收口（engine 内直接装配——旧壳的 McpCallerAdapter
            // 只是跨 crate 的转发壳，随平移归一）。
            mcp: Some(Arc::new(McpCallerEngine {
                host: app.mcp.clone(),
            })),
            // run 内技能快照：`skill` 工具只读快照——正文
            // 全文与 description 同版本固定，reload/toggle 不影响活跃 run。
            skills: Some(Arc::new(run_skills.snapshot)),
            // 子代理委派面：主代理环境可委派；白名单不限
            // （allowed_tools=None = 主代理全表）。
            subagents: Some(Arc::new(SubagentSpawnerImpl {
                app: app.share_handles(),
                ctx: super::subagent::SubagentContext {
                    session_id: session_id.clone(),
                    parent_run_id: run_id.clone(),
                    parent_hub: handle.hub.clone(),
                    tier: run_config.tier,
                    model_id: run_config.model_id.clone(),
                    auto_confirm,
                    // 子代理命令类审批同主 run 展示
                    // 「始终允许（本会话）」，免批登记共用会话桶
                    session_memory_enabled,
                },
                resolve: resolve.clone(),
            })),
            allowed_tools: None,
            // 网络工具执行环境（web_fetch/http_request）：SSRF 策略
            // （network.allowPrivateNetwork）+ 代理 + 进程级节流器，按当前
            // AppConfig 装配（save_config 后下一次 start_chat 生效）。
            network: Some(Arc::new(crate::tools::NetworkEnv::from_config(
                app.config.load()?.as_ref(),
            ))),
        })
    });
    // 审批门装配 MCP 判定视图（动态许可核验 + 未声明
    // read_only 即便 full_access 也挂起）。围栏 AST 判定的工作区
    // 上下文（四象限）：roots 与项目主目录做 canonicalize
    // 后装配（词法与 canonical 同域比较的前提）；任一根 canonicalize
    // 失败则不装配——围栏退化为「有写目标即 Confirm」的保守面。
    // 装配面：内部状态禁区判定（数据目录根 + 项目主
    // 目录两根）、会话级免批桶（always_session，主 run 与子代理同桶）、
    // 审批策略快照（四开关 ①④ + 白名单条目，按 run 读取生效）。
    let gate = {
        let roots = crate::session_roots::resolve_session_roots(app, &session_id);
        let canonical = |p: &std::path::Path| std::fs::canonicalize(p).ok();
        let gate_ws = match (
            canonical(&roots.project_root),
            canonical(&roots.workspace_root),
        ) {
            (Some(project), Some(data)) => Some(crate::backend::gate::GateWorkspace {
                roots: {
                    // 全根 canonical（含授权目录——授权判定与围栏判定同域
                    // 比较）
                    let mut all = vec![project.clone(), data];
                    for extra in &roots.extra_dirs {
                        if let Some(c) = canonical(extra) {
                            all.push(c);
                        }
                    }
                    all
                },
                base: project.clone(),
            }),
            _ => None,
        };
        let approval = approval_config.clone();
        let mut gate = ApprovalGate::new(run_config.tier, handle.plan.clone())
            .with_mcp(Arc::new(McpGateEngine {
                host: app.mcp.clone(),
            }))
            .with_internal_guard(crate::safety::internal_state::InternalStateGuard::new(
                // 内部状态禁区判定：数据目录根 + 项目主目录两根
                // ——注意第二根是 workspace 子目录，禁区
                // 判定仍以数据目录根为界（config/sessions 等恒在禁区）。
                app.store.root(),
                &roots.project_root,
            ))
            // 外部目录授权：初始集合 = 会话已授权目录，
            // 恢复循环在用户允许后热追加；数据目录根做禁授权目标判定。
            .with_external_dirs(roots.extra_dirs.clone(), app.store.root().to_path_buf())
            .with_session_memory(app.approval_memory.bucket(&session_id))
            // 计划基线扩展桶（范围门 always_session）：主 run
            // 装配会话桶——「本会话始终允许」的授权目录跨 run 生效（进程内
            // 不持久化）。
            .with_plan_scope_memory(app.plan_scopes.bucket(&session_id))
            .with_policy(crate::backend::gate::ApprovalPolicy {
                enabled: approval.enabled,
                allowlist_enabled: approval.allowlist_enabled,
                command_allowlist: approval.command_allowlist.clone(),
                require_plan: approval.require_plan,
                git_allow_stage_commit: approval.git_allow_stage_commit,
            });
        if let Some(ws) = gate_ws {
            gate = gate.with_workspace(ws);
        }
        Arc::new(gate)
    };
    // 自动起名：user 消息落盘后判定——index 标题仍为
    // 默认 && 树内仅此一条 user 消息（改名/非首条/重启续聊三不触发，重启
    // 不重生成由判定天然保证）。命中 spawn 后台标题任务（零工具小调用，
    // 独立于 run 生命周期；失败静默）。放在 run 副本表创建
    // 之后 spawn——标题与 run/suggest 共用同一份会话累计快照，tokens:update
    // 全量校正在同一累计域内收敛（跨表双源会造成事件快照互相覆盖）。
    {
        let meta_title = app.store.index().get(&session_id).map(|m| m.title);
        let user_count = app
            .store
            .load_session_tree(&session_id)
            .map(|tree| {
                tree.messages
                    .iter()
                    .filter(|m| m.role == MessageRole::User)
                    .count()
            })
            .unwrap_or(0);
        let first_text = first_user_text(&messages);
        if title_should_trigger(meta_title.as_deref(), user_count) && !first_text.trim().is_empty()
        {
            // 模型 = 压缩摘要专用模型配置共用（已落 `compaction.
            // modelId` 则复用）；未配/解析失败回退会话
            // 激活模型（上面 ① 已解析成功的同源，起名不因摘要配置漂移跳过）。
            let title_resolved = app
                .config
                .load()
                .ok()
                .and_then(|c| c.compaction.model_id.clone())
                .and_then(|id| resolve(app, Some(&id)).ok())
                .or_else(|| resolve(app, initial_model_id.as_deref()).ok());
            if let Some(resolved) = title_resolved {
                let session_usage_snapshot = Arc::new(Mutex::new(
                    app.session_usage
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone(),
                ));
                spawn_title_task(
                    Arc::new(TurnDriver::new(resolved.driver, RunSpec::new(), Vec::new())),
                    app.store.clone(),
                    app.events.clone(),
                    session_usage_snapshot,
                    app.ledger.clone(),
                    session_id.clone(),
                    run_id.clone(),
                    first_text,
                    (resolved.id, resolved.name),
                    TITLE_TIMEOUT,
                );
            }
        }
    }
    let suggest_tail = context_tail(&messages);
    // 审计的用户目标：本次 run 的首条 user 输入文本（first_user_text），
    // 随 run 任务进闭包——审计提示词只带本 run 输入摘要，不带
    // 整段聊天历史。
    let audit_goal = first_user_text(&messages);
    // 模型停滞看门狗超时：按 run 装配驱动时
    // 读取生效（save_config 后下一次 start_chat 应用）。
    let stall_timeout =
        std::time::Duration::from_secs(u64::from(app.config.load()?.watchdog.stall_timeout_secs));
    // 项目主目录（范围门恢复循环）：always_session 授权时把
    // 计划外写目标的词法路径展开为绝对路径再归一（与范围门判定同款基准）。
    let project_root_for_scope =
        crate::session_roots::resolve_session_roots(app, &session_id).project_root;
    // steer 插话源：RunHandle 的插话队列 + 宿主副作用
    // （canonical 落盘 + run:inject 回执）收进 HostSteerSource，RunLoop 于
    // turn 边界经 trait 取文本
    let steer_source: Arc<dyn SteerSource> = Arc::new(HostSteerSource {
        queue: handle.steer.clone(),
        store: app.store.clone(),
        events: app.events.clone(),
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        seq: std::sync::atomic::AtomicU32::new(0),
    });
    // run 收尾的队列依赖面：全部字段同源的共享句柄副本（Arc
    // 克隆零深拷贝）——收尾后按终态分流 followUp 队列并自动投递（投递要
    // 以完整状态面重新走装配入口，故捕获句柄而非细粒度克隆件）
    let session_usage_for_task = Arc::new(Mutex::new(
        app.session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    ));
    let run_id_for_result = run_id.clone();
    // ⑨ spawn run 任务：依赖面收进 RunTaskDeps（spawn 闭包不能借用 app）
    tokio::spawn(drive_main_run(RunTaskDeps {
        app: app.share_handles(),
        runs: runs.clone(),
        session_id,
        run_id,
        handle,
        driver,
        model_name,
        spec,
        prompt,
        history,
        tools,
        gate,
        executor,
        steer: steer_source,
        refresher,
        stall_timeout,
        auto_confirm,
        session_memory_enabled,
        summary_driver,
        summary_model: (summary_resolved.id, summary_resolved.name),
        policy,
        audit_goal,
        suggest_tail,
        run_config_for_delivery: run_config,
        project_root_for_scope,
        session_usage: session_usage_for_task,
        current_model,
        resolve,
    }));

    Ok(super::StartChatResult {
        run_id: run_id_for_result,
    })
}

/// MCP 调用面（engine 内装配实现；旧壳的 McpCallerAdapter 是跨 crate
/// 转发壳，随平移归一——backend 依赖倒置接缝不变）。
struct McpCallerEngine {
    host: Arc<crate::mcp::pool::McpHost>,
}

impl crate::backend::exec::McpCaller for McpCallerEngine {
    fn call(
        &self,
        normalized_name: &str,
        args_json: &str,
    ) -> std::pin::Pin<Box<dyn Future<Output = crate::tools::ToolOutcome> + Send + '_>> {
        // Arc 克隆进 future（生命周期统一到 &self；入参字符串克隆避免
        // 多重 borrow 交叉）
        let host = self.host.clone();
        let normalized_name = normalized_name.to_owned();
        let args_json = args_json.to_owned();
        Box::pin(async move {
            crate::mcp::pool::call_tool(&host, &normalized_name, &args_json).await
        })
    }
}

/// MCP 判定视图（engine 内装配实现；接缝 `backend::gate::McpGateView`）。
struct McpGateEngine {
    host: Arc<crate::mcp::pool::McpHost>,
}

impl crate::backend::gate::McpGateView for McpGateEngine {
    fn view(&self, normalized_name: &str) -> Option<crate::backend::gate::McpToolView> {
        let identity = self.host.registry().identity(normalized_name)?;
        Some(crate::backend::gate::McpToolView {
            server_name: identity.server_name,
            server_fingerprint: identity.server_fingerprint,
            original_name: identity.original_name,
            schema_hash: identity.schema_hash,
            read_only: identity.read_only,
        })
    }

    fn license_hit(&self, view: &crate::backend::gate::McpToolView) -> bool {
        self.host.licenses.is_licensed(
            &view.server_fingerprint,
            &view.original_name,
            &view.schema_hash,
        )
    }
}
