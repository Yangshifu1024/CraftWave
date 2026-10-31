//! 引擎会话服务（gpui-rewrite 阶段 2.C-2b）：「发消息 → 流式上屏 → 取消」
//! 的帧流管道与发送链。
//!
//! ## 线程模型（engine.rs 的桥在本模块消费）
//!
//! ```text
//! 主线程                                     tokio 线程池
//! ──────                                     ────────────
//! send_message（Enter 分流进来）
//!   ├─ 建会话 / 落盘 user 条目（同步小 IO）
//!   ├─ 重置聚合器 + 建通道 + 起泵/节流任务      start_chat（装配树 + run 任务）
//!   └─ busy 上屏（停止钮出现）                 FrameHub → FrameBridge ──┐
//! 泵任务（cx.spawn，主线程）                                            │
//!   recv UiMail::Frame → aggregator.push ◀──── UiMail::Frame ──────────┘
//!   recv UiMail::Started → 错误合成终态帧
//! 节流任务（cx.spawn，主线程）
//!   每 64ms：aggregator → ConversationSnapshot
//!   → ConversationState::apply_snapshot（唯一写入口，行分发/虚拟列表
//!     的重排逻辑全在会话实体侧）
//!   终态快照产出 → 下个 tick 落盘重载历史 → apply_snapshot(history, None)
//!   → busy 下线 → 收口退出
//! ```
//!
//! tokio 侧只跑 engine 的异步调用：`start_chat` 是 async fn 且装配树里
//! `tokio::spawn` 驱动 run 任务，必须在 tokio 运行时上下文内 poll——壳
//! 自备常驻多线程 `Runtime`（[`crate::engine`]），调用经
//! `runtime.spawn(...)` 进线程池。tokio 的 JoinHandle 不进 `tasks`
//! （它不是 gpui Task，回执经 [`UiMail::Started`] 通道回主线程，通道
//! 关闭即无人等待——与引擎内 run 任务同为 fire-and-forget 形态）。
//!
//! 节流不用 `SnapshotTicker`：它的 `next()` 内部是 `tokio::time::sleep`
//! （要求在 tokio 运行时上下文里被 poll），而快照的消费端要主线程的
//! 会话实体（`Rc` 历史不能跨线程）——壳侧用 gpui 自己的
//! `executor.timer` 写了同语义的循环（64ms 窗口 / 终态快照必达一次后
//! 收口 / abort 收口），逐条对齐 `SnapshotTicker` 的文档约定；
//! `SnapshotTicker` 留给能在 tokio 上下文里持有历史的宿主（`Rc` 改
//! Send 后可回归）。
//!
//! ## 所有权
//!
//! `SessionService` 是应用级服务（Global 持强引用，寿命 = 应用）；它持
//! 会话视图 `WeakEntity<ConversationState>`（视图随窗口生死，服务升级
//! 失败即静默停写）；Composer 的取消回调同样持服务的弱引用。泵 / 节流
//! 两类 gpui `Task` 句柄存 owner（[`Self::tasks`]），重置管道时整体
//! drop（gpui Task drop 即取消）。

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, Context, Task, WeakEntity};

use craftwave_engine::api::run::{
    self, AttachRunArgs, DEFAULT_MAX_TURNS, RunConfig, StartChatArgs,
};
use craftwave_engine::api::session::{
    self, CreateSessionArgs, LoadSessionTreeArgs, ReadImageBlobArgs, SaveSessionTreeArgs,
};
use craftwave_engine::api::workspace::{
    self as workspace_api, SearchWorkspacePathsArgs, WorkspacePathHit,
};
use craftwave_engine::app_state::AppState;
use craftwave_engine::backend::frames::{ChatFrame, EndReason, FrameError};
use craftwave_engine::followup::QueuedImageAttachment;
use craftwave_engine::safety::tier::PermissionTier;
use craftwave_engine::snapshot::{FrameAggregator, SNAPSHOT_INTERVAL};
use craftwave_engine::storage::{self, MessageEntry, MessageRole, WirePart};
use craftwave_engine::util::errors::IpcError;
use craftwave_engine::util::ids::new_uuid;

use crate::engine::{FrameBridge, MailEventSink, UiMail};

use super::composer::CompletionSource;
use super::state::ConversationState;

/// 会话服务：engine 调用面 + 帧流管道的 owner。
pub struct SessionService {
    /// engine 应用状态（AppState::assemble 的产物；测试注入 tempdir 装配）。
    state: Arc<AppState>,
    /// 事件汇的具体类型句柄（`AppState.events` 存的是 trait 对象）——
    /// 每次发消息把会话邮箱绑上去，终态三键等事件才有回主线程的路。
    events: Arc<MailEventSink>,
    /// 常驻 tokio 运行时（engine 异步调用的执行器）。
    runtime: Arc<tokio::runtime::Runtime>,
    /// 会话视图（弱引用：视图随窗口生死；升级失败 = 窗口已关，静默停写）。
    conversation: WeakEntity<ConversationState>,
    /// 当前会话 id（None = 尚未建会话，首条消息发送时创建）。
    session_id: Option<String>,
    /// 当前历史基准（活动路径线性序；节流快照携带它上屏）。
    history: Rc<Vec<Rc<MessageEntry>>>,
    /// 帧聚合器（每次新 run 重置；泵任务与节流任务共享 Arc）。
    aggregator: Arc<FrameAggregator>,
    /// 当前 run 的邮包发送端（持有它 = 通道开着；替换即关闭旧泵）。
    mailbox: Option<tokio::sync::mpsc::UnboundedSender<UiMail>>,
    /// 泵 / 节流任务的 gpui 句柄（存 owner：重置管道时整体 drop 即取消；
    /// owner 释放同样自动取消）。
    tasks: Vec<Task<()>>,
    /// 有进行中的 run（停止钮的显示面；终态收口置回 false）。
    busy: bool,
}

impl SessionService {
    /// 构造：`state` + `events` 由壳装配（生产 `engine::assemble_state`，
    /// 测试 tempdir 装配）后注入；`runtime` 取常驻运行时。
    pub fn new(
        state: Arc<AppState>,
        events: Arc<MailEventSink>,
        runtime: Arc<tokio::runtime::Runtime>,
        conversation: WeakEntity<ConversationState>,
    ) -> Self {
        Self {
            state,
            events,
            runtime,
            conversation,
            session_id: None,
            history: Rc::new(Vec::new()),
            aggregator: Arc::new(FrameAggregator::new()),
            mailbox: None,
            tasks: Vec::new(),
            busy: false,
        }
    }

    /// 当前会话 id。
    // 生产消费方随阶段 3.1 多会话侧栏接入；本批由测试面使用
    #[allow(dead_code)]
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// 登记会话视图（窗口重开 / 重建时更新：服务在 Global 常驻，视图随
    /// 窗口生死——弱引用，旧视图释放后升级失败即静默停写）。
    pub fn set_conversation(&mut self, conversation: WeakEntity<ConversationState>) {
        self.conversation = conversation;
    }

    /// 是否有进行中的 run（Composer 停止钮的显示面）。
    #[allow(dead_code)]
    pub fn busy(&self) -> bool {
        self.busy
    }

    /// 当前历史基准（活动路径；测试断言用）。
    #[allow(dead_code)]
    pub fn history(&self) -> &Rc<Vec<Rc<MessageEntry>>> {
        &self.history
    }

    /// 帧聚合器句柄（测试驱动假帧用；生产只有泵任务写它）。
    #[cfg(test)]
    pub(crate) fn aggregator(&self) -> &Arc<FrameAggregator> {
        &self.aggregator
    }

    /// 起一套帧流管道并交出邮包发送端（集成测试用）：集成测试经 engine
    /// 的 test-utils 接缝（`start_chat_test`）注入剧本模型，sink 用本端
    /// 构造 FrameBridge——与生产 `send_message` 的 ⑥ 段同一形态，只是
    /// 模型解析来源不同。busy 由测试置位（对应真实发送）。
    #[cfg(test)]
    pub(crate) fn begin_pipeline(
        &mut self,
        cx: &mut Context<Self>,
    ) -> tokio::sync::mpsc::UnboundedSender<UiMail> {
        self.reset_pipeline(self.history.clone(), cx);
        self.set_busy(true, cx);
        self.mailbox
            .clone()
            .expect("reset_pipeline 后必有邮箱")
    }

    /// 补全查询的上下文（AppState + 会话 id）：EngineCompletionSource 经
    /// 它取数；无会话（尚未发过消息）返回 None。
    pub(crate) fn search_context(&self) -> Option<(Arc<AppState>, String)> {
        let session_id = self.session_id.as_deref()?;
        Some((self.state.clone(), session_id.to_owned()))
    }

    // ==================================================================
    // 发送链（Composer Enter 分流的真通道）
    // ==================================================================

    /// 发送一条消息：建会话（无会话时）→ user 条目落盘 → 管道重置 →
    /// start_chat（tokio）。模型与权限档本批用默认值（设置界面阶段 3）。
    pub fn send_message(
        &mut self,
        text: String,
        attachments: Vec<QueuedImageAttachment>,
        cx: &mut Context<Self>,
    ) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        // ① 会话（无则建；create_session 是一次 index 追加，同步小 IO）
        if self.session_id.is_none()
            && let Err(err) = self.ensure_session()
        {
            eprintln!("[craftwave] 会话创建失败：{err:?}");
            return;
        }
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        // ② 发送前树同步（双写协议：user 拓扑先落盘再 start_chat——engine
        //    侧另有幂等兜底，这里按正常协议走）
        let user_entry = match self.persist_user(&session_id, &text, attachments) {
            Ok(entry) => entry,
            Err(err) => {
                eprintln!("[craftwave] 消息落盘失败（不发送）：{err:?}");
                return;
            }
        };
        // ③ 管道重置：新聚合器 + 新通道 + 新泵/节流（旧任务句柄 drop 即取消）
        let mut history = (*self.history).clone();
        history.push(Rc::new(user_entry.clone()));
        self.reset_pipeline(Rc::new(history), cx);
        // ④ user 气泡即刻上屏（不等首帧）
        self.push_history(cx);
        // ⑤ busy 上屏（停止钮出现）
        self.set_busy(true, cx);
        // ⑥ start_chat 跑 tokio：装配树 + run 任务全在线程池；帧经
        //    FrameBridge 回主线程泵，完成回执经同一通道。JoinHandle 不存
        //    （回执驱动，见模块文档）。
        let state = self.state.clone();
        let mailbox = match self.mailbox.clone() {
            Some(mailbox) => mailbox,
            None => return,
        };
        let auto_confirm = state
            .config
            .load()
            .map(|config| config.approval.auto_confirm)
            .unwrap_or(false);
        self.runtime.spawn(async move {
            let result = run::start_chat(
                &state,
                &state.runs,
                StartChatArgs {
                    session_id,
                    messages: vec![user_entry],
                    run_config: default_run_config(),
                },
                Box::new(FrameBridge::new(mailbox.clone())),
                auto_confirm,
            )
            .await;
            // 回执给泵（发送失败 = 通道已关：管道已重建，本次调用的结果
            // 不再有意义）
            let _ = mailbox.send(UiMail::Started(result.map(|r| r.run_id)));
        });
    }

    /// 取消当前会话的活跃 run（停止钮 / 窗口关闭共用）：引擎置位取消令牌
    /// 后自行补发 runEnded(cancelled)，节流循环按终态帧自然收口。
    pub fn cancel(&mut self, _cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id.clone() else {
            return;
        };
        match run::cancel_run(&self.state, &session_id) {
            Ok(started) => {
                if !started {
                    eprintln!("[craftwave] 取消时无活跃 run（幂等）");
                }
            }
            Err(err) => eprintln!("[craftwave] 取消失败：{err:?}"),
        }
    }

    /// 图片附件落盘（生产文件选择器阶段 4 接 rfd 后调用；本批由测试驱动
    /// 验证 attach_image 链路）：路径读取 → 魔数嗅探 → 内容寻址落盘 →
    /// 回读 mime → 返回可入 Composer 队列的附件引用。需要已有会话
    /// （blob 按会话目录存放），无会话时先建。
    // 生产消费方随阶段 4 的文件选择器接入；本批由测试面使用
    #[allow(dead_code)]
    pub fn attach_image_from_path(
        &mut self,
        path: &str,
        _cx: &mut Context<Self>,
    ) -> Result<QueuedImageAttachment, IpcError> {
        if self.session_id.is_none() {
            self.ensure_session()?;
        }
        let session_id = self
            .session_id
            .clone()
            .ok_or_else(|| IpcError::internal("会话创建后仍无 id"))?;
        let attached = session::attach_image(
            &self.state,
            session::AttachImageArgs {
                session_id: session_id.clone(),
                data_base64: None,
                file_path: Some(path.to_owned()),
                // filePath 形态时 mime 以魔数嗅探为准（声明字段被引擎忽略）
                mime_type: "image/png".to_owned(),
            },
        )?;
        // mime 回读（attach_image 只回 hash；blob 里存的是嗅探结果——
        // 附件 chip 与渲染层显示同一口径）
        let blob = session::read_image_blob(
            &self.state,
            ReadImageBlobArgs {
                session_id,
                blob_hash: attached.blob_hash.clone(),
            },
        )?;
        Ok(QueuedImageAttachment {
            blob_hash: attached.blob_hash,
            mime_type: blob.mime_type,
        })
    }

    /// 切换 / 打开会话：装载历史 + 重置管道 + `attach_run` 续流（该会话
    /// 有进行中 run 时帧从 0 重放并重新接上节流）。阶段 3.1 多会话侧栏
    /// 的接线点；本批由测试面使用。
    #[allow(dead_code)]
    pub fn open_session(&mut self, session_id: String, cx: &mut Context<Self>) {
        // 旧管道收摊（泵/节流 drop；旧通道关闭）
        self.tasks.clear();
        self.mailbox = None;
        self.aggregator = Arc::new(FrameAggregator::new());
        self.busy = false;
        self.session_id = Some(session_id.clone());
        // 历史装载（会话不存在 → 空历史 + 日志，不 panic）
        self.history = match session::load_session_tree(
            &self.state,
            LoadSessionTreeArgs {
                session_id: session_id.clone(),
            },
        ) {
            Ok(tree) => active_path(&tree.messages, tree.active_leaf_id.as_deref()),
            Err(err) => {
                eprintln!("[craftwave] 会话历史装载失败（按空会话继续）：{err:?}");
                Rc::new(Vec::new())
            }
        };
        // 续流：重放 (0, 头] 全区间到新通道；无活跃 run 时返回空重放。
        // 心跳任务无条件起：无活跃 run 时通道空转（每 64ms 一次空排空），
        // 换取「有 run 即自动接流」的简单语义（busy 面与重放结果对齐）
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let attach = run::attach_run(
            &self.state,
            AttachRunArgs {
                session_id: session_id.clone(),
                since_seq: 0,
            },
            Box::new(FrameBridge::new(tx.clone())),
        );
        self.mailbox = Some(tx.clone());
        self.events.set_mailbox(tx);
        let running = attach.map(|result| result.run_id.is_some()).unwrap_or(false);
        self.busy = running;
        if running {
            self.spawn_stream_loop(rx, cx);
        }
        self.push_history(cx);
        self.push_busy(cx);
    }

    /// 窗口关闭的优雅收尾：取消进行中 run（退出拦截本身阶段 4.6）。
    pub fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.cancel(cx);
    }

    // ==================================================================
    // 管道内部件
    // ==================================================================

    /// 无会话时建一个（临时会话：project_id None）。
    fn ensure_session(&mut self) -> Result<(), IpcError> {
        let meta = session::create_session(&self.state, CreateSessionArgs::default())?;
        self.session_id = Some(meta.id);
        Ok(())
    }

    /// 发送前树同步：load 全树 → 追加 user 条目（挂活动叶）→ save。
    /// 失败上抛（调用方放弃发送并日志——输入还留在 composer 不丢字）。
    /// 树尚不存在（新会话首条消息：历史文件随首次 save 产生，load 报
    /// E_NOT_FOUND）按空树处理——与旧栈「失败不阻断」不同，这里保持
    /// 正常协议（save 成功后再 start_chat），不依赖引擎的幂等兜底。
    fn persist_user(
        &self,
        session_id: &str,
        text: &str,
        attachments: Vec<QueuedImageAttachment>,
    ) -> Result<MessageEntry, IpcError> {
        let tree = match session::load_session_tree(
            &self.state,
            LoadSessionTreeArgs {
                session_id: session_id.to_owned(),
            },
        ) {
            Ok(tree) => tree,
            Err(err) if err.code == craftwave_engine::util::errors::E_NOT_FOUND => {
                // 空树：无活动叶（user 是首节点）、revision 从 0 起
                craftwave_engine::storage::SessionTreeData {
                    session_id: session_id.to_owned(),
                    messages: Vec::new(),
                    compactions: Vec::new(),
                    active_leaf_id: None,
                    revision: 0,
                }
            }
            Err(err) => return Err(err),
        };
        // 正文在前，附件随后展开为 Image part（wire 契约：blobHash + mime）
        let mut parts = vec![WirePart::Text {
            text: text.to_owned(),
        }];
        for attachment in &attachments {
            parts.push(WirePart::Image {
                blob_hash: attachment.blob_hash.clone(),
                mime_type: attachment.mime_type.clone(),
            });
        }
        let entry = MessageEntry {
            id: new_uuid(),
            parent_id: tree.active_leaf_id.clone(),
            role: MessageRole::User,
            parts,
            created_at: storage::trust::now_ms(),
            run_id: None,
            status: None,
        };
        let mut messages = tree.messages;
        messages.push(entry.clone());
        session::save_session_tree(
            &self.state,
            SaveSessionTreeArgs {
                session_id: session_id.to_owned(),
                messages,
                active_leaf_id: Some(entry.id.clone()),
                base_revision: tree.revision,
            },
        )?;
        Ok(entry)
    }

    /// 重置帧流管道：新聚合器 + 新通道 + 新心跳任务。
    fn reset_pipeline(&mut self, history: Rc<Vec<Rc<MessageEntry>>>, cx: &mut Context<Self>) {
        // 旧任务句柄 drop 即取消（旧通道随之关闭）
        self.tasks.clear();
        self.mailbox = None;
        self.aggregator = Arc::new(FrameAggregator::new());
        self.history = history;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.mailbox = Some(tx.clone());
        // 引擎事件汇也绑上本会话邮箱（终态三键等转发到主线程）
        self.events.set_mailbox(tx);
        self.spawn_stream_loop(rx, cx);
    }

    /// 心跳任务（泵 + 节流合一）：每 64ms 排空通道（帧进聚合器、事件
    /// 日志化、start_chat 完成回执分流）→ 产全量快照 → 会话实体
    /// `apply_snapshot`（唯一写入口）。
    ///
    /// 通道用 **`try_recv` 轮询**而非 `recv().await`：await 会注册 waker，
    /// tokio 线程 send 时跨线程唤醒 gpui 任务——gpui 测试调度器按单线程
    /// 确定性模型设计，跨线程活动直接判非确定性 panic；轮询形态下
    /// send 不触碰 gpui 调度器，帧的可见延迟至多一个节流窗口（与 64ms
    /// 节流同语义）。终态快照产出后**再等一个窗口**（让终态流式行至少
    /// 渲染过一帧）才落盘重载收口——与 `SnapshotTicker` 的「尾帧必达 +
    /// 产出终态后收口」同语义。
    fn spawn_stream_loop(
        &mut self,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<UiMail>,
        cx: &mut Context<Self>,
    ) {
        let aggregator = self.aggregator.clone();
        let history = self.history.clone();
        let conversation = self.conversation.clone();
        let state = self.state.clone();
        let session_id = self.session_id.clone().unwrap_or_default();
        let executor = cx.background_executor().clone();
        self.tasks.push(cx.spawn(async move |this, cx| {
            let mut terminal_emitted = false;
            loop {
                executor.timer(SNAPSHOT_INTERVAL).await;
                // 排空通道（通道关闭 = 管道已重建/服务释放，排完即止）
                loop {
                    match rx.try_recv() {
                        Ok(UiMail::Frame(frame)) => aggregator.push(frame),
                        Ok(UiMail::Event { key, payload }) => {
                            // 事件到达面（终态三键等）；通知等壳能力阶段 4 接
                            eprintln!("[craftwave] 事件 {key}: {payload}");
                        }
                        Ok(UiMail::Started(Ok(_run_id))) => {
                            // run 已注册，帧流已在跑；无 UI 动作
                        }
                        Ok(UiMail::Started(Err(err))) => {
                            // 命令面失败（未配模型/参数非法等，注册 run 前
                            // 即失败、零帧）：合成错误终态帧让心跳循环收口，
                            // 对齐旧壳 chat.rs 的命令失败路径
                            let seq = aggregator
                                .snapshot_streaming()
                                .and_then(|streaming| streaming.max_seq)
                                .map_or(0, |max| max + 1);
                            aggregator.push_seq(
                                seq,
                                ChatFrame::RunEnded {
                                    reason: EndReason::Error,
                                    duration_ms: 0,
                                    tool_count: 0,
                                    turn: 0,
                                    error: Some(FrameError::new(err.code, err.message)),
                                },
                            );
                            let _ =
                                this.update(cx, |service, cx| service.set_busy(false, cx));
                        }
                        Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                    }
                }
                if terminal_emitted {
                    // 终态已上屏一帧：落盘重载历史（run 落盘器在 runEnded
                    // 帧下发前已写完 canonical 条目），流式行由历史接管
                    let history = reload_history(&state, &session_id, history);
                    let _ = conversation.update(cx, |conversation, cx| {
                        conversation.apply_snapshot(history, None, cx);
                    });
                    let _ = this.update(cx, |service, cx| service.set_busy(false, cx));
                    return;
                }
                // 只取流式态（聚合器全量克隆的是流式面）；历史沿用本
                // run 固定的 Rc 指针——apply_snapshot 的指针相等对账让
                // 「同历史 + 流式增量」的快照只走 set_streaming（remeasure
                // 行高），不 reset 列表（滚动位置不炸）。conversation_
                // snapshot 的 to_vec 形态用于「历史确实换新」的场景
                //（终态重载），这里不用它
                let streaming = aggregator.snapshot_streaming();
                let ended = streaming
                    .as_ref()
                    .is_some_and(|streaming| streaming.ended.is_some());
                // 视图已释放（窗口关闭）：停止上屏但照常走到收口判定
                let _ = conversation.update(cx, |conversation, cx| {
                    conversation.apply_snapshot(history.clone(), streaming, cx);
                });
                if ended {
                    terminal_emitted = true;
                    continue;
                }
                if aggregator.is_aborted() {
                    // 本地中止且终态帧未到：直接收口（终态以 runEnded 帧为
                    // 权威——生产取消走 cancel_run，不会走到这里）
                    return;
                }
            }
        }));
    }

    /// 历史上屏（新历史指针推给会话实体）。
    fn push_history(&self, cx: &mut Context<Self>) {
        if let Some(conversation) = self.conversation.upgrade() {
            conversation.update(cx, |conversation, cx| {
                conversation.apply_snapshot(self.history.clone(), None, cx);
            });
        }
    }

    /// busy 状态推给会话实体（转 Composer 停止钮）。
    fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        self.busy = busy;
        self.push_busy(cx);
    }

    /// busy 下推（不经 self 可变借用的路径：泵任务回调里用）。
    fn push_busy(&self, cx: &mut Context<Self>) {
        if let Some(conversation) = self.conversation.upgrade() {
            let busy = self.busy;
            conversation.update(cx, |conversation, cx| {
                if let Some(composer) = &conversation.composer {
                    composer.update(cx, |composer, cx| composer.set_busy(busy, cx));
                }
            });
        }
    }
}

/// 本批默认运行配置：模型走全局激活配置（modelId None）、thinking 未设
/// （按 per-model reasoningEffort 回落）、权限档 AutoEdit（与 engine
/// followUp 队列的缺省档一致）。设置界面阶段 3 接入后由设置面提供。
fn default_run_config() -> RunConfig {
    RunConfig {
        model_id: None,
        thinking: None,
        tier: PermissionTier::AutoEdit,
        max_turns: DEFAULT_MAX_TURNS,
        steer: false,
    }
}

/// 终态后落盘重载：从会话树重建活动路径（run 落盘器的 canonical 条目
/// 此时已全部在盘上）。装载失败保留原历史（尽力而为，不崩收口流程）。
fn reload_history(
    state: &AppState,
    session_id: &str,
    fallback: Rc<Vec<Rc<MessageEntry>>>,
) -> Rc<Vec<Rc<MessageEntry>>> {
    match session::load_session_tree(
        state,
        LoadSessionTreeArgs {
            session_id: session_id.to_owned(),
        },
    ) {
        Ok(tree) => active_path(&tree.messages, tree.active_leaf_id.as_deref()),
        Err(_) => fallback,
    }
}

/// 全树消息 → 活动路径线性序（leaf 沿 parent_id 回溯到根再反转）。
/// 断链防御：父节点缺失时止于当前链（树数据损坏按已见部分展示）。
pub(crate) fn active_path(
    messages: &[MessageEntry],
    leaf: Option<&str>,
) -> Rc<Vec<Rc<MessageEntry>>> {
    let by_id: HashMap<&str, &MessageEntry> = messages
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    let mut chain: Vec<Rc<MessageEntry>> = Vec::new();
    let mut cursor = leaf;
    while let Some(id) = cursor
        && let Some(entry) = by_id.get(id)
    {
        chain.push(Rc::new((*entry).clone()));
        cursor = entry.parent_id.as_deref();
    }
    chain.reverse();
    Rc::new(chain)
}

/// @ 补全真源：engine `search_workspace_paths`（会话 roots BFS）经
/// 后台线程池异步执行（P95<300ms 的阻塞 IO，不占主线程）。结果由
/// Composer 侧绑查询词对账（过期丢弃），本源只管发起。
pub struct EngineCompletionSource {
    service: WeakEntity<SessionService>,
}

impl EngineCompletionSource {
    /// 用服务的弱引用构造（服务在 Global，寿命覆盖全部 composer）。
    pub fn new(service: WeakEntity<SessionService>) -> Self {
        Self { service }
    }
}

impl CompletionSource for EngineCompletionSource {
    /// 发起一次补全查询（Task 即发即弃：句柄由 composer 的回写任务持有）。
    /// 无会话（尚未发过消息）或服务已释放时返回空结果任务。
    fn search(&self, query: &str, cx: &mut App) -> Task<Vec<WorkspacePathHit>> {
        let Some(service) = self.service.upgrade() else {
            return Task::ready(Vec::new());
        };
        let Some((state, session_id)) = service.read(cx).search_context() else {
            return Task::ready(Vec::new());
        };
        let query = query.to_owned();
        cx.background_spawn(search_paths(state, session_id, query))
    }
}

/// `search_workspace_paths` 的降级包装（错误按空结果——补全是尽力而为
/// 的 UI 面，搜索失败不该打断输入）。跑 gpui 后台线程池（阻塞 IO 不占
/// 主线程；内部无 await，不需要 tokio）。
async fn search_paths(
    state: Arc<AppState>,
    session_id: String,
    query: String,
) -> Vec<WorkspacePathHit> {
    workspace_api::search_workspace_paths(
        &state,
        SearchWorkspacePathsArgs {
            session_id,
            query,
            limit: None,
        },
    )
    .unwrap_or_default()
}
