//! run 帧出口的装饰层（平移自旧壳 `host/commands/chat.rs`）。
//!
//! 装配链的帧路径 = [`EventfulSink`]（事件层：usage 记账 + RunState 同步 +
//! tokens:update + 工具起止日志行）包 [`PersistingSink`](crate::run_persistence::
//! PersistingSink)（canonical 落盘）包 [`HubSink`](crate::agent::HubSink)
//! （seq 递增 → 入环 → 下发）。各层职责单一，本文件只承载事件层与
//! 辅助日志实现。
//!
//! - [`HostLlmLogger`]：模型调用元数据/原文日志（脱敏闸红线在实现内）；
//! - [`HostAskLogger`]：问卷 ask 开关两行的会话日志落点；
//! - [`DetachedSink`]：自动投递新 run 的空下发端（帧只入环，前端经
//!   run:start 事件 attach 观察态续流）；
//! - [`record_aux_usage`]：辅助小调用（suggest/压缩/审计）的会话记账。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rig::completion::request::CompletionRequest;

use crate::agent::RunHandle;
use crate::backend::frames::{ChatFrame, FrameError, FrameSink, SequencedFrame, SequencedSink, UsageTotal};
use crate::session_log::{LogKind, SessionLogger};
use crate::usage::UsageLedger;
use crate::util::errors::E_INTERNAL;
use crate::util::events::{EventSink, keys};

/// 当前时间的 Unix 毫秒时间戳（取不到时兜底 0）。
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// usage 帧的 run 累计差分（UsageTotal 无 Sub）。
pub(crate) fn sub_usage(a: &UsageTotal, b: &UsageTotal) -> UsageTotal {
    UsageTotal {
        input: a.input.saturating_sub(b.input),
        output: a.output.saturating_sub(b.output),
        cache_read: a.cache_read.saturating_sub(b.cache_read),
        cache_write: a.cache_write.saturating_sub(b.cache_write),
    }
}

/// 发送错误终态帧（RunLoop 冒出的 PromptError 兜底，保证前端必有终态）。
/// dev 构建同时打到进程终端，便于排障。
pub(crate) fn send_error_ended(sink: &mut dyn FrameSink, message: String) {
    #[cfg(debug_assertions)]
    eprintln!("[craftwave] run 错误终态：{message}");
    sink.send(ChatFrame::RunEnded {
        reason: crate::backend::frames::EndReason::Error,
        duration_ms: 0,
        tool_count: 0,
        turn: 1,
        error: Some(FrameError::new(E_INTERNAL, message)),
    });
}

/// 帧出口的事件层：usage 帧增量折算 → 会话累计 →
/// `tokens:update` 全量校正事件（sessionUsage 以事件为准）。
/// usage 帧载荷是 run 内累计值，须减去上一帧的 run 累计得本轮增量。
/// 同一处同步 RunHandle 的运行时状态（RunState）——turn 取
/// stepStarted 帧、工具计数按收到的工具结果帧递增、usage 取 usage 帧的
/// 运行累计载荷；`session_run_snapshot` / attach 摘要帧据此对账。
/// usage 差分增量同步落账（日/模型聚合 + run/会话明细 +
/// stats_cache 写穿）——记账日取当时本地日期；主 run 首个
/// 有计量的帧计当日该模型 runs 一次；模型身份取 `current_model`
/// 槽（每拍刷新，run 内切模型按拍真实归属）。
/// 非泛型：本层只消费帧与共享句柄，不触及模型驱动类型（同旧壳）。
pub(crate) struct EventfulSink {
    pub(crate) inner: Box<dyn FrameSink>,
    pub(crate) events: Arc<dyn EventSink>,
    pub(crate) session_id: String,
    /// 本 sink 所属的 run id（明细落账键）。
    pub(crate) run_id: String,
    pub(crate) last_run_total: UsageTotal,
    pub(crate) session_usage: Arc<Mutex<HashMap<String, UsageTotal>>>,
    /// 所属 run 的句柄（运行时状态同步目标；测试直驱路径可为 None）。
    pub(crate) handle: Option<Arc<RunHandle>>,
    /// 用量账本（AppState.ledger 的克隆件——spawn 闭包内无整体）。
    pub(crate) ledger: UsageLedger,
    /// 当前生效模型槽 `(配置 id, 显示名)`：run 装配时置初值，
    /// 每拍刷新（模型切换时更新）——Usage 帧落账时
    /// 读到的即「本拍实际执行模型」（真实归属）。
    pub(crate) current_model: super::model::ModelSlot,
    /// 本 run 是否已计当日该模型的 runs 次数（首个有计量的 usage 帧计一次；
    /// 无计量不伪造）。
    pub(crate) run_counted: bool,
    /// 会话日志器（工具起止行的落点；测试直驱路径 None）。
    pub(crate) session_log: Option<Arc<SessionLogger>>,
    /// 在飞工具起点表（ToolStarted 记名与时刻，ToolResult 收口
    /// 算耗时落 tool 行——帧面无耗时字段，宿主侧配对补齐）。
    pub(crate) tool_starts: HashMap<String, (String, std::time::Instant)>,
}

impl EventfulSink {
    /// 把本轮增量累进共享的会话用量表，并发出全量快照事件
    /// （前端只认事件里的全量值，不自己加总）。`model_id` 随载荷带出
    /// （前端统计区块当日/该模型本地增量修正的依据）。
    fn send_tokens_update(&mut self, delta: &UsageTotal, model_id: &str) {
        let mut map = self
            .session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let total = map.entry(self.session_id.clone()).or_default();
        total.accumulate(delta);
        let snapshot = *total;
        let mut payload = serde_json::json!({ "sessionId": self.session_id, "usage": snapshot });
        if !model_id.is_empty() {
            payload["modelId"] = serde_json::json!(model_id);
        }
        self.events.emit(keys::TOKENS_UPDATE, payload);
    }

    /// usage 差分增量落账（明细/聚合/缓存三面）。`counts_run` = 本 run
    /// 尚未计次且本次有真实计量（主 run 计一次，无计量不伪造）。
    fn record_stats(&mut self, delta: &UsageTotal) {
        if *delta == UsageTotal::default() {
            return; // 零计量不落账（未知用量不伪造）
        }
        let (model_id, model_name) = self
            .current_model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_default();
        let counts_run = !self.run_counted;
        self.run_counted = true;
        self.ledger.record(
            &self.session_id,
            &self.run_id,
            crate::usage::run_kind::MAIN,
            &model_id,
            &model_name,
            *delta,
            counts_run,
        );
    }

    /// 把帧面的运行时进度同步进 RunHandle 的 RunState（对账数据源）。
    /// tool_count 口径 = 已收到的工具结果帧数（含硬拦/取消的补发结果帧，
    /// 与 RunLoop 的 DriveState 计数同面；审批拒绝的补发结果帧也计数——
    /// 语义为"该调用已被处理"）。
    fn sync_run_state(&self, frame: &ChatFrame, run_total: Option<UsageTotal>) {
        let Some(handle) = &self.handle else {
            return;
        };
        let mut state = handle
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match frame {
            ChatFrame::StepStarted { turn } => state.turn = *turn,
            ChatFrame::ToolResult { .. } => state.tool_count = state.tool_count.saturating_add(1),
            ChatFrame::Usage { .. } => {
                if let Some(total) = run_total {
                    state.usage = total;
                }
            }
            _ => {}
        }
    }
}

impl FrameSink for EventfulSink {
    fn send(&mut self, frame: ChatFrame) {
        if let ChatFrame::Usage { usage, .. } = &frame {
            let delta = sub_usage(usage, &self.last_run_total);
            self.last_run_total = *usage;
            // usage 帧载荷即运行累计值：先同步状态再发事件（快照与帧面一致）
            self.sync_run_state(&frame, Some(*usage));
            // 统计落账在事件之前（同帧同源：事件 modelId 与落账模型一致）
            self.record_stats(&delta);
            let model_id = self
                .current_model
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .map(|(id, _)| id)
                .unwrap_or_default();
            self.send_tokens_update(&delta, &model_id);
        } else {
            self.sync_run_state(&frame, None);
        }
        // 工具起止行：ToolStarted 记起点，ToolResult 收口算
        // 耗时（错误码随 error 面）；恢复重放批可能无 started（挂起前
        // 已发过）——耗时省略，行照记。
        match &frame {
            ChatFrame::ToolStarted { call_id, name } => {
                self.tool_starts
                    .insert(call_id.clone(), (name.clone(), std::time::Instant::now()));
            }
            ChatFrame::ToolResult {
                call_id,
                ok,
                error,
                ui_summary,
            } => {
                if let Some(logger) = &self.session_log {
                    let (name, elapsed_ms) = match self.tool_starts.remove(call_id) {
                        Some((name, started)) => (name, started.elapsed().as_millis() as u64),
                        // 无起点（审批挂起恢复重放 / 子代理帧等）：耗时未知
                        None => (String::new(), 0),
                    };
                    let code = error.as_ref().map(|e| e.code.clone());
                    let msg = if *ok {
                        format!("工具执行完成：{ui_summary}")
                    } else {
                        format!(
                            "工具执行失败：{}",
                            error
                                .as_ref()
                                .map(|e| e.message.clone())
                                .unwrap_or_else(|| ui_summary.clone())
                        )
                    };
                    logger.session_line(
                        &self.session_id,
                        if *ok {
                            crate::providers::types::LogLevel::Info
                        } else {
                            crate::providers::types::LogLevel::Warn
                        },
                        LogKind::Tool,
                        msg,
                        Some(serde_json::json!({
                            "name": name,
                            "ok": ok,
                            "durationMs": elapsed_ms,
                            "code": code,
                        })),
                        Some(&self.run_id),
                        Some(call_id),
                        None,
                    );
                }
            }
            _ => {}
        }
        self.inner.send(frame);
    }
}

/// 问卷类 ask 开关的日志实现（`backend::exec::AskLogSink`）：
/// 把 backend 的回调面落到会话日志 kind=approval 行
/// （问卷与审批卡共用同一 kind；askId 作 callId 锚）。主 run 与子代理
/// 环境的 ExecutorEnv 装配复用同一实现。
pub(crate) struct HostAskLogger {
    pub(crate) session_log: Arc<SessionLogger>,
}

impl crate::backend::exec::AskLogSink for HostAskLogger {
    fn ask_opened(&self, session_id: &str, ask_id: &str, run_id: &str) {
        self.session_log.session_line(
            session_id,
            crate::providers::types::LogLevel::Info,
            LogKind::Approval,
            "等待用户回答问卷".to_owned(),
            None,
            Some(run_id),
            Some(ask_id),
            None,
        );
    }

    fn ask_closed(&self, session_id: &str, ask_id: &str, run_id: &str, answered: bool) {
        self.session_log.session_line(
            session_id,
            crate::providers::types::LogLevel::Info,
            LogKind::Approval,
            if answered {
                "问卷已回答".to_owned()
            } else {
                "问卷未回答（取消）".to_owned()
            },
            Some(serde_json::json!({ "answered": answered })),
            Some(run_id),
            Some(ask_id),
            None,
        );
    }
}

/// 模型调用日志的宿主实现（[`TurnDriver`](crate::backend::turn::TurnDriver) 所挂的
/// `backend::turn::LlmLogSink`）：
/// - Info 级：每次收口记一行元数据（host/model/耗时/usage，不涉内容）；
/// - Debug 级（verbose）：追加请求/响应原文两行——**落盘前必须过脱敏
///   闸**（图片 base64 段摘要化为路径/哈希/格式/大小 + 密钥名单与
///   `Bearer`/`sk-` 兜底替换，红线），序列化本身也只在 verbose
///   才发生（大载荷成本不进常规路径）。
///
/// 图片路径口径：请求里的图片本体段替换为摘要后，宿主把
/// 会话 imgblob 目录传入摘要器——附件/粘贴图以内容寻址落在该目录，
/// 哈希命中即补真实 blob 路径；read 读图的工作区源路径在工具结果文本
/// 内（base64 段外的普通文本，不被替换、原样保留）。无来源的图片
/// 不虚构路径。
pub(crate) struct HostLlmLogger {
    pub(crate) session_log: Arc<SessionLogger>,
    pub(crate) session_id: String,
    pub(crate) run_id: String,
    /// API 端点 host（resolve 时快照的 baseUrl 取 host 部分，诊断面）。
    pub(crate) api_host: String,
    /// 会话图片 blob 目录（摘要补路径用；内容寻址、仅实际存在才补）。
    pub(crate) blob_dir: std::path::PathBuf,
    /// 当前生效模型槽 `(配置 id, 显示名)`（与 refresher/sink 共享的同一
    /// 句柄——运行中切模型后日志读到的是本拍实际执行模型，同源）。
    pub(crate) current_model: super::model::ModelSlot,
}

impl HostLlmLogger {
    /// 从 baseUrl 提取 host 部分（`scheme://host[:port]/…` → `host[:port]`；
    /// 解析不出原样返回，诊断面不做强校验）。
    pub(crate) fn host_of(base_url: &str) -> String {
        let after_scheme = base_url.split("://").nth(1).unwrap_or(base_url);
        after_scheme
            .split(['/', '?', '#'])
            .next()
            .unwrap_or(base_url)
            .to_owned()
    }

    /// 当前模型显示名（槽未置位时回退配置 id 的空串占位）。
    fn model_label(&self) -> String {
        self.current_model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .map(|(_, name)| name)
            .unwrap_or_default()
    }

    /// usage 四口径折一行 JSON（None = 无计量，如取消路径）。
    fn usage_json(usage: Option<&rig::completion::Usage>) -> serde_json::Value {
        match usage {
            None => serde_json::Value::Null,
            Some(u) => serde_json::json!({
                "input": u.input_tokens.unwrap_or(0),
                "output": u.output_tokens.unwrap_or(0),
                "cacheRead": u.cached_input_tokens.unwrap_or(0),
                "cacheWrite": u.cache_creation_input_tokens.unwrap_or(0),
            }),
        }
    }
}

impl crate::backend::turn::LlmLogSink for HostLlmLogger {
    fn on_request(&self, turn: u32, request: &CompletionRequest) {
        // verbose 才序列化（级别检查在 verbose()，防大载荷进常规路径）
        if !self.session_log.verbose() {
            return;
        }
        let Ok(mut raw) = serde_json::to_value(request) else {
            return;
        };
        // 脱敏闸（红线）：先图片本体摘要化（补真实 blob 路径），
        // 再密钥名单与 Bearer/sk- 兜底
        crate::util::redact::summarize_images(&mut raw, Some(&self.blob_dir));
        crate::util::redact::redact_json(&mut raw);
        self.session_log.session_line(
            &self.session_id,
            crate::providers::types::LogLevel::Debug,
            LogKind::Llm,
            if turn == 0 {
                "模型请求原文（辅助调用，已脱敏，图片仅路径/哈希/格式/大小）".to_owned()
            } else {
                "模型请求原文（已脱敏，图片仅路径/哈希/格式/大小）".to_owned()
            },
            Some(raw),
            Some(&self.run_id),
            None,
            None,
        );
    }

    fn on_response(
        &self,
        turn: u32,
        duration_ms: u64,
        usage: Option<&rig::completion::Usage>,
        response: Option<&rig::completion::request::CompletionResponse>,
        error: Option<&str>,
    ) {
        let meta = || {
            serde_json::json!({
                "host": self.api_host,
                "model": self.model_label(),
                "durationMs": duration_ms,
                "usage": Self::usage_json(usage),
            })
        };
        match error {
            // 取消：Info 行（流被丢弃，无 usage 无响应体——不算失败）
            Some("cancelled") => {
                self.session_log.session_line(
                    &self.session_id,
                    crate::providers::types::LogLevel::Info,
                    LogKind::Llm,
                    "模型请求被取消".to_owned(),
                    Some(meta()),
                    Some(&self.run_id),
                    None,
                    None,
                );
            }
            // 失败：Warn 行，失败说明过字符串兜底防密钥经错误消息漏出；
            // 失败无响应体，Debug 级不追加
            Some(e) => {
                let safe = crate::util::redact::redact_string(e)
                    .unwrap_or_else(|| (*e).to_owned());
                self.session_log.session_line(
                    &self.session_id,
                    crate::providers::types::LogLevel::Warn,
                    LogKind::Llm,
                    format!("模型请求失败：{safe}"),
                    Some(meta()),
                    Some(&self.run_id),
                    None,
                    None,
                );
            }
            // 正常收口：Info 行
            None => {
                let msg = if turn == 0 {
                    "模型辅助调用完成".to_owned()
                } else {
                    "模型请求完成".to_owned()
                };
                self.session_log.session_line(
                    &self.session_id,
                    crate::providers::types::LogLevel::Info,
                    LogKind::Llm,
                    msg,
                    Some(meta()),
                    Some(&self.run_id),
                    None,
                    None,
                );
            }
        }
        // Debug 级追加响应原文（脱敏闸同请求面；取消/失败路径 response=None）
        if let (Some(response), true) = (response, self.session_log.verbose()) {
            let Ok(mut raw) = serde_json::to_value(response) else {
                return;
            };
            crate::util::redact::summarize_images(&mut raw, Some(&self.blob_dir));
            crate::util::redact::redact_json(&mut raw);
            self.session_log.session_line(
                &self.session_id,
                crate::providers::types::LogLevel::Debug,
                LogKind::Llm,
                "模型响应原文（已脱敏）".to_owned(),
                Some(raw),
                Some(&self.run_id),
                None,
                None,
            );
        }
    }
}

/// 自动投递新 run 的帧出口占位：自动投递时后端没有前端
/// 通道——帧枢纽以本空实现起手（帧只入环形缓冲不下发），前端收到
/// `run:start` 事件后经 attach_run（观察态）以新通道重放续流。
pub(crate) struct DetachedSink;

impl SequencedSink for DetachedSink {
    fn send_seq(&mut self, _frame: SequencedFrame) {}
}

/// 辅助小调用的会话记账（suggest 与 审计与压缩共用）：已知实际用量
/// 落账（日/模型聚合 + 明细 kind=aux + 缓存写穿；runs 计数不增）+
/// tokens:update 全量校正事件。零计量不落账（未知用量不伪造）。
pub(crate) fn record_aux_usage(
    events: &Arc<dyn EventSink>,
    session_usage: &Mutex<HashMap<String, UsageTotal>>,
    ledger: &UsageLedger,
    session_id: &str,
    aux_run_id: &str,
    model: &(String, String),
    delta: &UsageTotal,
) {
    if *delta == UsageTotal::default() {
        return;
    }
    ledger.record(
        session_id,
        aux_run_id,
        crate::usage::run_kind::AUX,
        &model.0,
        &model.1,
        *delta,
        false,
    );
    let total = {
        let mut map = session_usage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let total = map.entry(session_id.to_owned()).or_default();
        total.accumulate(delta);
        *total
    };
    let mut payload = serde_json::json!({ "sessionId": session_id, "usage": total });
    if !model.0.is_empty() {
        payload["modelId"] = serde_json::json!(model.0);
    }
    events.emit(keys::TOKENS_UPDATE, payload);
}
