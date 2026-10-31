//! 引擎桥（gpui-rewrite 阶段 2.C-2b）：GPUI 壳与 craftwave-engine 之间的
//! 线程模型与通道基础设施。会话域的业务装配（发送链 / 帧管道）在
//! `chat/service.rs` 的 [`crate::chat::service::SessionService`]，本模块只
//! 承担与具体 feature 无关的三件事：
//!
//! ## 1. 常驻 tokio 运行时（[`EngineRuntime`] Global）
//!
//! engine 的 run 装配树（`api::run::start_chat` 装配 + `tokio::spawn`
//! 的 run 任务）要求调用方自备 tokio 运行时（`run.rs` 模块文档的壳约定）。
//! 壳在启动时装一个多线程 `Runtime` 挂 Global：engine 的异步调用
//! （start_chat / 后续 resolve_ask 等）经 `runtime.spawn(...)` 跑在 tokio
//! 线程池上；GPUI 侧（主线程）不跑 engine 的 future。
//!
//! ## 2. 帧桥（[`FrameBridge`]）与 UI 邮包（[`UiMail`]）
//!
//! run 任务在 tokio 线程上产出帧（`FrameHub` → `SequencedSink`）。帧本身
//! 含 `Rc` 的消费形态不能跨线程，因此把「生产在 tokio、消费在主线程」的
//! 段用 `tokio::sync::mpsc` 无界通道连接（tokio 的同步原语不依赖 tokio
//! 调度器，接收端可以直接在 gpui 的本地任务里 await）：
//!
//! ```text
//! tokio 线程                          主线程（gpui）
//! run 任务 → FrameHub → FrameBridge ──UiMail──▶ 泵任务（cx.spawn）
//! （send_seq = channel send）            recv → FrameAggregator.push
//!                                        节流循环（64ms timer）→ 快照
//!                                        → ConversationState::apply_snapshot
//! ```
//!
//! ## 3. 事件汇轻实现（[`MailEventSink`]）
//!
//! engine 的 `EventSink` 事件（终态三键 run:done / run:error /
//! run:cancelled、tokens:update 等）经同一通道转派发到主线程——本批只
//! 到达（日志化），系统通知等壳能力按阶段 4 计划再接。事件发射是尽力
//! 而为语义：无邮箱（run 尚未装配/已收口）时丢弃不影响主链路。

use std::sync::{Arc, Mutex};

use gpui_kit::{App, Global};

use craftwave_engine::app_state::AppState;
use craftwave_engine::backend::frames::{SequencedFrame, SequencedSink};
use craftwave_engine::util::events::EventSink;
use tokio::sync::mpsc;

use crate::prefs;

/// 常驻 tokio 运行时（多线程调度；进程生命周期内唯一）。挂 Global：
/// engine 的异步调用入口从这里取运行时句柄。
pub struct EngineRuntime(Arc<tokio::runtime::Runtime>);

impl Global for EngineRuntime {}

/// 取常驻 tokio 运行时。未装配即 panic——装配序错误（漏调
/// [`install_runtime`]）应当显式暴露而不是静默重建。
pub fn runtime(cx: &App) -> Arc<tokio::runtime::Runtime> {
    cx.global::<EngineRuntime>().0.clone()
}

/// 启动装配（main 最先调用）：建多线程 tokio 运行时挂 Global。
/// 独立于 AppState 的装配，让不触引擎的测试也能只装运行时。
pub fn install_runtime(cx: &mut App) {
    let rt = tokio::runtime::Runtime::new().expect("tokio 运行时创建失败");
    cx.set_global(EngineRuntime(Arc::new(rt)));
}

/// UI 邮包：tokio 侧 → 主线程泵的统一信封。帧与事件同走一条通道，
/// 泵端单点消费（两个来源两个通道会让泵任务变 select 循环，不值当——
/// 两者都只是「到达主线程就处理」的尽力而为消息）。
pub enum UiMail {
    /// 一帧（run 任务的帧出口产物；seq/ts 已由 FrameHub 填好）。
    Frame(SequencedFrame),
    /// 一条引擎事件（EventSink 的转发；key 取 `util::events::keys` 常量）。
    Event {
        key: String,
        payload: serde_json::Value,
    },
    /// start_chat 调用的完成回执（tokio 侧 spawn 的调用完成后回传）。
    /// Ok = run 已注册（帧流开始）；Err = 注册前失败（零帧，壳侧合成
    /// 错误终态收口，对齐旧壳 chat.rs 的命令失败路径）。
    Started(Result<String, craftwave_engine::util::errors::IpcError>),
}

/// 帧桥：`SequencedSink` 的壳实现——run 任务在 tokio 线程调用
/// `send_seq`，帧经无界通道送往主线程泵。发送失败（接收端已关闭 = 泵任务
/// 已被取消，例如用户切换会话）按「尽力而为」丢弃：帧仍在 FrameHub 的
/// 环形缓冲里，`attach_run` 可重放。
pub struct FrameBridge(mpsc::UnboundedSender<UiMail>);

impl FrameBridge {
    /// 用当前会话的邮包发送端构造桥。
    pub fn new(tx: mpsc::UnboundedSender<UiMail>) -> Self {
        Self(tx)
    }
}

impl SequencedSink for FrameBridge {
    /// 下发一帧：塞进通道即返回（不阻塞 run 任务；无界通道不做反压，
    /// 与旧壳 Tauri Channel 的语义一致）。
    fn send_seq(&mut self, frame: SequencedFrame) {
        let _ = self.0.send(UiMail::Frame(frame));
    }
}

/// 事件汇轻实现：engine 的 `Arc<dyn EventSink>` 生产实现——emit 即转发
/// 进同一 UiMail 通道（终态三键转派发到主线程；通知等壳能力阶段 4 再接）。
///
/// 邮箱用 `Mutex<Option<...>>` 延迟绑定：`AppState::assemble` 要先有
/// EventSink 才能构造，而会话的通道在每次发消息时才建立——两端装配
/// 时机天然错开。无邮箱（run 间隙/已收口）时事件直接丢弃（尽力而为）。
pub struct MailEventSink(Mutex<Option<mpsc::UnboundedSender<UiMail>>>);

impl MailEventSink {
    /// 构造（初始无邮箱）。
    pub fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// 绑定 / 更换邮箱（每次发消息建立新通道时调用；旧通道的发送端
    /// 被替换即关闭，旧事件随后自然丢弃）。
    pub fn set_mailbox(&self, tx: mpsc::UnboundedSender<UiMail>) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx);
    }
}

impl Default for MailEventSink {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSink for MailEventSink {
    /// 转发一条事件到主线程（无邮箱则丢弃）。失败路径不存在——无界
    /// 通道 send 只在接收端关闭时出错，同上按尽力而为丢弃。
    fn emit(&self, key: &str, payload: serde_json::Value) {
        let mailbox = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(tx) = mailbox {
            let _ = tx.send(UiMail::Event {
                key: key.to_owned(),
                payload,
            });
        }
    }
}

/// 引擎桥句柄（Global）：AppState（引擎调用面）+ 事件汇具体类型句柄
/// （发消息时给事件汇绑定会话邮箱）。会话服务（chat::service）经它装配。
pub struct EngineBridge {
    /// 引擎应用状态（AppState::assemble 产物）。
    pub state: Arc<AppState>,
    /// 事件汇具体类型（`state.events` 存的是 trait 对象，绑邮箱需要它）。
    pub events: Arc<MailEventSink>,
}

impl Global for EngineBridge {}

/// 取引擎桥（owned 副本：Arc 克隆零深拷贝；未装配 = None——测试/回声
/// 装配不装引擎）。返回 owned 而非引用：调用方随后还要以 &mut 借用
/// `App`（开窗 / 建服务），借用形态会撞可变借用。
pub fn bridge(cx: &App) -> Option<EngineBridge> {
    cx.try_global::<EngineBridge>().map(|bridge| EngineBridge {
        state: bridge.state.clone(),
        events: bridge.events.clone(),
    })
}

/// 引擎桥装配（生产入口；`main` 在组件层初始化后调用）。
///
/// - tokio 运行时挂 Global（[`EngineRuntime`]）；
/// - 数据目录按壳口径 `~/.craftwave`（[`prefs::data_dir`]，与旧壳一致）；
/// - 内置技能根 dev 传 None（源码树回退；打包态由阶段 5 打包脚本解析
///   资源目录后传入）。
pub fn install(cx: &mut App) {
    install_runtime(cx);
    let events = Arc::new(MailEventSink::new());
    let state = AppState::assemble(prefs::data_dir(), events.clone(), None);
    cx.set_global(EngineBridge { state, events });
}
