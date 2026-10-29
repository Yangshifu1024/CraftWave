//! 上下文压缩编排（技术设计文档 §6.2 压缩调度，FR-03-05，04 号票）。
//!
//! 名词解释：上下文压缩指把一段早期对话历史替换成一份摘要文本（summary），
//! 让长会话不因超出模型上下文窗口而报错；「压缩边界」是摘要覆盖到的最后
//! 一条条目（cutEntryId）——之后的内容正常发给模型，之前的以摘要取代。
//! 「增量摘要」指压缩输入 = 上次摘要 + 其边界之后的新增内容（首次压缩无
//! 旧摘要），而不是每次都从头摘要全部历史。
//!
//! 本模块承载纯逻辑与摘要调用（§6.2 字段级）：
//! - [`compression_scope`]：从活动路径与最近压缩边界算出本次待压缩范围；
//! - [`entry_to_block`] / [`plan_blocks`]：条目 → 摘要输入文本（Q38：超大
//!   已完成工具批次在此转为带工具名/callId/来源标记的普通文本，仅改摘要
//!   输入表示，原始记录与正式配对不动）+ 按摘要模型窗口贪心分块；
//! - [`summarize`]：分块顺序摘要 + 滚动合并，全部调用共用 compact_timeout
//!   总预算（Q31：不能按块重置超时）。
//!
//! 事件发射、落盘（append_compaction）与帧出口由 host 层（chat.rs）编排：
//! 失败时不提交新 compaction 条目、不推进边界（原子提交面在存储层）。

use std::time::{Duration, Instant};

use rig::completion::Usage;

use super::estimator::estimate_text_tokens;
use super::turn::TurnDriver;
use crate::storage::MessageEntry;

/// 摘要 prompt 骨架（§6.2 定稿要点）：输入 = 旧摘要 + 新增内容块；
/// 输出 = 结构化摘要正文。
fn summary_prompt(old_summary: Option<&str>, block: &str) -> String {
    let old = match old_summary {
        Some(s) if !s.trim().is_empty() => s.to_owned(),
        _ => "（首次压缩，无旧摘要）".to_owned(),
    };
    format!(
        "把以下对话内容压缩为一份结构化摘要，供后续对话作为唯一早期上下文使用。\
         摘要必须保留：用户的目标与约束、已做出的关键决定、重要文件路径/命令/事实、\
         未完成的任务与下一步。直接输出摘要正文，不要解释。\n\
         <上次摘要>\n{old}\n</上次摘要>\n\
         <新增对话>\n{block}\n</新增对话>"
    )
}

/// 压缩失败形态（§6.2 失败兜底：事件 run:compact_failed 的发起依据）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionError {
    /// 总超时耗尽（Q31：分块共用同一预算）。
    TimedOut,
    /// 摘要调用失败（网络/供应商/解析）。
    Failed(String),
}

impl std::fmt::Display for CompactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TimedOut => write!(f, "压缩总超时耗尽"),
            Self::Failed(m) => write!(f, "摘要调用失败：{m}"),
        }
    }
}

/// 压缩范围（§6.2 增量摘要协议的输入面）。
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionScope {
    /// 待压缩条目（上次边界之后、本次输入之前的全部；为空则无需压缩）。
    pub entries: Vec<MessageEntry>,
    /// 新压缩边界 = 待压缩条目中最后一条的 id。
    pub cut_entry_id: String,
    /// 上次有效摘要（首次压缩为 None）。
    pub old_summary: Option<String>,
}

/// 压缩范围计算（§6.2）：
/// - `body` = 参与压缩的条目全集（**调用方决定是否剔除本次输入 prompt**：
///   run 开始前场景剔除末条 user；ask 关闭后/手动压缩场景传全量——后者
///   没有"本次输入"豁免面）；
/// - `latest` = 史册最近 compaction 的（cutEntryId, summary）；找不到 cut
///   节点（候选集不含该锚点，异常形态）时保守从头压缩并保留旧摘要——
///   旧摘要覆盖的范围仍是历史的一部分，增量语义不破坏。
///
/// 范围为空（无新增内容）→ None（无需压缩）。
pub fn compression_scope(
    body: &[MessageEntry],
    latest: Option<(&str, &str)>,
) -> Option<CompactionScope> {
    let start = match latest {
        Some((cut_id, _)) => body
            .iter()
            .position(|e| e.id == cut_id)
            .map(|idx| idx + 1)
            .unwrap_or(0), // 锚点不在路径上：从头压缩（保守增量）
        None => 0,
    };
    let scope_entries: Vec<MessageEntry> = body[start..].to_vec();
    let last = scope_entries.last()?;
    Some(CompactionScope {
        cut_entry_id: last.id.clone(),
        entries: scope_entries,
        old_summary: latest.map(|(_, summary)| summary.to_owned()),
    })
}

/// 条目 → 摘要输入文本块（Q38：工具调用与结果转为带来源标记的普通文本；
/// 原始条目不动，只改摘要输入的表示）。
pub fn entry_to_block(entry: &MessageEntry) -> String {
    let mut lines = vec![format!("[{} {}]", entry.role.as_str(), entry.id)];
    for part in &entry.parts {
        match part {
            crate::storage::WirePart::Text { text } => lines.push(text.clone()),
            crate::storage::WirePart::Reasoning { text } => {
                lines.push(format!("[thinking] {text}"));
            }
            crate::storage::WirePart::ToolCall {
                call_id,
                name,
                args_json,
            } => {
                lines.push(format!(
                    "[tool name={name} callId={call_id}] args={args_json}"
                ));
            }
            crate::storage::WirePart::ToolResult {
                call_id,
                ok,
                summary,
            } => {
                let mark = if *ok { "result ok" } else { "result err" };
                lines.push(format!("[{mark} callId={call_id}] {summary}"));
            }
            // 计划快照与 run 边界标记不进摘要输入（wire 重建同样跳过，§6.2）
            crate::storage::WirePart::PlanSnapshot { .. }
            | crate::storage::WirePart::RunMarker { .. } => {}
        }
    }
    lines.join("\n")
}

/// 按摘要模型窗口把文本块贪心装箱（§6.2/Q31）：块序列依次装进同一批直到
/// 超窗；单块自身超窗时按字符再切（Q38 超大批次兜底——切块纯为摘要输入
/// 表示，切缝不承担语义）。至少产出一批。
pub fn plan_blocks(blocks: Vec<String>, window_tokens: u64) -> Vec<String> {
    if blocks.is_empty() {
        return Vec::new();
    }
    let window = window_tokens.max(1);
    let mut batches: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_tokens = 0u64;
    for block in blocks {
        let block_tokens = estimate_text_tokens(&block).max(1);
        if block_tokens > window {
            // 单块超窗：先把当前批收口，再把块按字符比例切段装箱
            if !current.is_empty() {
                batches.push(std::mem::take(&mut current));
                current_tokens = 0;
            }
            let total_chars = block.chars().count() as u64;
            let parts = block_tokens.div_ceil(window);
            let per_part = total_chars.div_ceil(parts).max(1);
            let mut chunk = String::new();
            let mut count = 0u64;
            for ch in block.chars() {
                chunk.push(ch);
                count += 1;
                if count >= per_part {
                    batches.push(std::mem::take(&mut chunk));
                    count = 0;
                }
            }
            if !chunk.is_empty() {
                batches.push(chunk);
            }
            continue;
        }
        if current_tokens + block_tokens > window && !current.is_empty() {
            batches.push(std::mem::take(&mut current));
            current_tokens = 0;
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(&block);
        current_tokens += block_tokens;
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// 一次摘要调用的产出。
pub struct SummaryCall {
    /// 摘要文本（trim 后）。
    pub text: String,
    /// 本次调用的用量（rig Usage，调用方折算累计进会话）。
    pub usage: Usage,
}

/// 分块顺序摘要 + 滚动合并（§6.2/Q31）：每块一次 `complete_once` 非流式
/// 调用，输入 = 上次摘要（或旧摘要/首次占位）+ 当前块；全部调用共用
/// `timeout` 总预算——**任一块开始前检查剩余时间，耗尽即 TimedOut**，
/// 不能按块重置。返回最终摘要与全部调用的用量列表（供调用方计入会话）。
pub async fn summarize(
    driver: &TurnDriver,
    old_summary: Option<&str>,
    entries: &[MessageEntry],
    summary_window_tokens: u64,
    timeout: Duration,
) -> Result<(String, Vec<Usage>), CompactionError> {
    let deadline = Instant::now() + timeout;
    let blocks = plan_blocks(
        entries.iter().map(entry_to_block).collect(),
        summary_window_tokens,
    );
    if blocks.is_empty() {
        return Err(CompactionError::Failed("压缩范围为空".to_owned()));
    }
    let mut running_summary: Option<String> = old_summary
        .map(str::to_owned)
        .filter(|s| !s.trim().is_empty());
    let mut usages = Vec::new();
    for block in &blocks {
        if Instant::now() >= deadline {
            return Err(CompactionError::TimedOut);
        }
        let prompt = summary_prompt(running_summary.as_deref(), block);
        let remaining = deadline.saturating_duration_since(Instant::now());
        // 单次调用以剩余总预算为上限（tokio::time::timeout 兜底网络悬挂），
        // 超时即整体失败——共用预算的正确实现面。
        let call = tokio::time::timeout(remaining, driver.complete_once(&prompt))
            .await
            .map_err(|_| CompactionError::TimedOut)?
            .map_err(|e| CompactionError::Failed(e.to_string()))?;
        let (text, usage) = call;
        usages.push(usage);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(CompactionError::Failed("摘要返回空文本".to_owned()));
        }
        running_summary = Some(trimmed.to_owned());
    }
    Ok((running_summary.unwrap_or_default(), usages))
}

/// 压缩条目 id（§6.1 规则 `{owner}:compact:{n}`；n 跨 owner 连续递增）。
pub fn compaction_entry_id(owner: &str, n: usize) -> String {
    format!("{owner}:compact:{n}")
}

#[cfg(test)]
#[path = "compaction_test.rs"]
mod compaction_test;
