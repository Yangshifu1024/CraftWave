//! 审批挂起恢复循环（平移自旧壳 `host/commands/chat.rs`）。
//!
//! RunExit::Suspended 后的宿主职责：
//! - [`build_approval_payload`]：挂起调用 → AskPayload（计划范围卡 /
//!   外部目录授权卡 / MCP 审批卡 / 通用审批卡四形态）；
//! - [`wait_approval_with`]：注册 AskSlot + ask:opened → 等
//!   `resolve_ask` / 取消 / autoConfirm 300s → 统一清槽 + ask:closed；
//! - [`run_command_audit`]：灰区命令审计编排（证据收集 + 单次无工具
//!   小调用 + 15s 总超时）；allow/block/ask 三态结论的帧面与收场在
//!   run 任务（`super::run_task`）分流；
//! - [`denied_finish`]：拒绝/取消的收场（补拒绝结果帧 + runEnded(cancelled)）。

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::agent::RunHandle;
use crate::app_state::AppState;
use crate::backend::ask::{AskAnswer, AskMode, AskOption, AskPayload, AskSlot, AuditNote};
use crate::backend::exec::AskSlotCell;
use crate::backend::frames::{ChatFrame, EndReason, FrameError, FrameSink, UsageTotal};
use crate::backend::audit::{AuditPromptInput, EvidenceCollector, build_audit_prompt, parse_verdict, bad_output_reason};
use crate::backend::run_loop::{DriveState, E_CANCELLED, PendingCall};
use crate::backend::turn::{TurnDriver, usage_total};
use crate::session_log::{LogKind, SessionLogger};
use crate::util::errors::E_AUDIT_BLOCKED;
use crate::util::events::{EventSink, keys};

use super::model::{ModelResolver, ResolvedModel, RunTransport, RunWire};

/// autoConfirm 自动放行等待（300s 超时自动选 recommended 项）。
pub(crate) const AUTO_CONFIRM_TIMEOUT: Duration = Duration::from_secs(300);

/// 审计总等待上限：含证据收集与模型请求，固定
/// 15 秒，不可配置。
pub(crate) const AUDIT_TOTAL_TIMEOUT: Duration = Duration::from_secs(15);
/// 审计回复上限：一句简短中文依据足够，防模型失控长输出烧钱。
pub(crate) const AUDIT_MAX_TOKENS: u64 = 256;

/// 审批挂起 → AskPayload(kind=approval)：write=新文件全 add、edit=替换 hunk、
/// command=命令行 + fenceReason。`session_memory_enabled`
/// （会话内记忆启用）为 true 且待批调用是命令类时，选项增「始终允许
/// （本会话）」（mode=always_session）——选中后同签名命令本会话跨 run 免批
/// （记忆在会话级审批记忆桶，不持久化）。`plan_scope_note`
/// 为已批准计划基线的人话摘要，范围门挂起（pending.plan_scope）时随范围卡
/// 展示对照；其他形态传 None。`audit_note`为灰区命令审计
/// 结论注记（ask/manual），随普通审批卡展示（范围门/外部目录/MCP 等非灰区
/// 卡不带）；allow/block 不产生审批卡，不经本参数承载。
pub(crate) fn build_approval_payload(
    pending: &PendingCall,
    session_memory_enabled: bool,
    plan_scope_note: Option<&str>,
    audit_note: Option<&AuditNote>,
) -> AskPayload {
    // 计划范围卡：计划外写路径 + 批准基线对照 +
    // 三选项（允许一次 / 本会话始终允许（父目录入基线）/ 拒绝）。无推荐项
    // ——不参与 autoConfirm 自动批准（计划范围须用户明确同意，与外部目录
    // 授权同精神）；command 场景附命令行原文（用户需知道执行什么）；围栏
    // Confirm 待决时一并附上（门判定层吸收——一次交互覆盖本次调用的全部
    // 审批）。
    if let Some(path) = &pending.plan_scope {
        let mut reason = format!("写入路径「{path}」不在已批准计划的范围内");
        if let Some(note) = plan_scope_note {
            reason.push_str(&format!("（批准范围：{note}）"));
        }
        if let Some(fence) = &pending.fence_reason {
            reason.push_str(&format!("；{fence}"));
        }
        let command = if pending.name == "command" {
            serde_json::from_str::<serde_json::Value>(&pending.args_json)
                .ok()
                .and_then(|v| v.get("command").and_then(|c| c.as_str()).map(str::to_owned))
        } else {
            None
        };
        return AskPayload::Approval {
            title: "计划范围确认".to_owned(),
            diff: None,
            command,
            fence_reason: Some(reason),
            preview_html: None,
            plan_scope: Some(path.clone()),
            audit: None,
            options: vec![
                AskOption {
                    id: "allow_once".to_owned(),
                    label: "允许（仅本次）".to_owned(),
                    recommended: None,
                    mode: Some(AskMode::AllowOnce),
                },
                AskOption {
                    id: "always_session".to_owned(),
                    label: "本会话始终允许（写入所在目录）".to_owned(),
                    recommended: None,
                    mode: Some(AskMode::AlwaysSession),
                },
                AskOption {
                    id: "deny".to_owned(),
                    label: "拒绝".to_owned(),
                    recommended: None,
                    mode: Some(AskMode::Deny),
                },
            ],
        };
    }
    // 外部目录授权卡：路径 + 粒度说明 + 允许/拒绝。授权
    // 仅本会话有效（会话 roots 快照），无 recommended——不参与 autoConfirm。
    if let Some(dir) = &pending.external_dir {
        return AskPayload::ExternalDir {
            path: dir.clone(),
            options: vec![
                AskOption {
                    id: "allow".to_owned(),
                    label: "始终允许（本会话）".to_owned(),
                    recommended: None,
                    mode: None,
                },
                AskOption {
                    id: "deny".to_owned(),
                    label: "拒绝".to_owned(),
                    recommended: None,
                    mode: Some(AskMode::Deny),
                },
            ],
        };
    }
    let args: serde_json::Value = serde_json::from_str(&pending.args_json).unwrap_or_default();
    // MCP 动态工具分支：title 带服务器名与工具名；
    // 选项含「始终允许」（持久许可的授予入口；恢复循环
    // 侧写 trust/mcp-tools.json）。MCP 审批无 recommended——不参与
    // autoConfirm 自动批准。归一名反查失败（极窄窗口：断开已摘除动态段）
    // 落到下方通用审批标题兜底。
    if let Some(identity) = crate::mcp::tools::McpToolRegistry::global().identity(&pending.name) {
        return AskPayload::Approval {
            title: format!(
                "MCP 调用审批：服务器 {server} 的工具 {tool}",
                server = identity.server_name,
                tool = identity.original_name
            ),
            diff: None,
            command: Some(format!("{} {}", pending.name, pending.args_json)),
            fence_reason: pending.fence_reason.clone(),
            preview_html: None,
            plan_scope: None,
            audit: None,
            options: vec![
                AskOption {
                    id: "allow_once".to_owned(),
                    label: "仅本次允许".to_owned(),
                    recommended: None,
                    mode: Some(AskMode::AllowOnce),
                },
                AskOption {
                    id: "always_allow".to_owned(),
                    label: "始终允许（当前版本）".to_owned(),
                    recommended: None,
                    mode: None,
                },
                AskOption {
                    id: "deny".to_owned(),
                    label: "拒绝".to_owned(),
                    recommended: None,
                    mode: Some(AskMode::Deny),
                },
            ],
        };
    }
    let title = match pending.name.as_str() {
        "write" => format!(
            "写入新文件 {}",
            args.get("path").and_then(|v| v.as_str()).unwrap_or("?")
        ),
        "edit" => format!(
            "编辑 {}",
            args.get("path").and_then(|v| v.as_str()).unwrap_or("?")
        ),
        "command" => "执行命令".to_owned(),
        // web 工具审批（Network 行）：标题承载方法 + URL 摘要行
        "web_fetch" => "抓取网页".to_owned(),
        "http_request" => "发送 HTTP 请求".to_owned(),
        other => format!("执行工具 {other}"),
    };
    let mut diff = None;
    let mut command = None;
    // HTML 预览附带：write/edit 目标扩展名为 .html/.htm
    // 且内容 ≤50,000 字符时，把产出全文附进审批载荷——审批卡「查看预览」
    // 以 render_html 同款沙箱渲染。超限/其他扩展名不附带
    // （载荷不膨胀，diff 仍是审批主预览面）。
    let mut preview_html = None;
    let html_target = |path: &str| {
        matches!(
            std::path::Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("html" | "htm")
        )
    };
    match pending.name.as_str() {
        "write" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if html_target(path)
                && content.chars().count() <= crate::tools::render_html::MAX_HTML_CHARS
            {
                preview_html = Some(content.to_owned());
            }
            diff = Some(crate::backend::ask::ApprovalDiff {
                path: path.to_owned(),
                hunks: vec![crate::backend::ask::DiffHunk {
                    header: "新文件".to_owned(),
                    lines: content
                        .lines()
                        .map(|l| crate::backend::ask::DiffLine {
                            kind: crate::backend::ask::DiffLineKind::Add,
                            text: l.to_owned(),
                        })
                        .collect(),
                }],
            });
        }
        "edit" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let old = args.get("oldText").and_then(|v| v.as_str()).unwrap_or("");
            let new = args.get("newText").and_then(|v| v.as_str()).unwrap_or("");
            if html_target(path)
                && new.chars().count() <= crate::tools::render_html::MAX_HTML_CHARS
            {
                // 预览语义 = 编辑后的效果：附替换后全文（newText）。
                preview_html = Some(new.to_owned());
            }
            let mut lines = Vec::new();
            for l in old.lines() {
                lines.push(crate::backend::ask::DiffLine {
                    kind: crate::backend::ask::DiffLineKind::Del,
                    text: l.to_owned(),
                });
            }
            for l in new.lines() {
                lines.push(crate::backend::ask::DiffLine {
                    kind: crate::backend::ask::DiffLineKind::Add,
                    text: l.to_owned(),
                });
            }
            diff = Some(crate::backend::ask::ApprovalDiff {
                path: path.to_owned(),
                hunks: vec![crate::backend::ask::DiffHunk {
                    header: format!(
                        "@@ -{del} +{add} @@",
                        del = old.lines().count(),
                        add = new.lines().count()
                    ),
                    lines,
                }],
            });
        }
        "command" => {
            command = args
                .get("command")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
        }
        // web 工具：command 字段承载 `GET https://…` 摘要行
        // （审批弹窗复用命令行展示位，AskPanel 零改动）
        "web_fetch" => {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            command = Some(format!("GET {url}"));
        }
        "http_request" => {
            let method = args
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("GET")
                .to_ascii_uppercase();
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            command = Some(format!("{method} {url}"));
        }
        _ => {}
    }
    // 通用审批选项：允许（推荐，autoConfirm 超时选它）+ 拒绝；命令类审批
    // 且 ③ 会话内记忆开启时插「始终允许（本会话）」。
    // 非推荐项——autoConfirm 只自动选 allow_once，会话级免批必须用户显式点。
    let mut options = vec![AskOption {
        id: "allow_once".to_owned(),
        label: "允许".to_owned(),
        recommended: Some(true),
        mode: Some(AskMode::AllowOnce),
    }];
    if session_memory_enabled && pending.name == "command" {
        options.push(AskOption {
            id: "always_session".to_owned(),
            label: "始终允许（本会话）".to_owned(),
            recommended: None,
            mode: Some(AskMode::AlwaysSession),
        });
    }
    options.push(AskOption {
        id: "deny".to_owned(),
        label: "拒绝".to_owned(),
        recommended: None,
        mode: Some(AskMode::Deny),
    });
    AskPayload::Approval {
        title,
        diff,
        command,
        fence_reason: pending.fence_reason.clone(),
        preview_html,
        plan_scope: None,
        audit: audit_note.cloned(),
        options,
    }
}

/// 挂起等待的裁决（option_id 供 MCP「始终允许」的持久许可授予面，
/// always_session 供会话级免批登记面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ApprovalResolution {
    /// 批准（option_id：allow_once / always_session / always_allow /
    /// autoConfirm 的 recommended 项）。
    Allowed { option_id: String },
    /// 拒绝/取消。
    Denied,
}

/// 挂起等待审批：注册 AskSlot + ask:opened → 等 resolve_ask / 取消 /
/// autoConfirm 超时。等待结束统一清槽 + ask:closed。
/// 主 run 与子代理共用：取消令牌与询问槽由调用方注入
/// （主 run = RunHandle.cancel/ask；子代理 = SubHandle.cancel/ask）。
/// `session_memory_enabled`：true 时命令类审批载荷增
/// 「始终允许（本会话）」选项。`plan_scope_note`：范围门
/// 挂起时随载荷展示的批准基线对照摘要（宿主从共享计划状态拼装）。
/// `audit_note`：灰区命令审计结论注记（ask/manual），
/// 随普通审批卡展示；None = 未经审计或审计已放行/拒绝（不产生审批卡）。
/// `session_log`：审批开关两行的落点（kind=approval：opened/
/// closed + answered）；None = 测试直驱路径不记。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn wait_approval_with(
    events: &Arc<dyn EventSink>,
    session_id: &str,
    run_id: &str,
    cancel: &CancellationToken,
    ask_slot: &Arc<AskSlotCell>,
    pending: &PendingCall,
    auto_confirm: bool,
    session_memory_enabled: bool,
    plan_scope_note: Option<&str>,
    audit_note: Option<&AuditNote>,
    session_log: Option<&Arc<SessionLogger>>,
) -> ApprovalResolution {
    let (tx, rx) = tokio::sync::oneshot::channel::<AskAnswer>();
    let ask_id = uuid::Uuid::new_v4().to_string();
    let payload =
        build_approval_payload(pending, session_memory_enabled, plan_scope_note, audit_note);
    {
        let mut slot = ask_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 同一时刻只允许一个未决询问：槽位已被占用则直接拒绝（防御性）
        if slot.is_some() {
            return ApprovalResolution::Denied;
        }
        *slot = Some(AskSlot {
            ask_id: ask_id.clone(),
            session_id: session_id.to_owned(),
            run_id: run_id.to_owned(),
            kind: "approval",
            // 原始载荷留存：重连后的待审批恢复查询据此展示
            // 原始操作及参数（diff/命令/围栏原因与选项）
            payload: payload.clone(),
            tx,
        });
    }
    events.emit(
        keys::ASK_OPENED,
        serde_json::json!({
            "sessionId": session_id,
            "askId": ask_id,
            "payload": payload,
        }),
    );
    // 审批打开行（kind=approval：askId + 操作名；载荷内容不进
    // 日志——参数里可能含正文，审批详情走帧面与审批卡）
    if let Some(logger) = session_log {
        let msg = format!("等待审批：{}", pending.name);
        logger.session_line(
            session_id,
            crate::providers::types::LogLevel::Info,
            LogKind::Approval,
            msg,
            Some(serde_json::json!({ "tool": pending.name })),
            Some(run_id),
            Some(&ask_id),
            None,
        );
    }

    // 挂起的三种醒来方式（select 多路等待的结果归一化）
    enum Wait {
        /// 前端 resolve_ask 送回了应答（None 表示发送端已丢弃）。
        Answer(Option<AskAnswer>),
        /// 用户点了取消（run 的取消令牌触发）。
        Cancelled,
        /// autoConfirm 开启且等满 300 秒无人回应。
        AutoConfirm,
    }
    let wait = tokio::select! {
        biased;
        _ = cancel.cancelled() => Wait::Cancelled,
        // autoConfirm 300s 只对「有推荐项（allow_once）」的载荷生效——
        // mcpTrust 等无 recommended 的信任询问不参与自动批准
        // （高危外部调用永远过人）。
        _ = tokio::time::sleep(AUTO_CONFIRM_TIMEOUT),
            if auto_confirm && payload.recommended_option_id().is_some() => Wait::AutoConfirm,
        answer = rx => Wait::Answer(answer.ok()),
    };
    let (resolution, answered) = match wait {
        Wait::Answer(Some(AskAnswer::Approval { option_id, .. })) => {
            // allow_once / always_session（本会话免批）/
            // always_allow（MCP）/ allow（外部目录授权）
            // 都放行本次；其余选项按拒绝收场
            let allowed = option_id == "allow_once"
                || option_id == "always_session"
                || option_id == "always_allow"
                || option_id == "allow";
            (
                if allowed {
                    ApprovalResolution::Allowed { option_id }
                } else {
                    ApprovalResolution::Denied
                },
                true,
            )
        }
        // 非审批形态应答（问卷）按拒绝收场
        Wait::Answer(Some(_)) => (ApprovalResolution::Denied, true),
        // 发送端已 drop：run 侧已收口，无需再发终态（返回 Denied 由循环兜底）
        Wait::Answer(None) => (ApprovalResolution::Denied, true),
        Wait::Cancelled => (ApprovalResolution::Denied, false),
        // autoConfirm：300s 自动选 recommended（allow_once）——挂起必非黑名单
        // （Block 不会挂起），灾难级例外天然满足；无 recommended 的
        // 载荷（mcpTrust/MCP 审批）已由 select 条件排除，不会走到这里
        Wait::AutoConfirm => (
            ApprovalResolution::Allowed {
                option_id: "allow_once".to_owned(),
            },
            true,
        ),
    };
    // 单一出口清槽 + ask:closed（resolve_ask 只投递不清理）
    *ask_slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    events.emit(
        keys::ASK_CLOSED,
        serde_json::json!({ "askId": ask_id, "answered": answered }),
    );
    // 审批收口行（kind=approval：answered + 裁决摘要）
    if let Some(logger) = session_log {
        let outcome = match &resolution {
            ApprovalResolution::Allowed { option_id } => format!("已批准（{option_id}）"),
            ApprovalResolution::Denied if answered => "已拒绝".to_owned(),
            ApprovalResolution::Denied => "未回答（取消）".to_owned(),
        };
        let msg = format!("审批收口：{outcome}");
        logger.session_line(
            session_id,
            crate::providers::types::LogLevel::Info,
            LogKind::Approval,
            msg,
            Some(serde_json::json!({ "answered": answered, "outcome": outcome })),
            Some(run_id),
            Some(&ask_id),
            None,
        );
    }
    resolution
}

/// 拒绝/取消的收场：挂起调用补拒绝结果帧 + runEnded(cancelled)（
/// AwaitingApproval → Cancelled；run 已序列化，模型侧不再收下一轮，帧面
/// 补 E_CANCELLED 结果让前端工具卡不悬空）。主 run 与子代理
/// 共用（DriveState 自带帧序映射与计数，无 run 句柄依赖）。
pub(crate) fn denied_finish(sink: &mut dyn FrameSink, pending: &PendingCall, state: &DriveState) {
    let frame_call_id = state
        .model_call_ids
        .get(&pending.model_call_id)
        .cloned()
        .unwrap_or_else(|| pending.model_call_id.clone());
    sink.send(ChatFrame::ToolResult {
        call_id: frame_call_id,
        ok: false,
        ui_summary: "用户已拒绝".to_owned(),
        error: Some(FrameError::new(E_CANCELLED, "用户拒绝了此操作")),
    });
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Cancelled,
        duration_ms: state
            .started_at
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0),
        tool_count: state.tool_count,
        turn: state.last_turn,
        error: None,
    });
}

/// 一次审计尝试的产物（用量记账面）：已获得的用量与执行审计的模型。
/// `usage` = None 表示模型请求未返回（超时/请求失败），不伪造未知用量。
pub(crate) struct AuditAttempt {
    pub(crate) usage: Option<UsageTotal>,
    pub(crate) model: (String, String),
}

/// 审计流程的退出形态。
pub(crate) enum AuditFlow {
    /// 模型给出合法三态结论。
    Verdict(crate::backend::audit::AuditVerdict, AuditAttempt),
    /// 审计失败/超时/输出无效——转人工（reason = 失败原因，审批卡展示）。
    Failed(String, AuditAttempt),
    /// 用户取消（run 令牌触发），审计终止。
    Cancelled,
}

/// 灰区命令审计编排（字段级）：模型解析 → 证据收集（会话
/// roots 内只读）→ 提示词构建 → 单次无工具调用（与 15 秒总超时、run 取消
/// 令牌 select! 竞争，超时丢弃在飞请求不留后台消耗）。调用前提：宿主已用
/// `ApprovalGate::audit_class` 判定挂起项属灰区。审计执行（本函数）不判断
/// 结论去向——放行/拒绝/转人工由宿主恢复循环分流。
///
/// `session_model_id` = 会话当前模型槽的配置 id（`commandAudit.modelId`
/// 未配置时跟随）；`plan_baseline` = 已批准计划基线路径（批准范围证据）；
/// `total_timeout` 传入 [`AUDIT_TOTAL_TIMEOUT`]（测试可传短超时）。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_command_audit<W: RunWire, T: RunTransport<W>>(
    app: &AppState,
    resolve: ModelResolver<W, T>,
    session_id: &str,
    pending: &PendingCall,
    session_model_id: Option<&str>,
    user_goal: &str,
    plan_baseline: &[String],
    cancel: &CancellationToken,
    total_timeout: Duration,
) -> AuditFlow {
    // ① 模型解析：专用审计模型优先；未配置跟随会话当前模型。
    //    指定模型解析失败按审计失败转人工并显示原因，不静默换供应商。
    let audit_model_id = app
        .config
        .load()
        .ok()
        .and_then(|c| {
            c.agent
                .as_ref()
                .and_then(|a| a.command_audit.as_ref())
                .and_then(|cmd| cmd.model_id.clone())
        });
    let model_id = audit_model_id.or_else(|| session_model_id.map(str::to_owned));
    let resolved: ResolvedModel<W, T> = match model_id.as_deref() {
        Some(id) => match resolve(app, Some(id)) {
            Ok(m) => m,
            Err(e) => {
                return AuditFlow::Failed(
                    format!("审计模型不可用（未静默换供应商）：{e}"),
                    AuditAttempt {
                        usage: None,
                        model: (String::new(), String::new()),
                    },
                );
            }
        },
        None => {
            return AuditFlow::Failed(
                "审计模型不可用：未配置审计模型且会话无当前模型".to_owned(),
                AuditAttempt {
                    usage: None,
                    model: (String::new(), String::new()),
                },
            );
        }
    };
    let attempt_model = (resolved.id.clone(), resolved.name.clone());

    // ② 证据收集（授权目录内只读——AST 路径目标片段 + 命令定义 +
    //    批准范围；cwd 基准与审批门 judge_base 同款）。
    let args: serde_json::Value = serde_json::from_str(&pending.args_json).unwrap_or_default();
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let roots = crate::session_roots::resolve_session_roots(app, session_id);
    // cwd 基准先解析（项目主目录为缺省基准，与审批门 judge_base 同款），
    // 再把根集合收进归一列表（roots 字段被移动前的克隆面）。
    let project_root = roots.project_root.clone();
    let cwd_raw = args.get("cwd").and_then(|v| v.as_str());
    let base = match cwd_raw {
        Some(c) if !c.is_empty() => {
            let p = std::path::Path::new(c);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                project_root.join(p)
            }
        }
        _ => project_root.clone(),
    };
    let root_paths: Vec<std::path::PathBuf> = [project_root, roots.workspace_root]
        .into_iter()
        .chain(roots.extra_dirs)
        .collect();
    // 判定同域归一（审批门装配同款）：canonicalize 失败的根跳过。
    let canonical_roots: Vec<std::path::PathBuf> = root_paths
        .iter()
        .filter_map(|p| crate::safety::safe_join::canonicalize_best_effort(p).ok())
        .collect();
    let collector = EvidenceCollector {
        roots: &canonical_roots,
        base: &base,
    };
    let evidence = collector.collect(&command);
    let root_list = canonical_roots
        .iter()
        .map(|r| r.display().to_string())
        .collect::<Vec<_>>()
        .join("、");
    let scope_text = if plan_baseline.is_empty() {
        format!("已授权目录：{root_list}")
    } else {
        format!(
            "已批准计划范围：{}\n已授权目录：{root_list}",
            plan_baseline.join("、")
        )
    };
    let prompt = build_audit_prompt(&AuditPromptInput {
        command: &command,
        cwd: &base.display().to_string(),
        evidence: &evidence,
        user_goal: Some(user_goal),
        scope: Some(&scope_text),
    });

    // ③ 单次无工具调用：与总超时、取消令牌竞争（select 丢弃 future 即关闭
    //    在飞 HTTP 请求——超时不留后台审计继续消耗，取消传播终止模型请求）。
    let driver = Arc::new(TurnDriver::new(resolved.driver, rig::run::spec::RunSpec::new(), Vec::new()));
    let work = driver.complete_once_capped(&prompt, AUDIT_MAX_TOKENS);
    tokio::select! {
        biased;
        _ = cancel.cancelled() => AuditFlow::Cancelled,
        _ = tokio::time::sleep(total_timeout) => AuditFlow::Failed(
            format!(
                "审计超时：{} 秒总预算（含证据收集与模型请求）已到，转人工确认",
                total_timeout.as_secs().max(1)
            ),
            AuditAttempt { usage: None, model: attempt_model },
        ),
        result = work => match result {
            Ok((text, usage)) => {
                let attempt = AuditAttempt {
                    usage: Some(usage_total(&usage)),
                    model: attempt_model,
                };
                match parse_verdict(&text) {
                    Some(verdict) => AuditFlow::Verdict(verdict, attempt),
                    None => AuditFlow::Failed(bad_output_reason(&text), attempt),
                }
            }
            Err(e) => AuditFlow::Failed(
                format!("审计模型请求失败：{e}"),
                AuditAttempt { usage: None, model: attempt_model },
            ),
        },
    }
}

/// 审计拒绝的收场：E_AUDIT_BLOCKED 工具结果帧（附模型
/// 简短依据）+ runEnded(error)——followUp 队列按失败暂停（宿主侧由调用方
/// 记录 errored 与 RunExit::Failed）。
pub(crate) fn audit_block_finish(
    sink: &mut dyn FrameSink,
    frame_call_id: &str,
    state: &DriveState,
    reason: &str,
) {
    sink.send(ChatFrame::ToolResult {
        call_id: frame_call_id.to_owned(),
        ok: false,
        ui_summary: format!("审计拒绝：{reason}"),
        error: Some(FrameError::new(
            E_AUDIT_BLOCKED,
            format!("命令审计模型判断高风险，已拒绝执行：{reason}"),
        )),
    });
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Error,
        duration_ms: state
            .started_at
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0),
        tool_count: state.tool_count,
        turn: state.last_turn,
        error: Some(FrameError::new(
            E_AUDIT_BLOCKED,
            format!("命令审计模型判断高风险，已拒绝执行：{reason}"),
        )),
    });
}

/// 审计期间用户取消的收场（取消向挂起 run 传播）：E_CANCELLED 工具结果
/// 帧 + runEnded(cancelled)（与用户拒绝同收场形态，摘要区分审计场景）。
pub(crate) fn audit_cancelled_finish(sink: &mut dyn FrameSink, frame_call_id: &str, state: &DriveState) {
    sink.send(ChatFrame::ToolResult {
        call_id: frame_call_id.to_owned(),
        ok: false,
        ui_summary: "已取消（审计终止）".to_owned(),
        error: Some(FrameError::new(E_CANCELLED, "用户取消了此操作（审计终止）")),
    });
    sink.send(ChatFrame::RunEnded {
        reason: EndReason::Cancelled,
        duration_ms: state
            .started_at
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0),
        tool_count: state.tool_count,
        turn: state.last_turn,
        error: None,
    });
}

/// 范围卡基线对照摘要：已批准冻结基线的人话清单；未批准
/// 或基线为空返回 None（范围卡不带对照段——基线空时文案由载荷侧说明首写
/// 须确认）。
pub(crate) fn plan_scope_note_of(handle: &RunHandle) -> Option<String> {
    let plan = handle
        .plan
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let state = plan.as_ref()?;
    if !state.is_approved() || state.baseline_paths.is_empty() {
        return None;
    }
    Some(state.baseline_paths.join("、"))
}
