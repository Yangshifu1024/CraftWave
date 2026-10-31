//! run 生命周期落盘器（§7.3 时机表的帧观察实现，15 号票）。
//!
//! 形态：`FrameSink` 装饰器（与 chat.rs 的 `EventfulSink` 同款模式，零侵入
//! RunLoop）——帧流出口处聚合出 canonical 条目并即时落盘：
//!
//! | 帧事件 | 落盘动作（§7.3） |
//! |---|---|
//! | `usage`（turn 收口，节流 flush 后必发） | 追加该 turn 文本条目 `{runId}:{turn}` |
//! | `toolResult`（批内最后一个结果到达时） | 追加该 turn 工具条目 `{runId}:{turn}.tools` |
//! | `runEnded` | 追加 runMarker 条目 `{runId}:marker` + index upsert |
//!
//! user 条目不在本层：发送协议为「前端 append → save_session_tree（全树，
//! user 拓扑正确）→ start_chat」，start_chat 侧仅做幂等兜底。
//!
//! 条目 id 规则（两侧同源）：assistant 文本 `{runId}:{turn}`、工具
//! `{runId}:{turn}.tools`、runMarker `{runId}:marker`——前端可从
//! runStarted/stepStarted 帧同规则派生（save 载荷引用 canonical 节点时用）。
//!
//! 落盘失败记日志不中断 run（M1 取舍：帧面可用性优先；历史缺行在下次
//! load 的崩溃恢复扫描中补 interrupted 标记）。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::backend::frames::{ChatFrame, FrameSink, UsageTotal};
use crate::storage::index_store::SessionMeta;
use crate::storage::{
    CompleteReason, MessageEntry, MessageRole, MessageStatus, SessionStore, StoredEntry, WirePart,
};

/// 单 turn 的帧聚合缓冲（文本桶 + 工具桶分两次收口）。
#[derive(Default)]
struct TurnBuffer {
    /// 模型输出的正文文本（TextDelta 逐段拼接）。
    text: String,
    /// 模型的思考/推理文本（ReasoningDelta 逐段拼接）。
    reasoning: String,
    /// 本 turn 内发起的工具调用缓冲（等结果齐了再整体落盘）。
    calls: Vec<CallBuffer>,
}

/// 单个工具调用的聚合缓冲：参数分片拼满、结果到达后凑成一对
/// （ToolCall + ToolResult），随所在 turn 一起落盘。
#[derive(Default)]
struct CallBuffer {
    call_id: String,
    name: String,
    args: String,
    result: Option<(bool, String)>,
}

/// run 落盘器：聚合 + canonical 追加 + index 维护。
pub struct RunRecorder {
    store: Arc<SessionStore>,
    session_id: String,
    run_id: String,
    /// 应用日志通道（32 号票，§7.3 ⑤ 落盘失败记账：canonical/index 写失败
    /// 记 app 行不中断 run——帧面可用性优先，M1 取舍的记账面）。None =
    /// 测试装配。
    app_log: Option<Arc<crate::host::session_log::SessionLogger>>,
    started_at: Option<i64>,
    turns: BTreeMap<u32, TurnBuffer>,
    current_turn: u32,
    /// canonical 链尾（首条目的 parent = 史册 activeLeaf，此后为本 run 内链）。
    last_entry_id: Option<String>,
    usage: UsageTotal,
    last_revision: u64,
    last_message_count: u64,
}

/// §7.3 落盘装饰器：先记录再转发（帧序不变）。
pub struct PersistingSink {
    recorder: RunRecorder,
    inner: Box<dyn FrameSink>,
}

impl PersistingSink {
    /// 包装内层出口：recorder 先记账落盘，inner（Channel 或测试桩）后收帧。
    pub fn new(recorder: RunRecorder, inner: Box<dyn FrameSink>) -> Self {
        Self { recorder, inner }
    }
}

impl FrameSink for PersistingSink {
    fn send(&mut self, frame: ChatFrame) {
        self.recorder.record(&frame);
        self.inner.send(frame);
    }
}

impl RunRecorder {
    /// 构造时读一次史册 activeLeaf 作为 canonical 链尾（新会话 → None）。
    /// `app_log` = 应用日志通道（32 号票：落盘失败记账面；测试传 None）。
    pub fn new(
        store: Arc<SessionStore>,
        session_id: String,
        run_id: String,
        app_log: Option<Arc<crate::host::session_log::SessionLogger>>,
    ) -> Self {
        let last_entry_id = store
            .load_session_tree(&session_id)
            .ok()
            .and_then(|d| d.active_leaf_id);
        Self {
            store,
            session_id,
            run_id,
            app_log,
            started_at: None,
            turns: BTreeMap::new(),
            current_turn: 0,
            last_entry_id,
            usage: UsageTotal::default(),
            last_revision: 0,
            last_message_count: 0,
        }
    }

    /// 帧分发：只聚合不落盘，收口时机交给 usage / 工具批齐 / runEnded
    /// 三个触发点（对应下方三个 flush/finish 方法），即 §7.3 时机表。
    fn record(&mut self, frame: &ChatFrame) {
        match frame {
            ChatFrame::RunStarted { run_id, model: _ } => {
                if self.started_at.is_none() {
                    self.started_at = Some(now_ms());
                    let _ = run_id;
                }
            }
            ChatFrame::StepStarted { turn } => {
                // 防御：无 usage 收口的残留文本（异常帧序）在此兜底收口
                self.flush_text_turn(self.current_turn);
                self.current_turn = *turn;
            }
            ChatFrame::TextDelta { turn, delta } => {
                self.turns.entry(*turn).or_default().text.push_str(delta);
            }
            ChatFrame::ReasoningDelta { turn, delta } => {
                self.turns
                    .entry(*turn)
                    .or_default()
                    .reasoning
                    .push_str(delta);
            }
            ChatFrame::ToolStarted { call_id, name } => {
                self.turns
                    .entry(self.current_turn)
                    .or_default()
                    .calls
                    .push(CallBuffer {
                        call_id: call_id.clone(),
                        name: name.clone(),
                        args: String::new(),
                        result: None,
                    });
            }
            ChatFrame::ToolArgs { call_id, chunk } => {
                if let Some(call) = self
                    .turns
                    .get_mut(&self.current_turn)
                    .and_then(|t| t.calls.iter_mut().find(|c| c.call_id == *call_id))
                {
                    call.args.push_str(chunk);
                }
            }
            ChatFrame::ToolResult {
                call_id,
                ok,
                ui_summary,
                ..
            } => {
                if let Some(bucket) = self.turns.get_mut(&self.current_turn)
                    && let Some(call) = bucket.calls.iter_mut().find(|c| c.call_id == *call_id)
                {
                    call.result = Some((*ok, ui_summary.clone()));
                }
                // 批内全部结果收齐 → 逐 turn 落（§7.3 工具结果行）
                let all_settled = self
                    .turns
                    .get(&self.current_turn)
                    .map(|t| t.calls.iter().all(|c| c.result.is_some()))
                    .unwrap_or(false);
                if all_settled {
                    let turn = self.current_turn;
                    self.flush_tool_turn(turn);
                }
            }
            ChatFrame::Usage { usage, .. } => {
                self.usage = *usage;
                let turn = self.current_turn;
                self.flush_text_turn(turn);
            }
            // 压缩完成帧（04 号票）：通知面透传，不落 canonical 条目
            //（compaction 条目由压缩编排经 append_compaction 落盘，§7.3）
            ChatFrame::CompactDone { .. } => {}
            // runState 摘要帧（05 号票）：attach 对账辅助，无 canonical 产物
            ChatFrame::RunSnapshot { .. } => {}
            // 子代理 4 帧（10 号票）：子代理过程经 subFrame 在父 run 帧面
            // 透传，canonical 只记父 run 的 subagent 工具调用与结果本身
            //（不展开子帧），子代理历史独立落 histories/subs/（§5.9）
            ChatFrame::SubStarted { .. } => {}
            ChatFrame::SubFrame { .. } => {}
            ChatFrame::SubReport { .. } => {}
            ChatFrame::SubDone { .. } => {}
            // 审计状态帧（36 号票，§3.4）：通知面透传（前端工具卡审计行），
            // 无 canonical 产物——审计不是工具执行，命令条目仍由其 toolResult
            // 收口
            ChatFrame::ToolAudit { .. } => {}
            ChatFrame::RunEnded {
                duration_ms,
                tool_count,
                ..
            } => {
                // 防御兜底：未收口的残留桶
                let turns: Vec<u32> = self.turns.keys().cloned().collect();
                for turn in turns {
                    self.flush_text_turn(turn);
                    self.flush_tool_turn(turn);
                }
                self.finish(*duration_ms, *tool_count);
            }
        }
    }

    /// 文本 turn 收口（§7.3「turn 文本完成」行）：usage 帧触发。
    fn flush_text_turn(&mut self, turn: u32) {
        let Some(bucket) = self.turns.get_mut(&turn) else {
            return;
        };
        if bucket.text.is_empty() && bucket.reasoning.is_empty() {
            return; // 纯工具 turn 无文本条目
        }
        let mut parts = Vec::with_capacity(2);
        if !bucket.reasoning.is_empty() {
            parts.push(WirePart::Reasoning {
                text: std::mem::take(&mut bucket.reasoning),
            });
        }
        if !bucket.text.is_empty() {
            parts.push(WirePart::Text {
                text: std::mem::take(&mut bucket.text),
            });
        }
        let entry = MessageEntry {
            id: format!("{}:{turn}", self.run_id),
            parent_id: self.last_entry_id.clone(),
            role: MessageRole::Assistant,
            parts,
            created_at: now_ms(),
            run_id: Some(self.run_id.clone()),
            status: Some(MessageStatus::Complete {
                reason: CompleteReason::Stop,
            }),
        };
        self.last_entry_id = Some(entry.id.clone());
        self.append(vec![StoredEntry::Message(entry)]);
    }

    /// 工具 turn 收口（§7.3「每个工具结果」行）：批内最后结果触发。
    fn flush_tool_turn(&mut self, turn: u32) {
        let Some(bucket) = self.turns.get_mut(&turn) else {
            return;
        };
        if bucket.calls.iter().any(|c| c.result.is_none()) {
            return; // 批未收齐
        }
        let mut parts = Vec::with_capacity(bucket.calls.len() * 2);
        for call in &bucket.calls {
            let Some((ok, summary)) = &call.result else {
                continue;
            };
            parts.push(WirePart::ToolCall {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                // 规范化 compact JSON（键序保持），与前端 JSON.stringify(args) 口径一致
                args_json: normalize_json(&call.args),
            });
            parts.push(WirePart::ToolResult {
                call_id: call.call_id.clone(),
                ok: *ok,
                summary: summary.clone(),
            });
        }
        if parts.is_empty() {
            return;
        }
        bucket.calls.clear(); // 幂等：StepStarted 防御路径不重复落
        let entry = MessageEntry {
            id: format!("{}:{turn}.tools", self.run_id),
            parent_id: self.last_entry_id.clone(),
            role: MessageRole::Assistant,
            parts,
            created_at: now_ms(),
            run_id: Some(self.run_id.clone()),
            status: Some(MessageStatus::Complete {
                reason: CompleteReason::Stop,
            }),
        };
        self.last_entry_id = Some(entry.id.clone());
        self.append(vec![StoredEntry::Message(entry)]);
    }

    /// run 终态（§7.3「run 终态」行）：runMarker 条目 + index upsert。
    fn finish(&mut self, duration_ms: u64, tool_count: u32) {
        let marker = MessageEntry {
            id: format!("{}:marker", self.run_id),
            parent_id: self.last_entry_id.clone(),
            role: MessageRole::System,
            parts: vec![WirePart::RunMarker {
                run_id: self.run_id.clone(),
                started_at: self.started_at.unwrap_or(0),
                ended_at: Some(now_ms()),
                duration_ms: Some(duration_ms as i64),
                tool_count: Some(tool_count),
            }],
            created_at: now_ms(),
            run_id: None,
            status: None,
        };
        self.last_entry_id = Some(marker.id.clone());
        self.append(vec![StoredEntry::Message(marker)]);
        self.upsert_index();
    }

    /// canonical 追加 + 记账（revision/messageCount 供 index upsert）。
    fn append(&mut self, batch: Vec<StoredEntry>) {
        match self.store.append_canonical(&self.session_id, &batch) {
            Ok((revision, count)) => {
                self.last_revision = revision;
                self.last_message_count = count;
            }
            Err(e) => {
                #[cfg(debug_assertions)]
                eprintln!(
                    "[craftwave] canonical 落盘失败（session={} run={}）：{e:?}",
                    self.session_id, self.run_id
                );
                // 落盘失败记账（32 号票，§7.3 ⑤ app 通道；无内容副本——
                // 只记元数据与错误形态）
                if let Some(log) = &self.app_log {
                    log.app_line(
                        crate::providers::types::LogLevel::Warn,
                        format!("canonical 落盘失败：run={}", self.run_id),
                        Some(serde_json::json!({ "sessionId": self.session_id, "error": format!("{e:?}") })),
                    );
                }
                #[cfg(not(debug_assertions))]
                let _ = e;
            }
        }
    }

    /// index.json upsert（§7.1）：会话首现时建档，终态推进用量与修订号。
    fn upsert_index(&self) {
        let now = now_ms();
        let existing = self
            .store
            .index()
            .list()
            .into_iter()
            .find(|m| m.id == self.session_id);
        let run_tokens =
            self.usage.input + self.usage.output + self.usage.cache_read + self.usage.cache_write;
        let meta = match &existing {
            Some(prior) => SessionMeta {
                id: self.session_id.clone(),
                project_id: prior.project_id.clone(),
                title: prior.title.clone(),
                created_at: prior.created_at,
                updated_at: now,
                revision: self.last_revision.max(prior.revision),
                message_count: self.last_message_count.max(prior.message_count),
                total_tokens: prior.total_tokens + run_tokens,
                // 外部目录授权跨 run 生效（24 号票 §9.9）：run 终态 upsert
                // 只推进用量/条目/修订号，不清既有授权（曾误写空数组的
                // 回归由 run_persistence_test 钉住）。
                authorized_external_dirs: prior.authorized_external_dirs.clone(),
            },
            None => SessionMeta {
                id: self.session_id.clone(),
                project_id: None,
                title: "未命名会话".to_owned(),
                created_at: now,
                updated_at: now,
                revision: self.last_revision,
                message_count: self.last_message_count,
                total_tokens: run_tokens,
                authorized_external_dirs: Vec::new(),
            },
        };
        if let Err(e) = self.store.index().upsert(meta) {
            #[cfg(debug_assertions)]
            eprintln!(
                "[craftwave] index.json upsert 失败（{}）：{e:?}",
                self.session_id
            );
            // 落盘失败记账（32 号票，§7.3 ⑤ app 通道）
            if let Some(log) = &self.app_log {
                log.app_line(
                    crate::providers::types::LogLevel::Warn,
                    "index.json upsert 失败".to_owned(),
                    Some(serde_json::json!({ "sessionId": self.session_id, "error": format!("{e:?}") })),
                );
            }
            #[cfg(not(debug_assertions))]
            let _ = e;
        }
    }
}

/// args JSON 规范化（保序 compact；解析失败保留原文——半截参数不落工具条目，
/// 正常路径 ToolResult 到达时参数必已完整）。
fn normalize_json(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .map(|v| v.to_string())
        .unwrap_or_else(|_| raw.to_owned())
}

/// 当前时间的 Unix 毫秒时间戳（取不到时兜底 0，只影响展示字段不参与逻辑）。
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
