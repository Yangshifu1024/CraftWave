//! shell.rs 的纯逻辑单测（19 号票）：退出确认状态机的超时兜底口径。
//! 计时全部显式传入（Instant 加法确定性推进），不依赖真实时间流逝。

use std::time::{Duration, Instant};

use super::{EXIT_CONFIRM_TIMEOUT, ExitConfirm};

/// 固定推进的时钟：从 t0 起按秒数取时点（测试可读性 > 绝对时刻）。
fn at(t0: Instant, secs: u64) -> Instant {
    t0 + Duration::from_secs(secs)
}

#[test]
fn exit_confirm_times_out_without_frontend_reply() {
    let t0 = Instant::now();
    let mut confirm = ExitConfirm::new(EXIT_CONFIRM_TIMEOUT);
    assert!(!confirm.is_pending(), "初始无挂起");

    confirm.request(t0);
    assert!(confirm.is_pending(), "请求后进入挂起（等待回执）");
    assert!(
        !confirm.timed_out(at(t0, 9)),
        "时限内（9s < 10s）不兜底：前端可能正在弹框等用户操作"
    );
    assert!(confirm.is_pending(), "时限内判定不解除挂起");

    // 到点（≥10s）仍无回执 → 兜底放行
    assert!(confirm.timed_out(at(t0, 10)), "10s 无回执应判超时放行");
    assert!(!confirm.is_pending(), "兜底判定同时解除挂起");
    // 解除后重复判定不再放行（防兜底任务与回执竞态下的重复退出）
    assert!(
        !confirm.timed_out(at(t0, 100)),
        "已解除的挂起不应重复判超时"
    );
}

#[test]
fn exit_confirm_default_timeout_is_ten_seconds() {
    let t0 = Instant::now();
    let mut confirm = ExitConfirm::new(EXIT_CONFIRM_TIMEOUT);
    confirm.request(t0);
    assert!(
        !confirm.timed_out(at(t0, 10) - Duration::from_millis(1)),
        "默认 10s：差一毫秒不应放行"
    );
    assert!(confirm.timed_out(at(t0, 10)), "默认 10s：到点放行");
}

#[test]
fn exit_confirm_reject_reply_cancels_fallback() {
    let t0 = Instant::now();
    let mut confirm = ExitConfirm::new(EXIT_CONFIRM_TIMEOUT);
    confirm.request(t0);

    // 前端及时回执「取消退出」：解除挂起，此后任何时刻都不兜底
    assert!(!confirm.resolve(false), "取消回执不放行退出");
    assert!(!confirm.is_pending(), "回执后解除挂起");
    assert!(
        !confirm.timed_out(at(t0, 3600)),
        "已回执的请求不触发超时兜底（用户选择留下 = 保持托盘常驻）"
    );
}

#[test]
fn exit_confirm_accept_reply_exits_immediately() {
    let t0 = Instant::now();
    let mut confirm = ExitConfirm::new(EXIT_CONFIRM_TIMEOUT);
    confirm.request(t0);
    assert!(confirm.resolve(true), "确认回执应放行退出");
    assert!(!confirm.is_pending(), "回执后解除挂起");
}

#[test]
fn exit_confirm_repeat_request_refreshes_timer() {
    let t0 = Instant::now();
    let mut confirm = ExitConfirm::new(EXIT_CONFIRM_TIMEOUT);
    confirm.request(t0);
    // 8s 后用户再点一次退出：计时起点刷新
    confirm.request(at(t0, 8));
    assert!(
        !confirm.timed_out(at(t0, 10)),
        "重复请求刷新计时：第一次请求的 10s 到点不再放行"
    );
    assert!(
        confirm.timed_out(at(t0, 18)),
        "刷新后 10s（t0+18s）到点放行"
    );
}

#[test]
fn exit_confirm_no_request_never_times_out() {
    let t0 = Instant::now();
    let mut confirm = ExitConfirm::new(EXIT_CONFIRM_TIMEOUT);
    assert!(
        !confirm.timed_out(at(t0, 3600)),
        "从未请求过退出：任何时刻都不兜底"
    );
}
