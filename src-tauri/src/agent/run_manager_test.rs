//! RunManager 单元测试（测试代码独立于源码文件，仅 cargo test 时编译）。
//!
//! 覆盖三个核心行为：同会话互斥（第二个 run 被拒，返回 E_SESSION_RUNNING）、
//! 取消只做置位（命中返回 true、未命中返回 false）、run 终态摘除与摘除后
//! 允许同会话重启。

use super::RunManager;

/// 断言同会话的第二个 run 被拒（E_SESSION_RUNNING），不同会话不受影响。
#[test]
fn second_run_same_session_rejected() {
    let mgr = RunManager::new();
    mgr.start("s1", "run-1".to_owned(), 1)
        .expect("首 run 应注册");
    let err = mgr.start("s1", "run-2".to_owned(), 2).unwrap_err();
    assert_eq!(err.code, "E_SESSION_RUNNING", "§3.3 #18 同会话互斥");
    // 不同会话不受影响
    mgr.start("s2", "run-3".to_owned(), 3)
        .expect("他会话可注册");
}

/// 断言 cancel 在无活跃 run 时返回 false；命中活跃 run 时置位令牌并返回 true。
#[test]
fn cancel_hits_active_run_true_missing_false() {
    let mgr = RunManager::new();
    assert!(!mgr.cancel("s1"), "无活跃 run → false");
    let handle = mgr.start("s1", "run-1".to_owned(), 1).unwrap();
    assert!(!handle.cancel.is_cancelled());
    assert!(mgr.cancel("s1"), "命中 → 置位并返回 true（§3.3 #19）");
    assert!(handle.cancel.is_cancelled());
}

/// 断言 remove 会校验 run_id（不匹配不摘除，防止竞态摘掉新 run），
/// 正确摘除后同会话可以再次启动新 run。
#[test]
fn remove_validates_run_id_allows_restart() {
    let mgr = RunManager::new();
    mgr.start("s1", "run-1".to_owned(), 1).unwrap();
    // run_id 不匹配不摘（防竞态摘掉新 run）
    mgr.remove("s1", "run-stale");
    assert!(mgr.get("s1").is_some());
    mgr.remove("s1", "run-1");
    assert!(mgr.get("s1").is_none(), "终态摘除");
    mgr.start("s1", "run-2".to_owned(), 2)
        .expect("摘除后同会话可再启动");
}

/// 断言 has_active 与注册表空/非空一致：空表放行应用退出，任一会话有
/// 活跃 run 则拦截退出。
#[test]
fn has_active_reflects_registry_emptiness() {
    let mgr = RunManager::new();
    assert!(!mgr.has_active(), "空注册表 → 无活跃 run（退出拦截放行）");
    mgr.start("s1", "run-1".to_owned(), 1).unwrap();
    assert!(mgr.has_active(), "任一会话有活跃 run → 拦截退出（§9.6）");
    mgr.remove("s1", "run-1");
    assert!(!mgr.has_active());
}
