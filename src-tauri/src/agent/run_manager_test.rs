//! RunManager 单元测试：同会话互斥（E_SESSION_RUNNING）、取消置位、终态摘除。

use super::RunManager;

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

#[test]
fn cancel_hits_active_run_true_missing_false() {
    let mgr = RunManager::new();
    assert!(!mgr.cancel("s1"), "无活跃 run → false");
    let handle = mgr.start("s1", "run-1".to_owned(), 1).unwrap();
    assert!(!handle.cancel.is_cancelled());
    assert!(mgr.cancel("s1"), "命中 → 置位并返回 true（§3.3 #19）");
    assert!(handle.cancel.is_cancelled());
}

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
