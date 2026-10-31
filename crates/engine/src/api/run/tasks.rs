//! run 任务的辅助装配域（平移自旧壳 `host/commands/chat.rs`）：
//!
//! - [`HostRequestRefresher`]：每拍模型请求前的会话偏好刷新源（运行中
//!   切模型/thinking 在同一 run 的下一次请求生效）；
//! - [`HostSteerSource`]：steer 插话消费源（出队 + canonical 落盘 +
//!   run:inject 回执，副作用收在实现内，RunLoop 只见纯文本）；
//! - [`spawn_suggest`]：run Done 后的后台建议小调用（失败静默）；
//! - 标题后台任务：自动起名判定（[`title_should_trigger`]）与
//!   [`spawn_title_task`]（双复判 + 兜底 + 双写落盘）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;


use crate::agent::RunHandle;
use crate::app_state::AppState;
use crate::backend::frames::UsageTotal;
use crate::backend::run_loop::{DriveState, RefreshFailure, RequestRefresher, SteerSource};
use crate::backend::turn::{TurnDriver, usage_total};
use crate::storage::{MessageEntry, MessageRole, StoredEntry, WirePart};
use crate::util::events::{EventSink, keys};

use super::model::{ModelResolver, ModelSlot, ResolvedModel, RunTransport, RunWire};
use super::sinks::{now_ms, record_aux_usage};

/// 每拍请求刷新的当前生效快照（对比面：判断本拍是否需要换模型/换参数）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct EffectiveRuntime {
    /// 生效的模型配置 id（ProviderModel.id）。
    pub(crate) model_id: Option<String>,
    /// 生效的会话 thinking 档（None = 未显式设置，按 per-model 回落）。
    pub(crate) thinking: Option<crate::providers::reasoning::ThinkingLevel>,
}

/// 每拍模型请求前的偏好刷新源：
/// 重读会话 prefs 文件（`<sid>.prefs.json`），模型或 thinking 变了就热更新
/// TurnDriver（换客户端 / 换请求参数 / 换图片降级开关），切到窗口更小的
/// 模型时先做预算重查（放不下即 E_CONTEXT_OVERFLOW 终止，暂停保留
/// 输入）。副作用全部收在本实现——backend 层的 `RequestRefresher` 契约
/// 只见判定结果。
///
/// 生效值 = prefs 文件值 > run 起点快照（RunConfig，合并了 start_chat 时的
/// prefs 初值）> 不设。正在进行的请求不被打断（驱动只在拍间读本实现）。
pub(crate) struct HostRequestRefresher<W: RunWire, T: RunTransport<W>> {
    /// 共享句柄副本（config/store 读取面；spawn 闭包不能借用 AppState，
    /// 与 followUp 投递同款 [`AppState::share_handles`]）。
    pub(crate) app: AppState,
    pub(crate) session_id: String,
    /// 目标驱动的共享句柄（换模型/换参数的写入口）。
    pub(crate) driver: Arc<TurnDriver<W, T>>,
    /// run 起点快照（RunConfig + start_chat 时的 prefs 初值合并）。
    pub(crate) initial: EffectiveRuntime,
    /// 当前生效快照（与上一次刷新后的驱动状态一致）。
    pub(crate) current: Mutex<EffectiveRuntime>,
    /// 当前生效模型槽 `(配置 id, 显示名)`（与 EventfulSink 共享——
    /// 每拍刷新时更新，usage 帧落账读到的即本拍实际执行模型）。
    pub(crate) current_model: ModelSlot,
    /// 模型解析接缝（切模型时按配置行重解析——生产方言 / 测试剧本同源）。
    pub(crate) resolve: ModelResolver<W, T>,
}

impl<W: RunWire, T: RunTransport<W>> RequestRefresher for HostRequestRefresher<W, T> {
    fn before_request(&self, state: &DriveState) -> Result<(), RefreshFailure> {
        let prefs = crate::storage::prefs::load_prefs(self.app.store.root(), &self.session_id);
        let wanted = EffectiveRuntime {
            model_id: prefs.model_id.or_else(|| self.initial.model_id.clone()),
            thinking: prefs.thinking.or(self.initial.thinking),
        };
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *current == wanted {
            return Ok(());
        }
        // —— 模型变化：重新解析 + 预算重查 + 换客户端 ——
        if current.model_id != wanted.model_id {
            let resolved: ResolvedModel<W, T> = (self.resolve)(&self.app, wanted.model_id.as_deref())
                .map_err(|e| RefreshFailure::Failed(e.message))?;
            // 预算接缝：切换后的模型窗口须装得下最近请求的真实
            // 输入 + 新模型的回复预留；last_input=0（尚无 usage，首拍前）
            // 也不跳过——按 0 + 预留做下限检查（run 开始前已有完整编排，
            // 该场景仅剩能力面切换）。
            let estimated = state.last_input.saturating_add(resolved.max_tokens);
            if crate::backend::estimator::is_over_wire_budget(estimated, resolved.context_window) {
                return Err(RefreshFailure::ContextOverflow {
                    estimated,
                    window: resolved.context_window,
                });
            }
            self.driver.swap_model(resolved.driver);
            // 运行中切非 vision：历史图片在请求面降级为占位，
            // 允许纯文字继续；切回 vision 模型自动恢复原图。
            self.driver.set_downgrade_images(!resolved.vision);
            // 模型槽同步（下一拍 usage 帧落账归属新模型）
            *self
                .current_model
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some((resolved.id.clone(), resolved.name.clone()));
        }
        // —— thinking 生效值重算（模型切换与档位变化都刷新；回落链以
        // 当前生效模型配置为准）——
        let model_id = wanted.model_id.clone().or(current.model_id.clone());
        // 指令与请求级 max_tokens 同源重算：Anthropic 方言恒带
        // 请求级 max_tokens（协议必填；有 thinking 预算时按 上调
        // 规则），OpenAI 系维持 None（请求不带 max_tokens，不回归）
        let (options, max_tokens) = match model_id.as_deref() {
            Some(id) => {
                let resolved: ResolvedModel<W, T> = (self.resolve)(&self.app, Some(id))
                    .map_err(|e| RefreshFailure::Failed(e.message))?;
                let directive =
                    super::model::effective_thinking_directive(wanted.thinking, &resolved);
                (
                    crate::backend::turn::reasoning_directive_to_options(&directive),
                    crate::backend::turn::directive_max_tokens(
                        &directive,
                        resolved.max_tokens,
                        resolved.api_format,
                    ),
                )
            }
            // 无模型可解析（防御面）：不传参数
            None => {
                let directive = crate::providers::reasoning::ReasoningDirective::none();
                (
                    crate::backend::turn::reasoning_directive_to_options(&directive),
                    None,
                )
            }
        };
        self.driver.update_spec(|spec| {
            spec.options = options;
            spec.max_tokens = max_tokens;
        });
        *current = wanted;
        Ok(())
    }
}

/// steer 消费的宿主实现：出队 → canonical 落盘
/// （id=`{runId}:steer:{n}`，父=消费时刻链尾）→
/// `run:inject` 回执（acceptedAtTurn + entryId/text，前端据以本地插入插话
/// 节点）。副作用收在本实现内，RunLoop（backend 层）只拿到注入文本。
pub(crate) struct HostSteerSource {
    /// 该 run 的插话队列（RunHandle.steer 的共享句柄）。
    pub(crate) queue: Arc<Mutex<std::collections::VecDeque<String>>>,
    pub(crate) store: Arc<crate::storage::SessionStore>,
    pub(crate) events: Arc<dyn EventSink>,
    pub(crate) session_id: String,
    pub(crate) run_id: String,
    /// 条目序号（id 的 `{n}` 段，run 内从 1 递增）。
    pub(crate) seq: std::sync::atomic::AtomicU32,
}

impl SteerSource for HostSteerSource {
    fn take(&self, turn: u32) -> Option<String> {
        let text = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()?;
        let n = self.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let entry_id = format!("{}:steer:{n}", self.run_id);
        // 父 = 消费时刻 canonical 链尾（steer 条目排在最新已落盘条目之后——
        // turn 收口的 assistant 条目已先于此落盘，时机次序天然成立）
        let parent_id = self
            .store
            .load_session_tree(&self.session_id)
            .ok()
            .and_then(|tree| tree.active_leaf_id);
        let entry = MessageEntry {
            id: entry_id.clone(),
            parent_id,
            role: MessageRole::User,
            parts: vec![WirePart::Text { text: text.clone() }],
            created_at: now_ms(),
            run_id: Some(self.run_id.clone()),
            status: None,
        };
        // 落盘失败不阻断注入（帧面可用性优先，取舍同 RunRecorder——
        // 缺行在崩溃恢复扫描中补 interrupted 标记，不会产生半行）
        if let Err(e) = self
            .store
            .append_canonical(&self.session_id, &[StoredEntry::Message(entry)])
        {
            #[cfg(debug_assertions)]
            eprintln!(
                "[craftwave] steer 条目落盘失败（{} {}）：{e:?}",
                self.session_id, entry_id
            );
            #[cfg(not(debug_assertions))]
            let _ = e;
        }
        self.events.emit(
            keys::RUN_INJECT,
            serde_json::json!({
                "sessionId": self.session_id,
                "runId": self.run_id,
                "acceptedAtTurn": turn,
                "entryId": entry_id,
                "text": text,
            }),
        );
        Some(text)
    }
}

/// 取走并清空某会话活跃 run 的 steer 未消费残余（run 收尾分流用：
/// Done → 转会话 followUp 队列尾；取消/失败 → 随 run:queue.returned 交还）。
pub(crate) fn drain_steer(handle: &Arc<RunHandle>) -> Vec<String> {
    handle
        .steer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .drain(..)
        .collect()
}

// ---- suggest 后台小调用 ----

/// suggest 后台小调用：一次非流式补全 → run:suggestions；失败静默、
/// 空文本静默；用量计入会话累计（tokens:update，不计 turnUsage——run 已终态）。
/// 驱动经共享句柄传入（run 任务收尾时把 Arc 交给本任务，suggest
/// 以 run 终态时的最新模型客户端执行）。已知实际用量同步落账
/// （日/模型聚合 + 明细 kind=aux + 缓存写穿；runs 计数不增）——失败
/// /无计量的未知用量不伪造，已返回的用量不因辅助身份被丢弃。
/// `final_model` = run 终态时的模型槽快照 `(配置 id, 显示名)`。
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_suggest<W: RunWire, T: RunTransport<W>>(
    driver: Arc<TurnDriver<W, T>>,
    events: Arc<dyn EventSink>,
    session_usage: Arc<Mutex<HashMap<String, UsageTotal>>>,
    ledger: crate::usage::UsageLedger,
    session_id: String,
    run_id: String,
    context_tail_text: String,
    final_model: (String, String),
) {
    tokio::spawn(async move {
        if context_tail_text.trim().is_empty() {
            return;
        }
        let prompt = format!(
            "根据下面这段对话结尾，生成一句简短的中文后续输入建议（ Composer 预填文案，\
             10–20 字，直接输出建议本身，不要引号和解释）：\n{context_tail_text}"
        );
        let Ok((text, usage)) = driver.complete_once(&prompt).await else {
            return; // 失败静默放弃（不重试、不提示）
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        // 用量计入会话累计（帧通道已收，走事件通道补计；辅助调用落账
        // 已返回 usage 必记账，runs 计数不增——记账面与审计共用 record_aux_usage）
        let delta = usage_total(&usage);
        record_aux_usage(
            &events,
            &session_usage,
            &ledger,
            &session_id,
            &format!("{run_id}:suggest"),
            &final_model,
            &delta,
        );
        events.emit(
            keys::RUN_SUGGESTIONS,
            serde_json::json!({ "sessionId": session_id, "runId": run_id, "text": trimmed }),
        );
    });
}

// ---- 自动起名（后台零工具小调用，suggest 同模式）----

/// 标题生成的调用超时（口径 10s；超时 → 首条消息截断兜底，不重试不提示）。
pub(crate) const TITLE_TIMEOUT: Duration = Duration::from_secs(10);

/// 标题生成调用的回复上限（口径 max_tokens 32——防模型失控长输出）。
pub(crate) const TITLE_MAX_TOKENS: u64 = 32;

/// 标题最大字符数（超时兜底截断 ≤30 字符口径；生成结果同限归一，
/// 模型无视 ≤20 字要求时兜住长度）。
pub(crate) const TITLE_MAX_CHARS: usize = 30;

/// 进入 prompt 的首条消息文本上限（口径：超长截 2000 字符）。
const TITLE_PROMPT_INPUT_CHARS: usize = 2000;

/// 触发判定（三态表）：仅当 index 标题仍是 create_session 默认值且
/// 树内恰好只有这一条 user 消息时自动起名。三个不触发面由此一并覆盖——
/// 用户改名过（title 非默认）、非首条消息（user 计数 > 1，含首条后编辑
/// 重发产生的兄弟分支节点）、重启后继续旧会话（重启不重生成由判定天然
/// 保证，无需额外标记位）。
pub(crate) fn title_should_trigger(meta_title: Option<&str>, user_message_count: usize) -> bool {
    meta_title == Some(crate::api::session::DEFAULT_SESSION_TITLE) && user_message_count == 1
}

/// 标题归一（生成结果与兜底共用）：trim → 取第一个非空行 → 超
/// [`TITLE_MAX_CHARS`] 字符截断（按字符取，多字节安全）→ 再 trim。
/// 兜底输入 = 首条消息全文（`lines()` 天然完成"首行截断 + 换行/前后
/// 空白归一"）。
pub(crate) fn normalize_title(raw: &str) -> String {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let truncated: String = line.chars().take(TITLE_MAX_CHARS).collect();
    truncated.trim().to_owned()
}

/// 标题生成 prompt：输入 = 首条 user
/// 消息文本，超长截 [`TITLE_PROMPT_INPUT_CHARS`] 字符；要求输出 ≤20 字
/// 标题、无引号无前缀。
pub(crate) fn title_prompt(first_user_text: &str) -> String {
    let clipped: String = first_user_text
        .chars()
        .take(TITLE_PROMPT_INPUT_CHARS)
        .collect();
    format!(
        "根据用户的这条消息，给这个 AI 助手会话起一个简短标题。要求：不超过 20 个字；\
         直接输出标题本身，不要引号、不要任何前缀或解释。消息内容：{clipped}"
    )
}

/// 首条 user 消息的正文文本（Text parts 顺序拼接；纯图片等无正文场景
/// 返回空串，调用方据此放弃起名）。
pub(crate) fn first_user_text(messages: &[MessageEntry]) -> String {
    messages
        .iter()
        .find(|m| m.role == MessageRole::User)
        .map(|m| {
            m.parts
                .iter()
                .filter_map(|p| match p {
                    WirePart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// 会话消息尾文本（suggest 上下文）：最后一条 assistant 的文本（缺省取
/// 最后一条 user）。
pub(crate) fn context_tail(messages: &[MessageEntry]) -> String {
    let text_of = |entry: &MessageEntry| {
        entry
            .parts
            .iter()
            .filter_map(|p| match p {
                WirePart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>()
    };
    messages
        .iter()
        .rev()
        .find(|m| m.role == MessageRole::Assistant)
        .map(text_of)
        .filter(|t| !t.trim().is_empty())
        .or_else(|| messages.last().map(text_of))
        .unwrap_or_default()
}

/// 后台标题任务（spawn 模式复制 [`spawn_suggest`] 先例
/// ——run 落盘链外独立任务、失败静默、用量计入会话）：
///
/// ① 锁内复判一（并发防护）：进入任务即查 index——标题已非默认
///    （用户抢先改名 / 并发任务已写）直接放弃；
/// ② 生成：`TurnDriver::complete_once_capped`（零工具、max_tokens 32）+
///    10s 超时；成功 → 归一后采用；超时/失败/空文本 → 兜底 = 首条消息
///    首行截断。成功调用已返回的 usage 照常记账（
///    已知用量不因后续路径丢弃；无 usage 不伪造，不能承诺失败免费）——
///    kind=aux、runId=`{runId}:title`、runs 计数不增，
///    tokens:update 照发（按当时本地日期落账，由 ledger 承担）；
/// ③ 锁内复判二（先写者为准）：落盘前再查一次 index——已被并发任务写入
///    则放弃（极端窗口下后写者可能覆盖，同值无害）；
/// ④ 落盘双写：meta 行（会话锁，`update_session_title`）→ index mutate
///    （title + updatedAt；锁序 session → index）；
/// ⑤ `session:title` 事件——严格在落盘之后发。
///
/// 全程失败静默：不报错、不提示、不重试；落盘失败不发事件
/// （事件承诺"已落盘"）。重启恢复遵守四口径记录（明细面已落账）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_title_task<W: RunWire, T: RunTransport<W>>(
    driver: Arc<TurnDriver<W, T>>,
    store: Arc<crate::storage::SessionStore>,
    events: Arc<dyn EventSink>,
    session_usage: Arc<Mutex<HashMap<String, UsageTotal>>>,
    ledger: crate::usage::UsageLedger,
    session_id: String,
    run_id: String,
    first_user_text: String,
    model: (String, String),
    timeout: Duration,
) {
    tokio::spawn(async move {
        let default_title = crate::api::session::DEFAULT_SESSION_TITLE;
        // index 标题仍为默认值的原子复判（SessionIndex 内部互斥保证读一致）
        let still_default = || {
            store
                .index()
                .get(&session_id)
                .map(|m| m.title == default_title)
                .unwrap_or(false)
        };
        // ① 复判一：已非默认（改名 / 并发已写）→ 放弃
        if !still_default() {
            return;
        }
        // ② 生成（超时兜底；usage 记账与标题取舍两段独立——已知用量不丢）
        let prompt = title_prompt(&first_user_text);
        let title = match tokio::time::timeout(
            timeout,
            driver.complete_once_capped(&prompt, TITLE_MAX_TOKENS),
        )
        .await
        {
            Ok(Ok((text, usage))) => {
                let delta = usage_total(&usage);
                if delta != UsageTotal::default() {
                    ledger.record(
                        &session_id,
                        &format!("{run_id}:title"),
                        crate::usage::run_kind::AUX,
                        &model.0,
                        &model.1,
                        delta,
                        false,
                    );
                    let total = {
                        let mut map = session_usage
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let total = map.entry(session_id.clone()).or_default();
                        total.accumulate(&delta);
                        *total
                    };
                    let mut payload =
                        serde_json::json!({ "sessionId": session_id, "usage": total });
                    if !model.0.is_empty() {
                        payload["modelId"] = serde_json::json!(model.0);
                    }
                    events.emit(keys::TOKENS_UPDATE, payload);
                }
                let generated = normalize_title(&text);
                if generated.is_empty() {
                    normalize_title(&first_user_text) // 空回复视为失败走兜底
                } else {
                    generated
                }
            }
            // 超时 / 调用失败：无 usage 不伪造，标题走静默兜底
            _ => normalize_title(&first_user_text),
        };
        if title.is_empty() {
            return; // 无标题可落（首条消息全空白等极端面），静默放弃
        }
        // ③ 复判二：先写者为准
        if !still_default() {
            return;
        }
        // ④ 落盘双写（meta 行先行——事件承诺"已落盘"；index 失败仅记日志，
        //    下次 canonical 落盘 upsert 时以 prior 补齐，尽力而为面）
        if store.update_session_title(&session_id, &title).is_err() {
            return;
        }
        if let Err(e) = store.index().mutate(&session_id, |m| {
            m.title = title.clone();
            m.updated_at = now_ms();
        }) {
            #[cfg(debug_assertions)]
            eprintln!("[craftwave] 标题 index 更新失败（{session_id}）：{e:?}");
            #[cfg(not(debug_assertions))]
            let _ = e;
        }
        // ⑤ 事件（严格在落盘之后）
        events.emit(
            keys::SESSION_TITLE,
            serde_json::json!({ "sessionId": session_id, "title": title }),
        );
    });
}
