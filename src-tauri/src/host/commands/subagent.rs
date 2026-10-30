//! 子代理委派的宿主装配与命令（§5.9，FR-07 域，10 号票）。
//!
//! 本文件承担三件事：
//! - **委派执行**（[`SubagentSpawnerImpl`]，注入 [`crate::backend::exec`]
//!   的 `SubagentSpawner` 接口）：查角色表 → 并发闸拿名额（满额排队）→
//!   写入角色在 Git 项目创建独立 worktree（ADR-0001）→ 发 subStarted 帧
//!   与 sub:spawn 事件 → 驱动子 run（复用 [`drive_run`]，含审批挂起恢复
//!   循环与取消级联）→ 四象限收尾（subReport/subDone 帧 + sub:* 事件 +
//!   worktree 差量回收/保留）→ `<report>` 汇报回父工具结果；
//! - **子代理历史**（`histories/subs/<parentRunId>/<subId>/`，§7.7）：
//!   meta.json + history.jsonl（最小子集；JSONL 逐行追加，读取忽略不完整
//!   尾行——与主会话 SegmentLog 同款追加语义），`load_subagent_history`
//!   命令转换为 WireMessage（MessageEntry）供抽屉回看；
//! - **命令面**：`stop_subagent`（#58，级联取消）与
//!   `load_subagent_history`（#59）。
//!
//! 公共操作归主代理（2026-10-29 用户决策）：子代理的工具面 = 角色白名单
//! （执行入口在 executor 检查，`allowed_tools`），写入子代理的文件根指向
//! worktree（写边界天然隔离），MCP 动态段不装配——command/MCP 入口不能
//! 绕过；数据库/外部服务等共享资源操作由模型在汇报中交回主代理协调
//! （worktree 不提供此类资源隔离，ADR-0001）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rig::completion::Message;
use rig::run::spec::RunSpec;
use serde::{Deserialize, Serialize};
use tauri::State;
use tokio_util::sync::CancellationToken;

use crate::agent::sub_registry::SubHandle;
use crate::agent::worktree::{self as wt, RecoveryReport, WorktreeLease};
use crate::agent::{SubagentRole, WorkspaceKind, clamp_budget_steps, find_role, roles};
use crate::backend::exec::{ExecutorEnv, SubagentSpawner, ToolsetExecutor};
use crate::backend::frames::{ChatFrame, EndReason, FrameError, FrameSink, SubOutcome, UsageTotal};
use crate::backend::gate::ApprovalGate;
use crate::backend::run_loop::{CancellableExecutor, DriveState, RunExit, drive_run};
use crate::backend::turn::TurnDriver;
use crate::host::AppState;
use crate::host::commands::chat::{
    ApprovalResolution, denied_finish, resolve_model, wait_approval_with,
};
use crate::host::commands::workspace::resolve_session_roots;
use crate::safety::tier::PermissionTier;
use crate::storage::{MessageEntry, MessageRole, WirePart};
use crate::tools::plan::PlanState;
use crate::util::errors::{E_INVALID_ARG, E_IO, E_SUB_BUDGET, IpcError};
use crate::util::events::keys;

/// 当前时间毫秒（Unix 纪元；取不到兜底 0，仅元数据展示用）。
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---- 委派参数与装配面 ----

/// `subagent` 工具参数契约（§8.3 M2 表）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SubSpawnArgs {
    pub role: String,
    pub task: String,
    /// 步数预算覆盖（钳制 1–1000；缺省按角色默认）。
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub budget_steps: Option<u32>,
}

/// 子代理历史 meta（§7.7：`histories/subs/<parentRunId>/<subId>/meta.json`，
/// 回看索引与 worktree 恢复指针）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct SubMeta {
    pub version: u32,
    pub session_id: String,
    pub sub_id: String,
    pub parent_run_id: String,
    pub role: String,
    pub task: String,
    pub budget_steps: u32,
    pub started_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// worktree 保留信息（失败/取消的未汇回成果位置，Q11 恢复索引）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_root: Option<String>,
}

/// 子代理委派的父 run 上下文（start_chat_core 装配面）：子代理默认跟随
/// 父 run 的模型与权限档（§5.9），帧经父枢纽透传。
#[derive(Clone)]
pub(crate) struct SubagentContext {
    pub session_id: String,
    pub parent_run_id: String,
    pub parent_hub: Arc<crate::agent::FrameHub>,
    pub tier: PermissionTier,
    pub model_id: Option<String>,
    pub auto_confirm: bool,
    /// 23 号票 ③ 会话内记忆启用：true 时命令类审批载荷增「始终允许
    /// （本会话）」选项；免批登记与主 run 共用会话桶。
    pub session_memory_enabled: bool,
}

/// 委派执行器（host 装配）：[`SubagentSpawner`] 的生产实现。
pub(crate) struct SubagentSpawnerImpl {
    /// 全字段同源的共享句柄副本（`AppState::share_handles`，06 号票同款）。
    pub app: AppState,
    pub ctx: SubagentContext,
}

impl SubagentSpawner for SubagentSpawnerImpl {
    fn spawn(
        &self,
        args_json: &str,
        parent_token: &CancellationToken,
    ) -> std::pin::Pin<Box<dyn Future<Output = crate::backend::run_loop::ToolExecution> + Send + '_>>
    {
        // AppState 非整体 Clone（内部全 Arc 句柄）：share_handles 产出同源
        // 轻量副本（06 号票同款），spawn 闭包持有
        let app = self.app.share_handles();
        let ctx = self.ctx.clone();
        let args_json = args_json.to_owned();
        let parent_token = parent_token.clone();
        Box::pin(async move { run_subagent(&app, &ctx, &args_json, &parent_token).await })
    }
}

/// 子 run 驱动的退出形态（收尾四象限的归一化输入）。
enum SubExit {
    /// 自然完成（可能有/没有汇报文本）。
    Done,
    /// 预算耗尽（rig max_turns 顶格 → `PromptError::MaxTurns`）。
    Budget,
    /// 被取消（stop_subagent / 父取消级联 / 审批拒绝）。
    Cancelled,
    /// 异常失败（模型/驱动错误）。
    Failed(String),
}

// ---- 子 run 帧出口：历史记账 + 事件 + 父枢纽透传 ----

/// 子代理单帧出口：历史记账 + 用量/步数事件 + 父枢纽 subFrame 包装转发。
/// 帧序不变量：外层先发 subStarted；本出口只负责中段；subDone 由收尾补发。
/// 20 号票：usage 差分增量同步落账（kind=sub，runs 计数不增，Q40）——
/// 会话明细（Q37）与日/模型聚合都按实际执行模型归属（`model_identity`
/// 为子 run 装配时解析的实际模型，子 run 不挂每拍刷新源，恒定不变）。
struct SubRunSink {
    /// 父 run 的帧枢纽（subFrame 入环可随 attach 重放恢复）。
    hub: Arc<crate::agent::FrameHub>,
    sub_id: String,
    recorder: SubHistoryRecorder,
    events: Arc<dyn crate::util::events::EventSink>,
    session_usage: Arc<Mutex<std::collections::HashMap<String, UsageTotal>>>,
    session_id: String,
    sub_run_id: String,
    /// 用量账本（AppState.ledger 克隆件；主/子/辅助共一入口，Q40 只计一次）。
    ledger: crate::host::usage::UsageLedger,
    /// 子 run 实际执行模型 `(配置 id, 显示名)`（装配时解析）。
    model_identity: (String, String),
    /// 子 run 自身累计（sub:usage 载荷 = 该子代理的自身消耗，Q40 子面板展示；
    /// 不重复累进会话累计——会话累计由 tokens:update 单一出口承担）。
    last_total: UsageTotal,
}

impl FrameSink for SubRunSink {
    fn send(&mut self, frame: ChatFrame) {
        // ① 历史记账（turn 收口/工具结果/文本累积）
        self.recorder.ingest(&frame);
        // ② 用量与步数事件（Q39/Q40）
        match &frame {
            ChatFrame::Usage { usage, .. } => {
                let delta = sub_usage(usage, &self.last_total);
                self.last_total = *usage;
                // 统计落账（20 号票：kind=sub；runs 计数不增）
                if delta != UsageTotal::default() {
                    self.ledger.record(
                        &self.session_id,
                        &self.sub_run_id,
                        crate::host::usage::run_kind::SUB,
                        &self.model_identity.0,
                        &self.model_identity.1,
                        delta,
                        false,
                    );
                }
                let total = {
                    let mut map = self
                        .session_usage
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let total = map.entry(self.session_id.clone()).or_default();
                    total.accumulate(&delta);
                    *total
                };
                let mut payload =
                    serde_json::json!({ "sessionId": self.session_id, "usage": total });
                if !self.model_identity.0.is_empty() {
                    payload["modelId"] = serde_json::json!(self.model_identity.0);
                }
                self.events.emit(keys::TOKENS_UPDATE, payload);
                self.events.emit(
                    keys::SUB_USAGE,
                    serde_json::json!({
                        "sessionId": self.session_id,
                        "runId": self.sub_run_id,
                        "subId": self.sub_id,
                        "usage": self.last_total,
                    }),
                );
            }
            ChatFrame::StepStarted { turn } => {
                self.events.emit(
                    keys::SUB_STEP,
                    serde_json::json!({
                        "sessionId": self.session_id,
                        "runId": self.sub_run_id,
                        "subId": self.sub_id,
                        "turn": turn,
                    }),
                );
            }
            _ => {}
        }
        // ③ 包 subFrame 转发父枢纽（seq 由父枢纽统一递增，入环可重放）
        self.hub.push(ChatFrame::SubFrame {
            sub_id: self.sub_id.clone(),
            frame: Box::new(frame),
        });
    }
}

/// usage 帧的子 run 累计差分。
fn sub_usage(a: &UsageTotal, b: &UsageTotal) -> UsageTotal {
    UsageTotal {
        input: a.input.saturating_sub(b.input),
        output: a.output.saturating_sub(b.output),
        cache_read: a.cache_read.saturating_sub(b.cache_read),
        cache_write: a.cache_write.saturating_sub(b.cache_write),
    }
}

// ---- 子代理历史（§7.7：单 JSONL 最小子集） ----

/// history.jsonl 的一行（serde tag=type）。Message/Tool 行按 turn 组织，
/// End 行收口（outcome + report）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SubHistoryLine {
    /// 一个模型轮次的 assistant 输出（正文 + 推理全文）。
    Message {
        turn: u32,
        text: String,
        #[serde(default)]
        reasoning: String,
    },
    /// 一次工具调用（参数全文 + 结果摘要）。
    Tool {
        turn: u32,
        call_id: String,
        name: String,
        args_json: String,
        ok: bool,
        summary: String,
    },
    /// 子代理收口行（四象限 + 汇报全文）。
    End { outcome: String, report: String },
}

/// 子代理历史记录器（Sink 侧纯记账；JSONL 逐行追加——崩溃最多丢不完整
/// 尾行，读取面忽略解析失败行，语义同主会话 SegmentLog 的恢复扫描）。
pub(crate) struct SubHistoryRecorder {
    path: PathBuf,
    turn: u32,
    text: String,
    reasoning: String,
    /// 本轮工具调用累积：(callId, name, argsJson)。
    tools: Vec<(String, String, String)>,
    /// 本轮工具结果：(callId, ok, summary)。
    results: Vec<(String, bool, String)>,
    /// 最后一次 assistant 正文（收尾 `<report>` 的取值面）。
    last_assistant_text: String,
}

impl SubHistoryRecorder {
    /// 打开记录器（建目录；文件在首行落盘时惰性创建）。
    pub(crate) fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            path: dir.join("history.jsonl"),
            turn: 0,
            text: String::new(),
            reasoning: String::new(),
            tools: Vec::new(),
            results: Vec::new(),
            last_assistant_text: String::new(),
        })
    }

    /// 单帧记账（不落盘；turn 收口/终态时 flush）。
    fn ingest(&mut self, frame: &ChatFrame) {
        match frame {
            ChatFrame::StepStarted { turn } => {
                self.close_turn();
                self.turn = *turn;
            }
            ChatFrame::TextDelta { delta, .. } => self.text.push_str(delta),
            ChatFrame::ReasoningDelta { delta, .. } => self.reasoning.push_str(delta),
            ChatFrame::ToolStarted { call_id, name } => {
                self.tools
                    .push((call_id.clone(), name.clone(), String::new()))
            }
            ChatFrame::ToolArgs { call_id, chunk } => {
                if let Some(entry) = self.tools.iter_mut().rev().find(|(id, _, _)| id == call_id) {
                    entry.2.push_str(chunk);
                }
            }
            ChatFrame::ToolResult {
                call_id,
                ok,
                ui_summary,
                ..
            } => self
                .results
                .push((call_id.clone(), *ok, ui_summary.clone())),
            _ => {}
        }
    }

    /// 收口当前轮：assistant 行（有输出时）+ 工具行逐条落盘并清空窗口。
    /// 幂等（重复调用无副作用）。
    pub(crate) fn close_turn(&mut self) {
        let turn = self.turn;
        if self.text.is_empty() && self.reasoning.is_empty() && self.tools.is_empty() {
            return;
        }
        if !self.text.is_empty() || !self.reasoning.is_empty() {
            let reasoning = std::mem::take(&mut self.reasoning);
            let text = std::mem::take(&mut self.text);
            self.last_assistant_text = text.clone();
            self.append(&SubHistoryLine::Message {
                turn,
                text,
                reasoning,
            });
        }
        let tools = std::mem::take(&mut self.tools);
        let results = std::mem::take(&mut self.results);
        for (call_id, name, args_json) in tools {
            // 结果按 callId 配对（同批帧序保证 result 晚于 started）
            let hit = results
                .iter()
                .find(|(id, _, _)| id == &call_id)
                .cloned()
                .unwrap_or_else(|| (call_id.clone(), true, String::new()));
            self.append(&SubHistoryLine::Tool {
                turn,
                call_id,
                name,
                args_json,
                ok: hit.1,
                summary: hit.2,
            });
        }
    }

    /// 追加一行（行级小写；失败仅 debug 打印——历史是回看面，不阻断 run）。
    fn append(&mut self, line: &SubHistoryLine) {
        use std::io::Write;
        if let Ok(mut json) = serde_json::to_string(line) {
            json.push('\n');
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
            {
                let _ = file.write_all(json.as_bytes());
            } else {
                #[cfg(debug_assertions)]
                eprintln!(
                    "[craftwave] 子代理历史行写入失败（{}）",
                    self.path.display()
                );
            }
        }
    }
}

/// 追加一行 End 到 history.jsonl（收尾面；文件不存在则创建）。
fn append_history_end(dir: &Path, outcome: &str, report: &str) {
    use std::io::Write;
    let line = SubHistoryLine::End {
        outcome: outcome.to_owned(),
        report: report.to_owned(),
    };
    if let Ok(mut json) = serde_json::to_string(&line) {
        json.push('\n');
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("history.jsonl"))
        {
            let _ = file.write_all(json.as_bytes());
        }
    }
}

/// 子代理历史目录：`<data>/histories/subs/<parentRunId>/<subId>/`。
fn sub_history_dir(data_root: &Path, parent_run_id: &str, sub_id: &str) -> PathBuf {
    data_root
        .join("histories")
        .join("subs")
        .join(parent_run_id)
        .join(sub_id)
}

/// 写 meta.json（原子写，§7.4）。
fn write_sub_meta(meta: &SubMeta, dir: &Path) {
    if let Ok(bytes) = serde_json::to_vec_pretty(meta) {
        let _ = crate::util::atomic::write_atomic(&dir.join("meta.json"), &bytes);
    }
}

/// 读一条子代理历史并转换为 WireMessage（MessageEntry）序列（FR-07-06
/// 抽屉回看面）：Message/Tool 行按 turn 组装为 assistant 条目（toolCall +
/// toolResult parts 同条目，与主会话格式同形），End 行产出 report 条目。
pub(crate) fn load_sub_history_messages(
    dir: &Path,
    sub_id: &str,
) -> Result<Vec<MessageEntry>, IpcError> {
    let path = dir.join("history.jsonl");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| IpcError::new(E_IO, format!("子代理历史读取失败：{e}")))?;
    let mut lines: Vec<SubHistoryLine> = Vec::new();
    for line in text.lines() {
        // 忽略解析失败行（崩溃残留的不完整尾行，见文件头注释）
        if let Ok(parsed) = serde_json::from_str::<SubHistoryLine>(line) {
            lines.push(parsed);
        }
    }
    // 按 turn 分组：turn → (tools, text, reasoning)
    struct TurnGroup {
        tools: Vec<(String, String, String, bool, String)>,
        text: String,
        reasoning: String,
    }
    let mut groups: BTreeMap<u32, TurnGroup> = BTreeMap::new();
    let mut report_text: Option<String> = None;
    for line in lines {
        match line {
            SubHistoryLine::Message {
                turn,
                text,
                reasoning,
            } => {
                let group = groups.entry(turn).or_insert_with(|| TurnGroup {
                    tools: Vec::new(),
                    text: String::new(),
                    reasoning: String::new(),
                });
                group.text.push_str(&text);
                if !reasoning.is_empty() {
                    if !group.reasoning.is_empty() {
                        group.reasoning.push_str("\n\n");
                    }
                    group.reasoning.push_str(&reasoning);
                }
            }
            SubHistoryLine::Tool {
                turn,
                call_id,
                name,
                args_json,
                ok,
                summary,
            } => {
                let group = groups.entry(turn).or_insert_with(|| TurnGroup {
                    tools: Vec::new(),
                    text: String::new(),
                    reasoning: String::new(),
                });
                group.tools.push((call_id, name, args_json, ok, summary));
            }
            SubHistoryLine::End { report, .. } => {
                if !report.is_empty() {
                    report_text = Some(report);
                }
            }
        }
    }
    let mut out: Vec<MessageEntry> = Vec::new();
    let mut parent: Option<String> = None;
    for (turn, group) in groups {
        let mut parts: Vec<WirePart> = Vec::new();
        for (call_id, name, args_json, ok, summary) in &group.tools {
            parts.push(WirePart::ToolCall {
                call_id: call_id.clone(),
                name: name.clone(),
                args_json: args_json.clone(),
            });
            parts.push(WirePart::ToolResult {
                call_id: call_id.clone(),
                ok: *ok,
                summary: summary.clone(),
            });
        }
        if !group.reasoning.is_empty() {
            parts.push(WirePart::Reasoning {
                text: group.reasoning.clone(),
            });
        }
        if !group.text.is_empty() {
            parts.push(WirePart::Text {
                text: group.text.clone(),
            });
        }
        let id = format!("{sub_id}:a{turn}");
        out.push(MessageEntry {
            id: id.clone(),
            parent_id: parent.take(),
            role: MessageRole::Assistant,
            parts,
            created_at: 0,
            run_id: Some(sub_id.to_owned()),
            status: None,
        });
        parent = Some(id);
    }
    if let Some(report) = report_text {
        let id = format!("{sub_id}:report");
        out.push(MessageEntry {
            id: id.clone(),
            parent_id: parent.take(),
            role: MessageRole::Assistant,
            parts: vec![WirePart::Text { text: report }],
            created_at: 0,
            run_id: Some(sub_id.to_owned()),
            status: None,
        });
    }
    Ok(out)
}

/// `load_subagent_history` 主体（§3.3 #59）：按 (sessionId, subId) 在
/// `histories/subs/<parentRunId>/<subId>/` 下定位（parentRunId 不在参数面，
/// 扫描一层目录匹配 meta），未命中 → E_NOT_FOUND。
pub(crate) fn load_subagent_history_core(
    app: &AppState,
    session_id: &str,
    sub_id: &str,
) -> Result<Vec<MessageEntry>, IpcError> {
    let subs_root = app.store.root().join("histories").join("subs");
    let Ok(entries) = std::fs::read_dir(&subs_root) else {
        return Err(IpcError::not_found(format!("子代理历史不存在：{sub_id}")));
    };
    for entry in entries.flatten() {
        let candidate = entry.path().join(sub_id);
        let meta_path = candidate.join("meta.json");
        if !meta_path.exists() {
            continue;
        }
        let hit = std::fs::read_to_string(&meta_path)
            .ok()
            .and_then(|text| serde_json::from_str::<SubMeta>(&text).ok())
            .is_some_and(|meta| meta.session_id == session_id && meta.sub_id == sub_id);
        if hit {
            return load_sub_history_messages(&candidate, sub_id);
        }
    }
    Err(IpcError::not_found(format!("子代理历史不存在：{sub_id}")))
}

// ---- worktree 收尾处置（Q10/Q11） ----

/// worktree 收尾处置结果（模型/UI 双通道摘要）。
struct WorktreeOutcome {
    /// 模型通道的回收摘要（主代理据此解决冲突/验收）。
    summary: String,
    /// UI 通道摘要。
    ui: String,
}

/// 处置 worktree：成功面（report/partial）汇回并清理；失败/取消保留现场
/// 标注未完成（不汇回，重启不自动续跑——Q11）。
fn settle_worktree(
    lease: &mut WorktreeLease,
    data_root: &Path,
    outcome: SubOutcome,
) -> WorktreeOutcome {
    match outcome {
        SubOutcome::Report | SubOutcome::Partial => match wt::recover(lease) {
            Ok(recovery) => {
                let summary = recovery_summary(&recovery);
                let ui = format!(
                    "已回收 {} 个改动文件、删除 {} 个；{} 个冲突保留双方",
                    recovery.applied.len(),
                    recovery.deleted.len(),
                    recovery.conflicts.len()
                );
                // 成功汇回并验证（回收报告即验证面）→ 清理 worktree
                let _ = wt::mark_status(lease, data_root, wt::LEASE_RECOVERED, None);
                let _ = wt::cleanup(lease, data_root);
                WorktreeOutcome { summary, ui }
            }
            Err(e) => {
                // 汇回失败：保留现场等主代理检查（不静默丢弃成果）
                let _ = wt::mark_status(
                    lease,
                    data_root,
                    wt::LEASE_ABANDONED,
                    Some(format!("回收失败：{e}")),
                );
                WorktreeOutcome {
                    summary: format!(
                        "注意：独立工作区汇回失败（{e}），改动保留在 {} 未汇入，\
                         可检查后回收",
                        lease.worktree_root
                    ),
                    ui: "汇回失败，改动已保留".to_owned(),
                }
            }
        },
        _ => {
            let _ = wt::mark_status(
                lease,
                data_root,
                wt::LEASE_ABANDONED,
                Some("未完成，改动保留待检查".to_owned()),
            );
            WorktreeOutcome {
                summary: format!(
                    "子代理未完成：其工作区改动保留在 {}（未汇回、标注未完成），\
                     可检查后决定是否回收",
                    lease.worktree_root
                ),
                ui: "未完成，工作区已保留".to_owned(),
            }
        }
    }
}

/// 回收报告的模型通道摘要（冲突文件附副本路径，主代理负责解决并验证）。
fn recovery_summary(report: &RecoveryReport) -> String {
    let mut lines = Vec::new();
    if !report.applied.is_empty() {
        lines.push(format!(
            "以下改动已直接应用到当前工作区（无冲突）：\n{}",
            report
                .applied
                .iter()
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if !report.deleted.is_empty() {
        lines.push(format!(
            "以下文件在独立工作区被删除且当前工作区未改动，已同步删除：\n{}",
            report
                .deleted
                .iter()
                .map(|p| format!("- {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if !report.conflicts.is_empty() {
        lines.push(format!(
            "以下文件存在冲突（当前工作区在子代理运行期间也被修改过）：\
             子代理版本已保留为冲突副本，请对比当前工作区文件解决并验证：\n{}",
            report
                .conflicts
                .iter()
                .map(|(p, copy)| format!("- {p}（子代理版本副本：{copy}）"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if lines.is_empty() {
        "独立工作区无文件改动需要回收。".to_owned()
    } else {
        lines.join("\n\n")
    }
}

// ---- 委派执行主流程 ----

/// 委派执行主体：参数校验 → 名额排队 → worktree 快照 → 子 run 驱动 →
/// 四象限收尾。任何失败路径都保证：gate 释放、注册表摘除、subDone 帧收尾。
async fn run_subagent(
    app: &AppState,
    ctx: &SubagentContext,
    args_json: &str,
    parent_token: &CancellationToken,
) -> crate::backend::run_loop::ToolExecution {
    // ① 参数与角色解析（失败 = 工具级错误回喂，不中断父 run）
    let args: SubSpawnArgs = match serde_json::from_str(args_json) {
        Ok(a) => a,
        Err(e) => return spawn_failure(E_INVALID_ARG, &format!("subagent 参数解析失败：{e}")),
    };
    let Some(role) = find_role(&args.role) else {
        let known = roles::ROLES
            .iter()
            .map(|r| r.name)
            .collect::<Vec<_>>()
            .join("、");
        return spawn_failure(
            E_INVALID_ARG,
            &format!("未知子代理角色 {}（可用：{known}）", args.role),
        );
    };
    let task = args.task.trim().to_owned();
    if task.is_empty() {
        return spawn_failure(
            E_INVALID_ARG,
            "task 不能为空（子代理无本对话历史，任务描述必须自足）",
        );
    }
    let budget = clamp_budget_steps(role, args.budget_steps);

    // ② 并发名额（全局共享上限；满额排队不拒绝；父取消 → 未启动即收）
    if !app.subs.gate().acquire(parent_token).await {
        return crate::backend::run_loop::ToolExecution::Cancelled {
            executed: false,
            message: "委派在排队等待并发名额时被取消".to_owned(),
        };
    }

    // ③ worktree 决策（Q7）：写入角色 + Git 项目 → 独立 worktree；
    //    非 Git 项目写入子代理直接拒绝（由主代理修改）；只读角色读当前工作区
    let (project_root, workspace_root) = resolve_session_roots(app, &ctx.session_id);
    let data_root = app.store.root().to_path_buf();
    let mut lease: Option<WorktreeLease> = None;
    if role.workspace == WorkspaceKind::WriteWorkspace {
        if !wt::project_is_git(&project_root) {
            app.subs.gate().release();
            return spawn_failure(
                E_INVALID_ARG,
                "非 Git 项目子代理只读：修改须由主代理执行；请把该任务接回主代理处理",
            );
        }
        let lease_sub_id = uuid::Uuid::new_v4().to_string();
        match wt::create_lease(
            &project_root,
            &data_root,
            &lease_sub_id,
            &ctx.session_id,
            role.name,
        ) {
            Ok(created) => lease = Some(created),
            // 快照创建失败不静默退回旧 HEAD（Q9）：直接回喂错误
            Err(e) => {
                app.subs.gate().release();
                return spawn_failure(E_IO, &format!("独立工作区创建失败：{e}"));
            }
        }
    }
    let sub_id = lease
        .as_ref()
        .map(|l| l.sub_id.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // ④ 注册句柄（取消令牌 = 父令牌的子令牌：父取消自动级联）
    let sub_run_id = uuid::Uuid::new_v4().to_string();
    let handle = Arc::new(SubHandle {
        sub_id: sub_id.clone(),
        role: role.name.to_owned(),
        session_id: ctx.session_id.clone(),
        parent_run_id: ctx.parent_run_id.clone(),
        cancel: parent_token.child_token(),
        ask: Arc::new(Mutex::new(None)),
        started_at: now_ms(),
    });
    app.subs.register(handle.clone());

    // 收尾闭包共用的历史目录
    let history_dir = sub_history_dir(&data_root, &ctx.parent_run_id, &sub_id);

    // ⑤ subStarted 帧 + sub:spawn 事件（先于全部子帧）+ meta.json
    ctx.parent_hub.push(ChatFrame::SubStarted {
        call_id: sub_id.clone(),
        sub_id: sub_id.clone(),
        role: role.name.to_owned(),
    });
    app.events.emit(
        keys::SUB_SPAWN,
        serde_json::json!({
            "sessionId": ctx.session_id,
            "runId": ctx.parent_run_id,
            "subId": sub_id,
            "role": role.name,
        }),
    );
    write_sub_meta(
        &SubMeta {
            version: 1,
            session_id: ctx.session_id.clone(),
            sub_id: sub_id.clone(),
            parent_run_id: ctx.parent_run_id.clone(),
            role: role.name.to_owned(),
            task: task.clone(),
            budget_steps: budget,
            started_at: handle.started_at,
            outcome: None,
            worktree_root: lease.as_ref().map(|l| l.worktree_root.clone()),
        },
        &history_dir,
    );

    // ⑥ 子 run 装配：模型（跟随父）→ 请求面（cleanContext 默认）→
    //    工具表（白名单 × 档位）→ 执行器（写边界指向 worktree）与审批门
    let stall_timeout = std::time::Duration::from_secs(u64::from(
        app.config
            .load()
            .map(|c| c.watchdog.stall_timeout_secs)
            .unwrap_or(300),
    ));
    let (resolved, spec, prompt, history) = match assemble_sub_run(app, ctx, role, &task, budget) {
        Ok(parts) => parts,
        Err(message) => {
            let exit = SubExit::Failed(message);
            return finalize_spawn(
                app,
                ctx,
                &sub_id,
                &sub_run_id,
                role,
                handle,
                lease,
                &history_dir,
                budget,
                exit,
                None,
            );
        }
    };
    let driver = TurnDriver::new(
        resolved.driver,
        spec.clone(),
        sub_tool_definitions(role, ctx.tier),
    );

    // 写入角色 → 工具根指向 worktree（写边界隔离）；只读角色 → 当前工作区
    let (roots_project, roots_data) = match &lease {
        Some(l) => (PathBuf::from(&l.worktree_root), workspace_root.clone()),
        None => (project_root.clone(), workspace_root.clone()),
    };
    // 审批门装配用副本（roots 本体移入执行器 ToolsContext）
    let gate_project = roots_project.clone();
    let gate_data = roots_data.clone();
    let allowed: Vec<String> = roles::role_tool_specs(role, ctx.tier)
        .iter()
        .map(|spec| spec.name.clone())
        .collect();
    let sub_plan: Arc<Mutex<Option<PlanState>>> = Arc::new(Mutex::new(None));
    let session_usage = Arc::new(Mutex::new(
        app.session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    ));
    let recorder = match SubHistoryRecorder::open(&history_dir) {
        Ok(r) => r,
        Err(e) => {
            let exit = SubExit::Failed(format!("子代理历史初始化失败：{e}"));
            return finalize_spawn(
                app,
                ctx,
                &sub_id,
                &sub_run_id,
                role,
                handle,
                lease,
                &history_dir,
                budget,
                exit,
                None,
            );
        }
    };
    let executor: Arc<dyn CancellableExecutor> = Arc::new(
        ToolsetExecutor::new(
            crate::tools::ToolsContext::new(roots_project, roots_data),
            ctx.tier,
        )
        .with_env(ExecutorEnv {
            events: app.events.clone(),
            ask: handle.ask.clone(),
            plan: sub_plan.clone(),
            session_id: ctx.session_id.clone(),
            run_id: sub_run_id.clone(),
            data_root: data_root.clone(),
            // 子代理边界（Q13 + 公共操作）：无 MCP 动态段、无委派面、
            // 执行入口按白名单过滤（工具可见面 = 执行许可）
            mcp: None,
            skills: None,
            subagents: None,
            allowed_tools: Some(allowed),
            // 网络工具环境（12 号票）：子代理白名单一般不含网络工具，
            // 但环境照常装配（执行入口白名单先拦；策略与主代理同源）。
            // 配置读取失败按缺省配置装配（fail-closed：缺省拒绝私网）。
            network: Some(std::sync::Arc::new(match app.config.load() {
                Ok(config) => crate::tools::NetworkEnv::from_config(&config),
                Err(_) => crate::tools::NetworkEnv::from_config(&Default::default()),
            })),
        }),
    );
    // 审批门（23 号票装配面）：与主 run 同口径——内部状态禁区判定
    //（FR-05-08，子代理写根 + 数据目录根）、会话级免批桶（always_session
    // 与主 run 共用会话桶）、审批策略快照（四开关 ①④ + 白名单条目）。
    // 无 MCP 视图（子代理不装配动态段，§5.9）。围栏工作区上下文同主 run
    // 口径装配（roots/base canonical 后传入；失败退化为有写目标即 Confirm）。
    let gate = {
        let canonical = |p: &std::path::Path| std::fs::canonicalize(p).ok();
        let gate_ws = match (canonical(&gate_project), canonical(&gate_data)) {
            (Some(project), Some(data)) => Some(crate::backend::gate::GateWorkspace {
                roots: vec![project, data],
                base: PathBuf::from(&project_root),
            }),
            _ => None,
        };
        let approval = app
            .config
            .load()
            .map(|c| c.approval.clone())
            .unwrap_or_default();
        let mut gate = ApprovalGate::new(ctx.tier, sub_plan)
            .with_internal_guard(crate::safety::internal_state::InternalStateGuard::new(
                &data_root,
                &project_root,
            ))
            .with_session_memory(app.approval_memory.bucket(&ctx.session_id))
            .with_policy(crate::backend::gate::ApprovalPolicy {
                enabled: approval.enabled,
                allowlist_enabled: approval.allowlist_enabled,
                command_allowlist: approval.command_allowlist.clone(),
            });
        if let Some(ws) = gate_ws {
            gate = gate.with_workspace(ws);
        }
        Arc::new(gate)
    };
    // 子 run 实际执行模型（20 号票落账归属；子 run 无每拍刷新源，恒定）
    let model_identity = (resolved.id.clone(), resolved.name.clone());
    let mut sink = SubRunSink {
        hub: ctx.parent_hub.clone(),
        sub_id: sub_id.clone(),
        recorder,
        events: app.events.clone(),
        session_usage,
        session_id: ctx.session_id.clone(),
        sub_run_id: sub_run_id.clone(),
        ledger: app.ledger.clone(),
        model_identity,
        last_total: UsageTotal::default(),
    };

    // ⑦ 驱动循环（审批挂起恢复 + 取消级联 + 预算耗尽归一）
    let mut run = rig::run::AgentRun::from_spec(&spec, prompt, Some(history));
    let mut resume_state: Option<DriveState> = None;
    let sub_cancel = handle.cancel.clone();
    let exit: SubExit = loop {
        let attempt = drive_run(
            &driver,
            run,
            &sub_run_id,
            &resolved.name,
            sub_cancel.clone(),
            executor.clone(),
            gate.clone(),
            &mut sink,
            resume_state.take(),
            None, // 子代理无 steer 插话
            None, // 子代理无每拍请求刷新源（18 号票刷新面仅主 run）
            stall_timeout,
        )
        .await;
        match attempt {
            // 预算耗尽（§5.9：max_turns 顶格 → rig MaxTurns）→ partial
            Err(rig::run::PromptError::MaxTurns { .. }) => break SubExit::Budget,
            Err(err) => {
                let message = err.to_string();
                send_sub_error_ended(&mut sink, message.clone());
                break SubExit::Failed(message);
            }
            Ok(RunExit::Done(_)) => break SubExit::Done,
            Ok(RunExit::Cancelled) => break SubExit::Cancelled,
            Ok(RunExit::Stalled { message }) => break SubExit::Failed(message),
            // 每拍请求刷新失败（18 号票新变体；子代理未挂刷新源，防御面
            // 兜底——语义与 Stalled 同形：runEnded(error) 已发出，按失败收）
            Ok(RunExit::Failed { message, .. }) => break SubExit::Failed(message),
            Ok(RunExit::Suspended(info)) => {
                let crate::backend::run_loop::SuspendedInfo {
                    run_json,
                    pending,
                    state,
                } = *info;
                match wait_approval_with(
                    &app.events,
                    &ctx.session_id,
                    &sub_run_id,
                    &handle.cancel,
                    &handle.ask,
                    &pending,
                    ctx.auto_confirm,
                    ctx.session_memory_enabled,
                )
                .await
                {
                    ApprovalResolution::Allowed { option_id } => {
                        gate.approve(&pending.name, &pending.args_json);
                        // 23 号票：always_session → 会话级免批桶登记
                        //（与主 run 共用会话桶，跨 run 生效、不持久化）
                        if option_id == "always_session" {
                            app.approval_memory.remember(
                                &ctx.session_id,
                                &pending.name,
                                &pending.args_json,
                            );
                        }
                        match crate::backend::run_loop::resume_run(&run_json) {
                            Ok(resumed) => {
                                run = resumed;
                                resume_state = Some(state);
                                continue;
                            }
                            Err(e) => {
                                let message = format!("审批恢复失败：{e}");
                                send_sub_error_ended(&mut sink, message.clone());
                                break SubExit::Failed(message);
                            }
                        }
                    }
                    ApprovalResolution::Denied => {
                        denied_finish(&mut sink, &pending, &state);
                        break SubExit::Cancelled;
                    }
                }
            }
        }
    };
    // 收口最后轮记账并取 <report> 候选文本（最后一次 assistant 正文）
    sink.recorder.close_turn();
    let report_text = sink.recorder.last_assistant_text.clone();
    finalize_spawn(
        app,
        ctx,
        &sub_id,
        &sub_run_id,
        role,
        handle,
        lease,
        &history_dir,
        budget,
        exit,
        Some(report_text),
    )
}

/// 装配产物（type_complexity 收敛）：模型、运行规格、prompt 与 history。
type AssembledSubRun = (
    crate::host::commands::chat::ResolvedModel,
    RunSpec,
    Arc<Message>,
    Vec<Arc<Message>>,
);

/// 子 run 装配：模型解析 + 请求面构造（cleanContext 默认——子代理上下文
/// 干净，仅角色系统提示 + task；父历史注入形态见 [`build_subagent_history`]，
/// M2 未开放配置）。失败返回错误消息（finalize 按 no_report 收口）。
fn assemble_sub_run(
    app: &AppState,
    ctx: &SubagentContext,
    role: &SubagentRole,
    task: &str,
    budget: u32,
) -> Result<AssembledSubRun, String> {
    let resolved = resolve_model(app, ctx.model_id.as_deref())
        .map_err(|e| format!("子代理模型解析失败：{}", e.message))?;
    // build_subagent_history 返回 [系统提示(, …), task]：末位 task 即本轮
    // prompt，其余为 history（drive_run 的入参切分）
    let mut messages = build_subagent_history(task, role, &[], true);
    let prompt = messages
        .pop()
        .expect("build_subagent_history 必产出 task 消息");
    let spec = RunSpec {
        max_turns: Some(budget as usize),
        ..RunSpec::new()
    };
    Ok((resolved, spec, prompt, messages))
}

/// 白名单工具表 → rig ToolDefinition（子 run 广播面；plan 档写工具已被
/// role_tool_specs 过滤——可见面 = 执行许可，三处同源）。
fn sub_tool_definitions(
    role: &SubagentRole,
    tier: PermissionTier,
) -> Vec<Arc<rig::completion::request::ToolDefinition>> {
    roles::role_tool_specs(role, tier)
        .into_iter()
        .filter_map(|spec| {
            let name = rig::message::ToolName::new(spec.name.clone()).ok()?;
            Some(Arc::new(rig::completion::request::ToolDefinition::new(
                name,
                spec.description,
                spec.params_schema,
            )))
        })
        .collect()
}

/// 子代理请求 history 构造（§5.9 末行）：返回 `[系统提示(, 父历史尾), task]`
/// 序列——末位是 task 消息（drive_run 的 prompt），其余为 history。
/// - clean_context=true（默认/生产）：仅角色系统提示 + task；
/// - clean_context=false：角色系统提示 + 父历史尾至多 6 条 + task
///   （经防腐层 wire 重建透传，不进父会话史册）。
pub(crate) fn build_subagent_history(
    task: &str,
    role: &SubagentRole,
    parent_tail: &[MessageEntry],
    clean_context: bool,
) -> Vec<Arc<Message>> {
    let mut history: Vec<Arc<Message>> = vec![Arc::new(Message::System {
        content: role.system_prompt.to_owned(),
    })];
    if !clean_context {
        let tail_start = parent_tail.len().saturating_sub(6);
        if let Ok(mut rebuilt) =
            crate::backend::wire::rebuild_message_stream(&parent_tail[tail_start..])
        {
            history.append(&mut rebuilt);
        }
    }
    history.push(Arc::new(Message::User {
        content: vec![rig::message::UserContent::text(task.to_owned())],
    }));
    history
}

/// 委派未启动/早夭的工具级失败面（gate 未取得名额或快照失败等；不发
/// subStarted——前端工具卡只见失败结果）。
fn spawn_failure(code: &str, message: &str) -> crate::backend::run_loop::ToolExecution {
    crate::backend::run_loop::ToolExecution::Completed {
        ok: false,
        ui_summary: format!("子代理委派失败：{message}"),
        error: Some(FrameError::new(code, message.to_owned())),
        model_text: format!("{code} {message}"),
    }
}

/// 子 run 错误终态兜底帧（经 SubRunSink 包成 subFrame 入父枢纽）。
fn send_sub_error_ended(sink: &mut SubRunSink, message: String) {
    #[cfg(debug_assertions)]
    eprintln!("[craftwave] 子代理错误终态：{message}");
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Error,
        duration_ms: 0,
        tool_count: 0,
        turn: sink.recorder.turn.max(1),
        error: Some(FrameError::new(crate::util::errors::E_INTERNAL, message)),
    });
}

/// outcome 的字符串键（事件/历史/meta 三面共用）。
fn outcome_key(outcome: &SubOutcome) -> &'static str {
    match outcome {
        SubOutcome::Report => "report",
        SubOutcome::Partial => "partial",
        SubOutcome::NoReport => "no_report",
        SubOutcome::Cancelled => "cancelled",
    }
}

/// 收尾主体（唯一出口）：四象限判定 → 历史收口 → meta 补写 → worktree
/// 处置 → subReport/subDone 帧 + sub:* 事件 → 注册表摘除与名额释放 →
/// 父工具结果。
#[allow(clippy::too_many_arguments)]
fn finalize_spawn(
    app: &AppState,
    ctx: &SubagentContext,
    sub_id: &str,
    sub_run_id: &str,
    role: &SubagentRole,
    handle: Arc<SubHandle>,
    mut lease: Option<WorktreeLease>,
    history_dir: &Path,
    budget: u32,
    exit: SubExit,
    report_candidate: Option<String>,
) -> crate::backend::run_loop::ToolExecution {
    // 子 run 明细收口（20 号票 Q37：补写 endedAt；终态只收口不搬移已记日期）
    let _ = app
        .ledger
        .detail
        .mark_run_ended(&ctx.session_id, sub_run_id);
    // 四象限（§5.9）：report / partial / no_report / cancelled
    let report = report_candidate.unwrap_or_default();
    let (outcome, error_note): (SubOutcome, Option<(String, String)>) = match exit {
        SubExit::Done => {
            if report.trim().is_empty() {
                (
                    SubOutcome::NoReport,
                    Some((
                        "E_INTERNAL".to_owned(),
                        "子代理结束但没有产出任何汇报文本".to_owned(),
                    )),
                )
            } else {
                (SubOutcome::Report, None)
            }
        }
        SubExit::Budget => (SubOutcome::Partial, None),
        SubExit::Cancelled => (SubOutcome::Cancelled, None),
        SubExit::Failed(message) => (
            SubOutcome::NoReport,
            Some(("E_INTERNAL".to_owned(), message)),
        ),
    };

    // 历史收口（End 行）+ meta 终态补写
    append_history_end(history_dir, outcome_key(&outcome), &report);
    let mut meta_updated: Option<SubMeta> = None;
    if let Ok(bytes) = std::fs::read(history_dir.join("meta.json"))
        && let Ok(mut meta) = serde_json::from_str::<SubMeta>(&String::from_utf8_lossy(&bytes))
    {
        meta.outcome = Some(outcome_key(&outcome).to_owned());
        meta.worktree_root = lease.as_ref().map(|l| l.worktree_root.clone());
        if let Ok(new_bytes) = serde_json::to_vec_pretty(&meta) {
            let _ = crate::util::atomic::write_atomic(&history_dir.join("meta.json"), &new_bytes);
        }
        meta_updated = Some(meta);
    }
    let _ = meta_updated;

    // worktree 处置（Q10/Q11）
    let worktree_outcome = match lease.as_mut() {
        Some(l) => settle_worktree(l, app.store.root(), outcome),
        None => WorktreeOutcome {
            summary: String::new(),
            ui: String::new(),
        },
    };

    // subReport 帧 + 事件（report/partial 且有文本）
    let has_report =
        !report.trim().is_empty() && matches!(outcome, SubOutcome::Report | SubOutcome::Partial);
    if has_report {
        ctx.parent_hub.push(ChatFrame::SubReport {
            sub_id: sub_id.to_owned(),
            report: report.clone(),
        });
        app.events.emit(
            keys::SUB_REPORT,
            serde_json::json!({
                "sessionId": ctx.session_id,
                "runId": ctx.parent_run_id,
                "subId": sub_id,
                "report": report,
            }),
        );
    }
    // 异常终态事件（partial 的预算说明不作为 error 事件——outcome 已表达）
    if let Some((code, message)) = &error_note
        && outcome == SubOutcome::NoReport
    {
        app.events.emit(
            keys::SUB_ERROR,
            serde_json::json!({
                "sessionId": ctx.session_id,
                "runId": ctx.parent_run_id,
                "subId": sub_id,
                "code": code,
                "message": message,
            }),
        );
    }
    // subDone 帧（该子代理帧序列的最后一帧）+ 事件
    ctx.parent_hub.push(ChatFrame::SubDone {
        sub_id: sub_id.to_owned(),
        outcome,
    });
    app.events.emit(
        keys::SUB_DONE,
        serde_json::json!({
            "sessionId": ctx.session_id,
            "runId": ctx.parent_run_id,
            "subId": sub_id,
            "outcome": outcome_key(&outcome),
        }),
    );
    // 摘除 + 释放名额
    app.subs.remove(sub_id);
    app.subs.gate().release();
    let _ = handle;

    // 父工具结果（模型通道）
    match outcome {
        SubOutcome::Cancelled => crate::backend::run_loop::ToolExecution::Cancelled {
            executed: true,
            message: "子代理已被取消（stop_subagent 或父任务取消级联）".to_owned(),
        },
        SubOutcome::Report => {
            let mut text = format!("<report>\n{report}\n</report>");
            if !worktree_outcome.summary.is_empty() {
                text.push_str("\n\n工作区回收结果：\n");
                text.push_str(&worktree_outcome.summary);
            }
            crate::backend::run_loop::ToolExecution::Completed {
                ok: true,
                ui_summary: format!(
                    "子代理 {} 完成 · {}",
                    role.name,
                    if worktree_outcome.ui.is_empty() {
                        "汇报已返回".to_owned()
                    } else {
                        worktree_outcome.ui
                    }
                ),
                error: None,
                model_text: text,
            }
        }
        SubOutcome::Partial => {
            let mut text = format!(
                "{E_SUB_BUDGET} 子代理步数预算耗尽（{budget} 轮上限，outcome=partial）。\
                 如需继续可再次委派并在 task 中说明续接点。\n"
            );
            if !report.trim().is_empty() {
                text.push_str(&format!("\n<report>\n{report}\n</report>\n"));
            }
            if !worktree_outcome.summary.is_empty() {
                text.push_str("\n工作区回收结果：\n");
                text.push_str(&worktree_outcome.summary);
            }
            crate::backend::run_loop::ToolExecution::Completed {
                ok: false,
                ui_summary: format!("子代理 {} 预算耗尽（partial）", role.name),
                error: Some(FrameError::new(
                    E_SUB_BUDGET,
                    format!("子代理步数预算耗尽（{budget} 轮上限）"),
                )),
                model_text: text,
            }
        }
        SubOutcome::NoReport => {
            let note = error_note
                .as_ref()
                .map(|(_, m)| m.clone())
                .unwrap_or_else(|| "子代理异常结束".to_owned());
            let mut text = format!("子代理异常结束（no_report）：{note}");
            if !worktree_outcome.summary.is_empty() {
                text.push('\n');
                text.push_str(&worktree_outcome.summary);
            }
            crate::backend::run_loop::ToolExecution::Completed {
                ok: false,
                ui_summary: format!("子代理 {} 异常结束", role.name),
                error: Some(FrameError::new(
                    error_note
                        .as_ref()
                        .map(|(c, _)| c.as_str())
                        .unwrap_or("E_INTERNAL"),
                    note.clone(),
                )),
                model_text: text,
            }
        }
    }
}

// ---- 命令面（§3.3 #58/#59） ----

/// `stop_subagent` 参数契约（§3.3 #58）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StopSubagentArgs {
    pub sub_id: String,
}

/// `stop_subagent` 主体（§3.3 #58）：级联取消（子代理令牌置位即返回；
/// 终态以该子代理的 subFrame 序列（runEnded + subDone）为权威）。
pub(crate) fn stop_subagent_core(app: &AppState, sub_id: &str) -> Result<(), IpcError> {
    let handle = app
        .subs
        .get(sub_id)
        .ok_or_else(|| IpcError::not_found(format!("子代理不存在或已结束：{sub_id}")))?;
    handle.cancel.cancel();
    Ok(())
}

/// `stop_subagent` 命令（§3.3 #58）。
#[tauri::command]
pub(crate) async fn stop_subagent(
    app: State<'_, AppState>,
    sub_id: String,
) -> Result<(), IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = StopSubagentArgs { sub_id };
    stop_subagent_core(&app, &args.sub_id)
}

/// `load_subagent_history` 参数契约（§3.3 #59）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct LoadSubagentHistoryArgs {
    pub session_id: String,
    pub sub_id: String,
}

/// `load_subagent_history` 命令（§3.3 #59）。
#[tauri::command]
pub(crate) async fn load_subagent_history(
    app: State<'_, AppState>,
    session_id: String,
    sub_id: String,
) -> Result<Vec<MessageEntry>, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = LoadSubagentHistoryArgs { session_id, sub_id };
    load_subagent_history_core(&app, &args.session_id, &args.sub_id)
}

// 单元/集成测试挂载：mock SSE 全链场景（仅 cargo test 时编译）。
#[cfg(test)]
#[path = "subagent_test.rs"]
mod subagent_test;
