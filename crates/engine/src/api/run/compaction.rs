//! 上下文压缩编排（平移自旧壳 `host/commands/chat.rs`）。
//!
//! 三个触发点（旧壳同款）：
//! - **触发点①** [`maybe_compact_before_run`]：run 开始前（ask 关闭前的
//!   装配点）——估算本次请求达阈值即先压缩，新边界立即生效于本次 run；
//! - **触发点②** [`maybe_compact_after_ask`]：ask 关闭后（run 任务内
//!   wait_approval 收口处）——落盘面压缩，失败不阻断恢复；
//! - **手动**：`compact_session` API（`super::run` 入口，不走阈值判定）。
//!
//! 摘要执行与原子落盘收在 [`execute_compaction`]；预算纯函数
//! （`should_auto_compact`/`is_over_wire_budget`/`estimate_request_tokens`）
//! 在 `backend::estimator`，本模块只做编排。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rig::completion::request::ToolDefinition;

use crate::app_state::AppState;
use crate::backend::compaction::{CompactionError, CompactionScope, compaction_entry_id, compression_scope, summarize};
use crate::backend::estimator::{
    estimate_entries_tokens, estimate_request_tokens, estimate_text_tokens,
    estimate_tool_tokens, is_over_wire_budget, should_auto_compact,
};
use crate::backend::frames::{ChatFrame, FrameSink, UsageTotal};
use crate::backend::run_loop::DriveState;
use crate::backend::turn::TurnDriver;
use crate::backend::wire::CompactionBoundary;
use crate::session_log::{LogKind, SessionLogger};
use crate::storage::MessageEntry;
use crate::util::errors::{E_CONTEXT_OVERFLOW, IpcError};
use crate::util::events::{EventSink, keys};

use super::model::{ResolvedModel, RunTransport, RunWire};
use super::sinks::{now_ms, record_aux_usage};

/// 压缩运行时策略快照（配置 + 模型窗口 + 回复预留）。
#[derive(Debug, Clone)]
pub(crate) struct CompactionRuntimePolicy {
    /// 自动压缩阈值（窗口占比，compaction.threshold）。
    pub threshold: f64,
    /// 压缩总超时（分块共用）。
    pub timeout: Duration,
    /// 当前模型上下文窗口（contextWindow）。
    pub context_window: u64,
    /// 回复预留（maxTokens，预算组成）。
    pub reply_reserve: u64,
    /// 摘要模型的上下文窗口（分块装箱面；专用小模型未配时 = 当前模型窗口）。
    pub summary_window: u64,
}

/// 从 AppConfig 读压缩策略（模型窗口取当前 run 模型；摘要专用模型未配时
/// 摘要窗口 = 当前模型窗口）。
pub(crate) fn compaction_policy<W: RunWire, T: RunTransport<W>>(
    app: &AppState,
    model: &ResolvedModel<W, T>,
) -> CompactionRuntimePolicy {
    let config = app
        .config
        .load()
        .map(|c| c.compaction.clone())
        .unwrap_or_default();
    // 摘要专用小模型未配置时跟随当前模型，窗口同取当前模型
    let summary_window = config
        .model_id
        .as_deref()
        .and_then(|id| {
            app.config.load().ok().and_then(|c| {
                c.providers
                    .iter()
                    .flat_map(|p| p.models.iter())
                    .find(|m| m.id == id)
                    .map(|m| u64::from(m.context_window))
            })
        })
        .filter(|w| *w > 0)
        .unwrap_or(model.context_window);
    CompactionRuntimePolicy {
        threshold: config.threshold,
        timeout: Duration::from_secs(config.timeout_secs),
        context_window: model.context_window,
        reply_reserve: model.max_tokens,
        summary_window,
    }
}

/// 工具表的估算输入面（name + description + schema 文本）。
pub(crate) fn tool_estimate_inputs(
    tools: &[Arc<ToolDefinition>],
) -> Vec<(String, String, String)> {
    tools
        .iter()
        .map(|t| {
            (
                t.name.to_string(),
                t.description.clone(),
                t.parameters.to_string(),
            )
        })
        .collect()
}

/// 压缩后的上下文占用估算（tokensAfter 口径）：裁剪后条目 + 摘要
/// 前缀 + 工具表 + 回复预留。
pub(crate) fn estimate_after_compaction(
    remaining_entries: &[MessageEntry],
    summary: &str,
    tools: &[(String, String, String)],
    reserve: u64,
) -> u64 {
    estimate_request_tokens(&[summary], remaining_entries, "", tools, reserve)
}

/// E_CONTEXT_OVERFLOW 错误构造（暂停并保留输入，提示重试
/// 压缩或开启新会话）。
pub(crate) fn context_overflow(estimated: u64, policy: &CompactionRuntimePolicy) -> IpcError {
    IpcError::new(
        E_CONTEXT_OVERFLOW,
        format!(
            "上下文占用约 {estimated} token，超过模型窗口（{}）的硬预算且压缩后仍放不下：\
             已暂停本次发送，输入已保留，可重试压缩或开启新会话",
            policy.context_window
        ),
    )
}

/// 把压缩摘要调用的实际用量计入会话累计（tokens:update 照发、
/// 不计 turnUsage——压缩不属主 run 轮次）。共享表 + 事件汇为细粒度参数
/// （run 任务的 spawn 闭包里没有 AppState 整体，只有这些克隆件）。
/// 已知实际用量同步落账（kind=aux，`{runId}:compact`；runs 计数
/// 不增，未知用量不伪造）。
#[allow(clippy::too_many_arguments)]
fn accumulate_compaction_usage(
    session_usage: &Mutex<HashMap<String, UsageTotal>>,
    events: &Arc<dyn EventSink>,
    ledger: &crate::usage::UsageLedger,
    session_id: &str,
    run_id: &str,
    summary_model: (&str, &str),
    usages: &[rig::completion::Usage],
) {
    let mut delta = UsageTotal::default();
    for usage in usages {
        delta.accumulate(&crate::backend::turn::usage_total(usage));
    }
    if delta == UsageTotal::default() {
        return; // 无计量（mock/未知用量不伪造）
    }
    record_aux_usage(
        events,
        session_usage,
        ledger,
        session_id,
        &format!("{run_id}:compact"),
        &(summary_model.0.to_owned(), summary_model.1.to_owned()),
        &delta,
    );
}

/// 压缩结果（提交面）：新压缩边界（tokensBefore/tokensAfter 由调用方按
/// 各自触发点的估算基准填入帧与事件载荷）。
#[derive(Debug, Clone)]
pub(crate) struct CompactionOutcome {
    pub boundary: CompactionBoundary,
}

/// 对会话执行一次压缩（核心编排）：
/// ① 摘要调用（增量输入 = 旧摘要 + 新增内容；分块共用总超时）；
/// ② `append_compaction` 原子落盘（含首次格式升级备份守卫——失败
///    暂停写入并报错，原数据不动）；③ 摘要用量计入会话累计。
/// 中途失败不提交条目、不推进边界（原子提交面），错误原样上抛由调用
/// 方发 run:compact_failed 与兜底判定。
/// 压缩生命周期日志（kind=compact 三事件：开始/完成/失败，
/// 「事件点复用」——落点即本函数的三个相位）；失败升 Warn。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_compaction<W: RunWire, T: RunTransport<W>>(
    store: &Arc<crate::storage::SessionStore>,
    events: &Arc<dyn EventSink>,
    session_usage: &Mutex<HashMap<String, UsageTotal>>,
    ledger: &crate::usage::UsageLedger,
    session_id: &str,
    owner: &str,
    summary_model: (&str, &str),
    scope: &CompactionScope,
    summary_driver: &TurnDriver<W, T>,
    policy: &CompactionRuntimePolicy,
    session_log: Option<&Arc<SessionLogger>>,
) -> Result<CompactionOutcome, CompactionError> {
    if let Some(logger) = session_log {
        logger.session_line(
            session_id,
            crate::providers::types::LogLevel::Info,
            LogKind::Compact,
            "上下文压缩开始".to_owned(),
            Some(serde_json::json!({ "entries": scope.entries.len() })),
            Some(owner),
            None,
            None,
        );
    }
    let result = execute_compaction_inner(
        store,
        events,
        session_usage,
        ledger,
        session_id,
        owner,
        summary_model,
        scope,
        summary_driver,
        policy,
    )
    .await;
    if let Some(logger) = session_log {
        match &result {
            Ok(_) => logger.session_line(
                session_id,
                crate::providers::types::LogLevel::Info,
                LogKind::Compact,
                "上下文压缩完成".to_owned(),
                Some(serde_json::json!({
                    "cutEntryId": scope.cut_entry_id,
                    "entries": scope.entries.len(),
                })),
                Some(owner),
                None,
                None,
            ),
            Err(e) => logger.session_line(
                session_id,
                crate::providers::types::LogLevel::Warn,
                LogKind::Compact,
                "上下文压缩失败".to_owned(),
                Some(serde_json::json!({ "error": e.to_string() })),
                Some(owner),
                None,
                None,
            ),
        }
    }
    result
}

/// 压缩编排本体（原 [`execute_compaction`] 主体；生命周期行在外层包装）。
#[allow(clippy::too_many_arguments)]
async fn execute_compaction_inner<W: RunWire, T: RunTransport<W>>(
    store: &Arc<crate::storage::SessionStore>,
    events: &Arc<dyn EventSink>,
    session_usage: &Mutex<HashMap<String, UsageTotal>>,
    ledger: &crate::usage::UsageLedger,
    session_id: &str,
    owner: &str,
    summary_model: (&str, &str),
    scope: &CompactionScope,
    summary_driver: &TurnDriver<W, T>,
    policy: &CompactionRuntimePolicy,
) -> Result<CompactionOutcome, CompactionError> {
    let (summary, usages) = summarize(
        summary_driver,
        scope.old_summary.as_deref(),
        &scope.entries,
        policy.summary_window,
        policy.timeout,
    )
    .await?;
    // 摘要已成功，进入提交面：备份守卫 + append 原子落盘（失败即整体失败，
    // 不推进边界——摘要结果被丢弃，下次重试重新摘要）
    let (_, count) = store
        .compaction_overview(session_id)
        .map_err(|e| CompactionError::Failed(e.to_string()))?;
    let entry = crate::storage::StoredCompactionEntry {
        id: compaction_entry_id(owner, count + 1),
        cut_entry_id: scope.cut_entry_id.clone(),
        summary: summary.clone(),
        tokens_before: estimate_entries_tokens(&scope.entries),
        created_at: now_ms(),
    };
    store
        .append_compaction(session_id, entry)
        .map_err(|e| CompactionError::Failed(e.to_string()))?;
    accumulate_compaction_usage(
        session_usage,
        events,
        ledger,
        session_id,
        owner,
        summary_model,
        &usages,
    );
    Ok(CompactionOutcome {
        boundary: CompactionBoundary {
            cut_entry_id: scope.cut_entry_id.clone(),
            summary,
        },
    })
}

/// 活动路径的边界裁剪切片（与 `backend::wire::split_prompt_with_boundary`
/// 同一裁剪口径的条目视图；边界锚点缺失按参数错误拒绝——不静默全量展开）。
pub(crate) fn boundary_ref_slice<'a>(
    entries: &'a [MessageEntry],
    boundary: Option<&CompactionBoundary>,
) -> Result<&'a [MessageEntry], IpcError> {
    match boundary {
        None => Ok(entries),
        Some(b) => {
            let idx = entries
                .iter()
                .position(|e| e.id == b.cut_entry_id)
                .ok_or_else(|| {
                    IpcError::invalid_arg(format!(
                        "压缩边界锚点 {} 不在会话活动路径上",
                        b.cut_entry_id
                    ))
                })?;
            Ok(&entries[idx + 1..])
        }
    }
}

/// run 开始前的自动压缩编排（ask 关闭前触发点）：
/// 1. 估算本次请求（系统前缀 + 历史边界裁剪 + prompt + 工具表 + 回复预留）；
/// 2. 未达阈值 → Ok((既有边界, tokens))，tokens 供 wire 预算复查；
/// 3. 达阈值 → run:compacting → 压缩编排 → compactDone 帧 + run:compacted；
/// 4. 压缩失败：原上下文能放入窗口（< 0.9）→ Ok((既有边界, …)) 继续；放不下 →
///    Err(E_CONTEXT_OVERFLOW)（输入已落盘保留，不静默裁历史，兜底）。
///
/// 压缩成功后的新边界立即生效于本次 run 的全部请求；
/// `summary_driver` 由调用方按配置解析（专用小模型或当前模型）。
/// `summary_model` = 摘要模型身份 `(配置 id, 显示名)`（落账归属）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn maybe_compact_before_run<W: RunWire, T: RunTransport<W>>(
    app: &AppState,
    session_id: &str,
    run_id: &str,
    messages: &[MessageEntry],
    tools: &[Arc<ToolDefinition>],
    policy: &CompactionRuntimePolicy,
    summary_driver: &TurnDriver<W, T>,
    summary_model: (&str, &str),
    sink: &mut dyn FrameSink,
    custom_prompt: Option<&str>,
) -> Result<(Option<CompactionBoundary>, u64), IpcError> {
    let overview = app
        .store
        .compaction_overview(session_id)
        .map_err(|e| IpcError::internal(e.to_string()))?;
    let boundary = overview
        .0
        .map(|c| CompactionBoundary {
            cut_entry_id: c.cut_entry_id,
            summary: c.summary,
        });
    // 估算面：活动路径按现有边界裁剪（与 wire 重建同一裁剪口径），
    // 再剔除末条 prompt（本次输入不参与压缩，触发点①口径）
    let whole_after_boundary = boundary_ref_slice(messages, boundary.as_ref())?;
    let body = &whole_after_boundary[..whole_after_boundary.len().saturating_sub(1)];
    let prompt_text = messages
        .last()
        .map(|m| {
            m.parts
                .iter()
                .filter_map(|p| match p {
                    crate::storage::WirePart::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<String>()
        })
        .unwrap_or_default();
    let tool_inputs = tool_estimate_inputs(tools);
    let prefixes: Vec<&str> = [custom_prompt, boundary.as_ref().map(|b| b.summary.as_str())]
        .into_iter()
        .flatten()
        .collect();
    let estimated = estimate_request_tokens(
        &prefixes,
        body,
        &prompt_text,
        &tool_inputs,
        policy.reply_reserve,
    );
    if !should_auto_compact(estimated, policy.context_window, policy.threshold) {
        return Ok((boundary, estimated));
    }

    // —— 达阈值：压缩编排（事件 → 摘要 → 落盘 → 帧/事件收口）——
    app.events.emit(
        keys::RUN_COMPACTING,
        serde_json::json!({ "sessionId": session_id }),
    );
    let scope = compression_scope(
        body,
        boundary
            .as_ref()
            .map(|b| (b.cut_entry_id.as_str(), b.summary.as_str())),
    );
    let executed = match scope {
        Some(scope) => {
            execute_compaction(
                &app.store,
                &app.events,
                &app.session_usage,
                &app.ledger,
                session_id,
                run_id,
                summary_model,
                &scope,
                summary_driver,
                policy,
                Some(&app.session_log),
            )
            .await
        }
        // 无可压内容但已达阈值（如首次请求即超大附件）：按超预算兜底
        None => Err(CompactionError::Failed(
            "已达压缩阈值但无可压缩的历史内容".to_owned(),
        )),
    };
    let outcome = match executed {
        Ok(outcome) => outcome,
        Err(_) => {
            app.events.emit(
                keys::RUN_COMPACT_FAILED,
                serde_json::json!({ "sessionId": session_id }),
            );
            // 失败兜底：原上下文能放入硬预算（窗口 ×0.9）则继续
            if !is_over_wire_budget(estimated, policy.context_window) {
                return Ok((boundary, estimated));
            }
            return Err(context_overflow(estimated, policy));
        }
    };
    // tokensAfter：新边界下的重算（压缩已在史册生效，重读概览取最新边界）
    let new_boundary = CompactionBoundary {
        cut_entry_id: outcome.boundary.cut_entry_id.clone(),
        summary: outcome.boundary.summary.clone(),
    };
    let new_body = boundary_ref_slice(messages, Some(&new_boundary))?;
    let tokens_after = estimate_after_compaction(
        new_body,
        &new_boundary.summary,
        &tool_inputs,
        policy.reply_reserve,
    );
    sink.send(ChatFrame::CompactDone {
        cut_entry_id: new_boundary.cut_entry_id.clone(),
        tokens_before: estimated,
        tokens_after,
    });
    app.events.emit(
        keys::RUN_COMPACTED,
        serde_json::json!({
            "sessionId": session_id,
            "tokensBefore": estimated,
            "tokensAfter": tokens_after,
        }),
    );
    Ok((Some(new_boundary), tokens_after))
}

/// ask 关闭后的压缩触发点（run 任务内 wait_approval 收口处，
/// 与落盘封口同一安全点）：**落盘面压缩**——摘要与 compaction 条目落盘后
/// 对恢复后的落盘条目与下一个 run 生效；当前 run 的 AgentRun 状态机
/// （rig 0.45）不支持中途替换内部 history，恢复后照常执行，恢复后请求超
/// 硬预算由请求面预算检查兜底终止（输入保留）。
///
/// 估算基准 = 最近请求真实输入（`DriveState.last_input`）+ 本次审批调用
/// 参数 + 工具表 + 回复预留。失败只发 run:compact_failed、不阻断恢复
/// （落盘面优化不承载硬预算义务——硬预算判定在恢复后的请求面）。
/// 存储器/事件汇/用量表为细粒度参数（spawn 闭包内无 AppState 整体）。
/// `ledger` 与 `summary_model`（摘要模型身份）随闭包传入（落账面）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn maybe_compact_after_ask<W: RunWire, T: RunTransport<W>>(
    store: &Arc<crate::storage::SessionStore>,
    events: &Arc<dyn EventSink>,
    session_usage: &Mutex<HashMap<String, UsageTotal>>,
    ledger: &crate::usage::UsageLedger,
    session_id: &str,
    run_id: &str,
    summary_model: (&str, &str),
    pending: &crate::backend::run_loop::PendingCall,
    state: &DriveState,
    tools: &[Arc<ToolDefinition>],
    policy: &CompactionRuntimePolicy,
    summary_driver: &TurnDriver<W, T>,
    session_log: Option<&Arc<SessionLogger>>,
) {
    let pending_tokens = estimate_text_tokens(&pending.args_json);
    let tool_tokens = estimate_tool_tokens(&tool_estimate_inputs(tools));
    let estimated = state.last_input + pending_tokens + tool_tokens + policy.reply_reserve;
    if !should_auto_compact(estimated, policy.context_window, policy.threshold) {
        return;
    }
    events.emit(
        keys::RUN_COMPACTING,
        serde_json::json!({ "sessionId": session_id }),
    );
    // 安全点语义：run 内落盘条目均已完成（turn 收口才落盘），边界不拆批次；
    // 压缩候选 = 史册全量（无"本次输入"豁免——ask 后场景没有独立 prompt）
    let compacted = async {
        let tree = store
            .load_session_tree(session_id)
            .map_err(|e| CompactionError::Failed(e.to_string()))?;
        let overview = store
            .compaction_overview(session_id)
            .map_err(|e| CompactionError::Failed(e.to_string()))?;
        let boundary = overview
            .0
            .map(|c| CompactionBoundary {
                cut_entry_id: c.cut_entry_id,
                summary: c.summary,
            });
        let scope = compression_scope(
            &tree.messages,
            boundary
                .as_ref()
                .map(|b| (b.cut_entry_id.as_str(), b.summary.as_str())),
        );
        let scope = scope
            .ok_or_else(|| CompactionError::Failed("无可压缩的历史内容".to_owned()))?;
        execute_compaction(
            store,
            events,
            session_usage,
            ledger,
            session_id,
            run_id,
            summary_model,
            &scope,
            summary_driver,
            policy,
            session_log,
        )
        .await
    }
    .await;
    match compacted {
        Ok(outcome) => {
            let tokens_after = store
                .load_session_tree(session_id)
                .ok()
                .and_then(|tree| {
                    let boundary = CompactionBoundary {
                        cut_entry_id: outcome.boundary.cut_entry_id.clone(),
                        summary: outcome.boundary.summary.clone(),
                    };
                    // 锚点必在史册（cut 取自史册条目）；防御失败按 0 折算
                    boundary_ref_slice(&tree.messages, Some(&boundary))
                        .ok()
                        .map(|rest| {
                            estimate_after_compaction(
                                rest,
                                &boundary.summary,
                                &[],
                                policy.reply_reserve,
                            )
                        })
                })
                .unwrap_or(0);
            events.emit(
                keys::RUN_COMPACTED,
                serde_json::json!({
                    "sessionId": session_id,
                    "tokensBefore": estimated,
                    "tokensAfter": tokens_after,
                }),
            );
        }
        Err(_) => {
            events.emit(
                keys::RUN_COMPACT_FAILED,
                serde_json::json!({ "sessionId": session_id }),
            );
        }
    }
}
