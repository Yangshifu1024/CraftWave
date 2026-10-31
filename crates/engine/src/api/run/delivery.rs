//! followUp 队列的自动投递（平移自旧壳 `host/commands/chat.rs`）：
//! run 正常完成后投递队列头并新开 run（走同一装配入口，压缩编排/wire
//! 切分天然继承）；撞车回队重试；其他失败回队 + 暂停等待用户明确继续。

use std::time::Duration;

use crate::app_state::AppState;
use crate::storage::{MessageEntry, MessageRole, WirePart};
use crate::util::errors::{E_SESSION_RUNNING, IpcError};

use super::model::{ModelResolver, RunTransport, RunWire};
use super::sinks::DetachedSink;

/// 等待会话空闲的重试参数：runEnded 与 remove 之间有并发 start_chat 撞车时
/// 队列头回队延迟重试（「投递循环不丢消息」）；总窗口约 5s，超时保留
/// 队列等下一次触发（下次 run 完成 / resume 命令），不丢消息。
const DELIVERY_RETRY_LIMIT: u32 = 50;
const DELIVERY_RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// `run:queue` 事件发射（实现住 `api::run`，delivery 域直接复用）。
use crate::api::run::emit_queue_state;

/// 自动投递队列头：等会话空闲（复查无活跃 run）→ 出队 →
/// 以附着 runConfig 构造 user 消息自动 start_chat 新 run（走同一入口）。
/// user 条目由装配侧 `persist_user_fallback` 兜底落盘（双写协议补落路径）。
///
/// 撞 E_SESSION_RUNNING 回队重试；其他失败（模型未配等命令面错误）回队 +
/// 暂停等待用户明确继续（`resume_followup_queue`）——两种路径都不丢消息。
/// `app` 为全部字段同源的共享句柄副本（[`AppState::share_handles`]）。
pub(crate) async fn deliver_followup_head<W: RunWire, T: RunTransport<W>>(
    app: &AppState,
    resolve: ModelResolver<W, T>,
    session_id: &str,
) -> Result<Option<String>, IpcError> {
    // ① 等会话空闲：runEnded 与 remove 之间并发的 start_chat 会占住会话
    for _ in 0..DELIVERY_RETRY_LIMIT {
        if app.runs.get(session_id).is_none() {
            break;
        }
        tokio::time::sleep(DELIVERY_RETRY_INTERVAL).await;
    }
    if app.runs.get(session_id).is_some() {
        // 空闲等待超时：保留队列（不投递），等下次 run 完成 / resume 触发
        return Ok(None);
    }
    // ② 出队前再查暂停态（resume 前的收尾投递竞态防御）
    if app.followups.snapshot(session_id).paused {
        return Ok(None);
    }
    let Some(entry) = app.followups.pop_head(session_id) else {
        return Ok(None);
    };
    emit_queue_state(
        &app.events,
        session_id,
        &app.followups,
        Vec::new(),
    );
    // ③ 构造 user 消息与 RunConfig（附着快照原样生效）。带图待发：
    //    附件引用展开为 image part（blob 已在 attach_image 落盘；
    //    wire 重建按投递时的模型视觉能力决定多模态块/占位文本）。
    let mut parts = vec![WirePart::Text {
        text: entry.text.clone(),
    }];
    for attachment in &entry.attachments {
        parts.push(WirePart::Image {
            blob_hash: attachment.blob_hash.clone(),
            mime_type: attachment.mime_type.clone(),
        });
    }
    let user_entry = MessageEntry {
        id: format!("auto-{}", entry.id),
        parent_id: None,
        role: MessageRole::User,
        parts,
        created_at: crate::storage::trust::now_ms(),
        run_id: None,
        status: None,
    };
    let run_config = super::RunConfig {
        model_id: entry.run_config.model_id.clone(),
        thinking: entry.run_config.thinking,
        tier: entry.run_config.tier,
        max_turns: entry.run_config.max_turns,
        steer: false,
    };
    // ④ 自动开新 run：帧出口用 DetachedSink（无前端通道，帧只入环——前端
    //    经 run:start 事件走 attach 观察态续流）
    let auto_confirm = app
        .config
        .load()
        .map(|c| c.approval.auto_confirm)
        .unwrap_or(false);
    match super::assemble::start_chat_assembled(
        app,
        &app.runs,
        session_id.to_owned(),
        vec![user_entry],
        run_config,
        Box::new(DetachedSink),
        auto_confirm,
        resolve,
    )
    .await
    {
        Ok(result) => Ok(Some(result.run_id)),
        Err(err) => {
            // 投递失败：队列头回队（不丢消息）。E_SESSION_RUNNING = 撞车，
            // 保留未暂停态由重试机制自然续投；其他错误（模型未配等）暂停
            // 队列等待用户明确继续（避免反复失败刷屏）
            app.followups.push_front(session_id, entry);
            if err.code != E_SESSION_RUNNING {
                app.followups.set_paused(session_id, true);
            }
            emit_queue_state(&app.events, session_id, &app.followups, Vec::new());
            Err(err)
        }
    }
}

/// 自动投递的异步触发（run 正常完成收尾用）：spawn 独立任务执行
/// [`deliver_followup_head`]，失败只记日志（队列态已回队/暂停，事件面可见）。
pub(crate) fn spawn_followup_delivery<W: RunWire, T: RunTransport<W>>(
    app: AppState,
    session_id: String,
    resolve: ModelResolver<W, T>,
) {
    tokio::spawn(async move {
        if let Err(e) = deliver_followup_head(&app, resolve, &session_id).await {
            #[cfg(debug_assertions)]
            eprintln!(
                "[craftwave] followUp 自动投递失败（{session_id}）：{}",
                e.message
            );
            #[cfg(not(debug_assertions))]
            let _ = (e, session_id);
        }
    });
}
