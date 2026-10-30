//! 并发闸与注册表测试（10 号票，FR-07-04/Q13 热调整语义）：
//! 满额排队不拒绝、降低不取消已有任务且低于新上限才补入、提高立即放行
//! 排队任务、取消传播级联、审批路由反查。

use super::*;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// 取名额是否在给定时间内完成（排队等待的可观察判定）。
async fn acquired_within(gate: &ConcurrencyGate, token: &CancellationToken, ms: u64) -> bool {
    tokio::time::timeout(Duration::from_millis(ms), gate.acquire(token))
        .await
        .unwrap_or(false)
}

/// 满额排队不拒绝：limit=1，第二个名额等待；释放后排队者获得名额。
#[tokio::test]
async fn gate_queues_when_full_and_admits_after_release() {
    let gate = Arc::new(ConcurrencyGate::new(1));
    let token = CancellationToken::new();
    assert!(
        acquired_within(&gate, &token, 100).await,
        "首个名额立即可得"
    );
    let second = CancellationToken::new();
    let waiter = tokio::spawn({
        let gate = gate.clone();
        async move { gate.acquire(&second).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !waiter.is_finished(),
        "满额时排队（不拒绝、不取消已有任务）"
    );
    gate.release();
    let got = tokio::time::timeout(Duration::from_millis(500), waiter)
        .await
        .expect("释放后排队者应获得名额")
        .unwrap();
    assert!(got);
}

/// 热降档（Q13）：4 运行降到 2 不取消已有任务；不补入直到运行数低于新上限。
#[tokio::test]
async fn lower_limit_waits_until_active_below_new_limit() {
    let gate = Arc::new(ConcurrencyGate::new(2));
    let token = CancellationToken::new();
    // 占满 2 个名额
    assert!(gate.acquire(&token).await);
    assert!(gate.acquire(&token).await);
    // 降到 1：抽空闲名额抽不到 → 记欠账 1（已有任务不取消）
    gate.set_limit(1);
    let queued = CancellationToken::new();
    let waiter = tokio::spawn({
        let gate = Arc::clone(&gate);
        async move { gate.acquire(&queued).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !waiter.is_finished(),
        "降档后活跃数(2) ≥ 新上限(1)：排队任务不补入"
    );
    // 释放 1 个：欠账清 1（名额湮灭，不归还信号量）→ 活跃数 1 仍 ≥ 上限 → 继续排队
    gate.release();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!waiter.is_finished(), "活跃数(1) ≥ 新上限(1)：仍不补入");
    // 再释放：活跃数 0 < 1 → 排队任务获得名额
    gate.release();
    let got = tokio::time::timeout(Duration::from_millis(500), waiter)
        .await
        .expect("活跃数低于新上限后应补入")
        .unwrap();
    assert!(got);
}

/// 热升档（Q13）：提高后立即按新上限启动排队任务。
#[tokio::test]
async fn raise_limit_admits_immediately() {
    // Arc 包装便于把同一闸移进两个 spawn 闭包（tokio::spawn 需 'static）
    let gate = Arc::new(ConcurrencyGate::new(1));
    let token = CancellationToken::new();
    assert!(gate.acquire(&token).await, "占满初始名额");
    gate.set_limit(3);
    let a = CancellationToken::new();
    let b = CancellationToken::new();
    let waiter_a = tokio::spawn({
        let gate = gate.clone();
        async move { gate.acquire(&a).await }
    });
    let waiter_b = tokio::spawn({
        let gate = gate.clone();
        async move { gate.acquire(&b).await }
    });
    let (ra, rb) = tokio::join!(
        tokio::time::timeout(Duration::from_millis(500), waiter_a),
        tokio::time::timeout(Duration::from_millis(500), waiter_b),
    );
    assert!(ra.unwrap().unwrap(), "升档后排队者 A 立即获得名额");
    assert!(rb.unwrap().unwrap(), "升档后排队者 B 立即获得名额");
}

/// 排队中取消：名额未取得即退出（委派未启动面）。
#[tokio::test]
async fn acquire_aborts_when_cancelled_while_queued() {
    let gate = Arc::new(ConcurrencyGate::new(1));
    let holder = CancellationToken::new();
    assert!(gate.acquire(&holder).await);
    let queued = CancellationToken::new();
    let queued_for_task = queued.clone();
    let waiter = tokio::spawn({
        let gate = Arc::clone(&gate);
        async move { gate.acquire(&queued_for_task).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    queued.cancel();
    let got = tokio::time::timeout(Duration::from_millis(500), waiter)
        .await
        .expect("取消后应退出等待")
        .unwrap();
    assert!(!got, "排队中被取消 → 未取得名额");
    gate.release();
}

/// 父取消级联（S5）：子令牌是父令牌的子令牌——父取消传播到全部子代理；
/// 子代理单独取消不影响父。
#[test]
fn parent_cancel_cascades_to_children() {
    let parent = CancellationToken::new();
    let child_a = parent.child_token();
    let child_b = parent.child_token();
    assert!(!child_a.is_cancelled());
    parent.cancel();
    assert!(child_a.is_cancelled(), "父取消级联到子 A");
    assert!(child_b.is_cancelled(), "父取消级联到子 B");
    // 反向：单独取消子不波及父
    let parent2 = CancellationToken::new();
    let child = parent2.child_token();
    child.cancel();
    assert!(
        !parent2.is_cancelled(),
        "stop_subagent 单独取消不影响父 run"
    );
}

/// 注册表：登记/摘除/active_count 与 cancel_by_session 级联兜底。
#[test]
fn registry_register_remove_and_cancel_by_session() {
    let registry = SubRegistry::new(4);
    let mk = |sub_id: &str, session: &str| {
        Arc::new(SubHandle {
            sub_id: sub_id.to_owned(),
            role: "explore".to_owned(),
            session_id: session.to_owned(),
            parent_run_id: "p1".to_owned(),
            cancel: CancellationToken::new(),
            ask: Arc::new(std::sync::Mutex::new(None)),
            started_at: 0,
        })
    };
    registry.register(mk("s1", "sess-1"));
    registry.register(mk("s2", "sess-2"));
    assert_eq!(registry.active_count(), 2);
    // 按会话级联：只取消 sess-1 的子代理
    assert_eq!(registry.cancel_by_session("sess-1"), 1);
    assert!(registry.get("s1").unwrap().cancel.is_cancelled());
    assert!(!registry.get("s2").unwrap().cancel.is_cancelled());
    registry.remove("s1");
    assert!(registry.get("s1").is_none());
    assert_eq!(registry.active_count(), 1);
}

/// 审批路由：askId 反查命中活跃子代理（resolve_ask 兜底链）。
#[test]
fn registry_find_by_ask_routes_to_sub() {
    use crate::backend::ask::{AskAnswer, AskSlot};
    let registry = SubRegistry::new(4);
    let (tx, rx) = tokio::sync::oneshot::channel::<AskAnswer>();
    let handle = Arc::new(SubHandle {
        sub_id: "s9".to_owned(),
        role: "reviewer".to_owned(),
        session_id: "sess-9".to_owned(),
        parent_run_id: "p9".to_owned(),
        cancel: CancellationToken::new(),
        ask: Arc::new(std::sync::Mutex::new(Some(AskSlot {
            ask_id: "ask-1".to_owned(),
            session_id: "sess-9".to_owned(),
            run_id: "sub-run".to_owned(),
            kind: "approval",
            payload: crate::backend::ask::AskPayload::Approval {
                title: "t".into(),
                diff: None,
                command: None,
                fence_reason: None,
                preview_html: None,
                options: vec![],
            },
            tx,
        }))),
        started_at: 0,
    });
    registry.register(handle);
    let hit = registry.find_by_ask("ask-1").expect("askId 反查命中");
    assert_eq!(hit.sub_id, "s9");
    // find_by_ask 是幂等查询（resolve_ask 才取走槽位）
    assert!(
        registry.find_by_ask("ask-1").is_some(),
        "查询不改状态（幂等）"
    );
    // 模拟 resolve_ask 的 take：槽位清空后不再命中
    *hit.ask.lock().unwrap() = None;
    assert!(registry.find_by_ask("ask-1").is_none());
    drop(rx);
}
