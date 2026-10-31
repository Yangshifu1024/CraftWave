//! 假数据装置（gpui-rewrite 2.12 最小版）：消息工厂 + 快照构造 + @ 补全 stub。
//!
//! 定位（spec 2.12「UI 测试装置」+ 原则 6「每切片附假数据驱动的薄测试装置」）：
//! - 阶段 2.C-1 的消息流底座 / Composer 不接真帧流（真管道是 2.C-2），全部
//!   行为由本模块构造的 [`ConversationSnapshot`] 驱动；
//! - 消息形态覆盖行分发四形态（User 气泡 / Assistant 三槽位列 / System 胶囊）
//!   与过滤器正反例（纪律二：只有 thinking 的 Assistant 是运行日志，不渲染）；
//! - 生产装配（main.rs 的演示会话）与 `#[gpui_kit::test]` 用例共用同一份
//!   工厂，保证「测的就是跑的」。
//!
//! 真 start_chat / attach_image / search_workspace_paths 的接线在 2.C-2；
//! 到时 fixture 只剩测试职责，生产装配换引擎装配链。

use std::rc::Rc;

use craftwave_engine::api::workspace::WorkspacePathHit;
use craftwave_engine::followup::QueuedImageAttachment;
use craftwave_engine::storage::{MessageEntry, MessageRole, WirePart};
// 流式态工厂只进测试（生产演示装配没有活跃 run）；类型随用随引
#[cfg(test)]
use craftwave_engine::backend::frames::{EndReason, UsageTotal};
#[cfg(test)]
use craftwave_engine::snapshot::{
    RunInfo, StreamPart, StreamingMessage, ThinkingSpan, ToolCallState, ToolResultState,
};

/// 统一的假时间戳基线（Unix 纪元毫秒；只影响排序展示，值本身无语义）。
pub const FAKE_NOW_MS: i64 = 1_736_000_000_000;

/// 自增序号（消息 id 唯一性由它保证；测试内多工厂共用无碍——id 只要求
/// 会话内唯一，fixture 每次生成自增串即可）。
fn next_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!("fake-{}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

// ======================================================================
// 单条消息工厂（四形态 + 过滤器正反例）
// ======================================================================

/// 基础骨架：角色 + parts → 条目（id/时间戳自动生成）。
fn entry(role: MessageRole, parts: Vec<WirePart>) -> Rc<MessageEntry> {
    Rc::new(MessageEntry {
        id: next_id(),
        parent_id: None,
        role,
        parts,
        created_at: FAKE_NOW_MS,
        run_id: None,
        status: None,
    })
}

/// User 纯文本消息（行分发形态一：右对齐气泡）。
pub fn user_msg(text: &str) -> Rc<MessageEntry> {
    entry(
        MessageRole::User,
        vec![WirePart::Text {
            text: text.to_owned(),
        }],
    )
}

/// User 带图消息（气泡内容段 = 文本 + 图片段；图片本批渲染占位块）。
#[cfg(test)]
pub fn user_msg_with_image(text: &str) -> Rc<MessageEntry> {
    entry(
        MessageRole::User,
        vec![
            WirePart::Text {
                text: text.to_owned(),
            },
            WirePart::Image {
                blob_hash: "fakehash0123456789abcdef0123456789".to_owned(),
                mime_type: "image/png".to_owned(),
            },
        ],
    )
}

/// Assistant 纯正文消息（形态二：全宽列，只有正文槽位）。
pub fn assistant_msg(text: &str) -> Rc<MessageEntry> {
    entry(
        MessageRole::Assistant,
        vec![WirePart::Text {
            text: text.to_owned(),
        }],
    )
}

/// Assistant 完整三槽位消息：thinking + 正文 + 两个工具（一簇卡）。
/// 工具结果一好一坏，覆盖簇卡失败 badge 的数据面。
pub fn assistant_full(thinking: &str, body: &str) -> Rc<MessageEntry> {
    entry(
        MessageRole::Assistant,
        vec![
            WirePart::Reasoning {
                text: thinking.to_owned(),
            },
            WirePart::Text {
                text: body.to_owned(),
            },
            WirePart::ToolCall {
                call_id: "call-ok".to_owned(),
                name: "read_file".to_owned(),
                args_json: r#"{"path":"src/main.rs"}"#.to_owned(),
            },
            WirePart::ToolResult {
                call_id: "call-ok".to_owned(),
                ok: true,
                summary: "128 行已读取".to_owned(),
            },
            WirePart::ToolCall {
                call_id: "call-bad".to_owned(),
                name: "run_command".to_owned(),
                args_json: r#"{"cmd":"cargo build"}"#.to_owned(),
            },
            WirePart::ToolResult {
                call_id: "call-bad".to_owned(),
                ok: false,
                summary: "编译错误：E0412".to_owned(),
            },
        ],
    )
}

/// System / 压缩摘要消息（形态三：居中小胶囊）。
/// 定稿投影里压缩摘要以 System 角色进入活动路径消息列，本形态即由它承载。
pub fn system_msg(text: &str) -> Rc<MessageEntry> {
    entry(
        MessageRole::System,
        vec![WirePart::Text {
            text: text.to_owned(),
        }],
    )
}

/// 运行日志型消息（过滤器正例）：Assistant 且无正文、无工具、无图、
/// 只有 thinking —— 纪律二要求渲染层滤掉（返回空行），源数据保留。
pub fn log_only_assistant(thinking: &str) -> Rc<MessageEntry> {
    entry(
        MessageRole::Assistant,
        vec![WirePart::Reasoning {
            text: thinking.to_owned(),
        }],
    )
}

/// 过滤器反例 1：Assistant 只有工具（无正文）——有工具就不算日志，要渲染。
pub fn assistant_tool_only() -> Rc<MessageEntry> {
    entry(
        MessageRole::Assistant,
        vec![
            WirePart::ToolCall {
                call_id: "call-1".to_owned(),
                name: "search_files".to_owned(),
                args_json: r#"{"query":"main"}"#.to_owned(),
            },
            WirePart::ToolResult {
                call_id: "call-1".to_owned(),
                ok: true,
                summary: "3 处命中".to_owned(),
            },
        ],
    )
}

/// 过滤器反例 2：parts 为空的 Assistant —— 同样按日志滤掉（all() 空集
/// 为真，与「只有 thinking」同判）。
#[cfg(test)]
pub fn empty_assistant() -> Rc<MessageEntry> {
    entry(MessageRole::Assistant, vec![])
}

// ======================================================================
// 历史消息列表（快照 messages 面）
// ======================================================================

/// 最小四形态历史：每个形态一条 + 过滤器正反例各一条。
/// 测试用它验证「四形态各渲一帧 + 日志行被滤」；下标即行号（过滤器
/// 不改变行号映射——被滤的行渲染为零高空 div，见 state.rs 注释）。
#[cfg(test)]
pub fn four_forms_history() -> Rc<Vec<Rc<MessageEntry>>> {
    Rc::new(vec![
        user_msg("帮我看看这个仓库的构建脚本"),                       // 行 0：User
        log_only_assistant("用户想看构建脚本，我先想想从哪入手……"),     // 行 1：日志型（滤掉）
        assistant_full(                                              // 行 2：Assistant 三槽位
            "先读 package.json 与根目录脚本。",
            "构建走 `pnpm build`：先 tsc 类型检查，再 vite 打包产物到 dist/。",
        ),
        system_msg("对话已压缩：之前 32 条消息已摘要为上文。"),          // 行 3：System 胶囊
        assistant_tool_only(),                                       // 行 4：工具-only（不滤）
        user_msg_with_image("报错截图如下，帮忙看一眼"),                // 行 5：User 带图
    ])
}

/// 演示用历史（main.rs 中栏装配）：四形态 + 足量条数让列表可滚动，
/// 覆盖滚动跟随 / 上滚暂停的肉眼验证。
pub fn demo_history() -> Rc<Vec<Rc<MessageEntry>>> {
    let mut messages: Vec<Rc<MessageEntry>> = vec![
        user_msg("帮我梳理这个项目的模块结构"),
        assistant_full(
            "先看 Cargo workspace 与 crates 目录。",
            "仓库是单一 Rust workspace：`crates/engine` 是框架无关的引擎库，\
             `crates/ui` 是 GPUI 应用壳。engine 内部按 agent / backend / tools / \
             storage 分层，api/ 目录暴露给 UI 的命令面。",
        ),
        log_only_assistant("用户想了解模块结构，我需要先看 Cargo.toml……"),
        user_msg("@Cargo.toml 里的依赖是怎么组织的？"),
        assistant_tool_only(),
        system_msg("对话已压缩：更早的消息已替换为摘要。"),
    ];
    // 循环填充到 24 条：长短交替，制造不等高行，滚动行为肉眼可验
    let mut i = 0;
    while messages.len() < 24 {
        let turn = i % 2 == 0;
        messages.push(if turn {
            user_msg(&format!("第 {} 个问题：这块的实现细节再展开讲讲？", i + 1))
        } else {
            assistant_msg(&format!(
                "第 {} 个回答：这里是占位正文，行高 1.9、全宽平铺。{}",
                i + 1,
                "补充说明文本让这行足够长，验证折行与不等高测量。".repeat(2)
            ))
        });
        i += 1;
    }
    Rc::new(messages)
}

/// n 条 User/Assistant 交替的长历史（滚动测试用：条数足够把列表撑出滚动量）。
#[cfg(test)]
pub fn long_history(n: usize) -> Rc<Vec<Rc<MessageEntry>>> {
    Rc::new(
        (0..n)
            .map(|i| {
                if i % 2 == 0 {
                    user_msg(&format!("问题 {i}：请回答一段足够长的内容把列表撑开。"))
                } else {
                    assistant_msg(&format!(
                        "回答 {i}：{}",
                        "占位正文，足够长以产生不等高行。".repeat(8)
                    ))
                }
            })
            .collect(),
    )
}

// ======================================================================
// 流式中的末条（快照 streaming 面）
// ======================================================================

/// 一个仍在流式的 run：thinking 已收口 + 一段正文增量（typewriter 面的
/// 起点形态）。`started_at` 用进程单调钟 now——思考时长展示只算相对值。
#[cfg(test)]
pub fn streaming_message(text_len: usize) -> StreamingMessage {
    let text: String = "这是流式输出的正文，".repeat(text_len.div_euclid(11).max(1));
    StreamingMessage {
        run: Some(RunInfo {
            run_id: "fake-run".to_owned(),
            model: "fake-model".to_owned(),
        }),
        started: true,
        ended: None,
        parts: vec![
            StreamPart::Reasoning {
                text: "先想想怎么回答。".to_owned(),
            },
            StreamPart::Text { text },
        ],
        tool_calls: Vec::new(),
        usage: UsageTotal::default(),
        rate: Some(42.0),
        compaction: None,
        thinking: Some(ThinkingSpan {
            started_at: std::time::Instant::now(),
            ended_at: Some(std::time::Instant::now()),
        }),
        subagents: Vec::new(),
        max_seq: Some(3),
        dropped_seq: 0,
    }
}

/// 终态流式 run（ended = Some）：收口重载前的最后一帧形态。
#[cfg(test)]
pub fn ended_streaming_message() -> StreamingMessage {
    let mut msg = streaming_message(12);
    msg.ended = Some(craftwave_engine::snapshot::RunEnd {
        reason: EndReason::Done,
        duration_ms: 4200,
        tool_count: 0,
        turn: 1,
        error: None,
    });
    msg
}

/// 带工具的流式 run：工具簇槽位在流式行也要能渲染。
#[cfg(test)]
pub fn streaming_with_tools() -> StreamingMessage {
    let mut msg = streaming_message(6);
    msg.parts.push(StreamPart::ToolCall {
        call_id: "call-live".to_owned(),
        name: "read_file".to_owned(),
        args_json: r#"{"path":"src/main.rs"}"#.to_owned(),
        result: Some(ToolResultState {
            ok: true,
            ui_summary: "ok".to_owned(),
            error: None,
        }),
        audit: None,
        audit_reason: None,
    });
    msg.tool_calls.push(ToolCallState {
        call_id: "call-live".to_owned(),
        name: "read_file".to_owned(),
        args_json: r#"{"path":"src/main.rs"}"#.to_owned(),
        result: Some(ToolResultState {
            ok: true,
            ui_summary: "ok".to_owned(),
            error: None,
        }),
        audit: None,
        audit_reason: None,
    });
    msg
}

// ======================================================================
// 回声模式（无服务的测试装配用）：发送分流把 (正文, 附件) 重建为一条
// User 条目直接追加进会话——2.C-1 回声发送的工厂保留为测试装置形态
// （真通道 = SessionService::send_message，不经过这里）。
// ======================================================================

/// (正文, 附件) → 一条 User 条目（正文在前，附件展开为 Image part）。
/// 与生产发送链 `service::persist_user` 的 parts 排布同一口径（树挂接
/// 除外：回声条目不落盘、不挂树）。
pub fn echo_entry(text: &str, attachments: Vec<QueuedImageAttachment>) -> Rc<MessageEntry> {
    let mut parts = vec![WirePart::Text {
        text: text.to_owned(),
    }];
    for attachment in &attachments {
        parts.push(WirePart::Image {
            blob_hash: attachment.blob_hash.clone(),
            mime_type: attachment.mime_type.clone(),
        });
    }
    Rc::new(MessageEntry {
        id: next_id(),
        parent_id: None,
        role: MessageRole::User,
        parts,
        created_at: FAKE_NOW_MS,
        run_id: None,
        status: None,
    })
}

// ======================================================================
// @ 补全 stub 源（真接线在 2.C-2b：EngineCompletionSource 经
// SessionService 取 AppState 装配链；本源仅测试装配使用）
// ======================================================================

/// 静态假补全源：内存路径表 + 基名大小写不敏感子串匹配（与引擎
/// `search_workspace_paths` 的匹配口径一致，只是数据来源不同）。
pub struct FakeCompletionSource {
    pub entries: Vec<WorkspacePathHit>,
}

impl Default for FakeCompletionSource {
    fn default() -> Self {
        Self {
            entries: vec![
                hit("crates/ui/src/main.rs", "file"),
                hit("crates/ui/src/chat/mod.rs", "file"),
                hit("crates/ui/src/chat/fixture.rs", "file"),
                hit("crates/engine/src/snapshot.rs", "file"),
                hit("docs/specs/gpui-rewrite", "dir"),
                hit("README.md", "file"),
            ],
        }
    }
}

fn hit(path: &str, kind: &str) -> WorkspacePathHit {
    WorkspacePathHit {
        path: path.to_owned(),
        kind: kind.to_owned(),
    }
}

impl super::composer::CompletionSource for FakeCompletionSource {
    /// 空查询给前几条（弹层初始候选），非空按子串过滤（trait 面，
    /// composer 的 source 槽位消费它）。假源同步快返回：用 ready 任务
    /// 保持与真源（后台线程池）同一条异步回写路径。
    fn search(&self, query: &str, cx: &mut gpui_kit::App) -> gpui_kit::Task<Vec<WorkspacePathHit>> {
        let needle = query.trim().to_lowercase();
        let hits = if needle.is_empty() {
            self.entries.iter().take(5).cloned().collect()
        } else {
            self.entries
                .iter()
                .filter(|hit| hit.path.to_lowercase().contains(&needle))
                .cloned()
                .collect()
        };
        let _ = cx;
        gpui_kit::Task::ready(hits)
    }
}
