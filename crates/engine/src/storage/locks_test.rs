//! 会话锁单元测试（独立测试文件，源码零测试）。
//!
//! 测试面覆盖：①同一会话的锁互斥、不同会话的锁并行（s1 持锁时 s2 仍能立即
//! 获锁）；②锁释放后同一会话可再次获锁。

use std::sync::Arc;
use std::thread;

use super::LockMap;

// 断言：s1 持锁期间同会话 try_lock 必失败（互斥）、不同会话 s2 不受影响可立即
// 获锁（并行），释放后同会话可再次获锁。全部 try_lock 即时返回，无时序赌注。
#[test]
fn same_session_mutex_different_parallel() {
    let map = Arc::new(LockMap::default());
    let mutex = map.get("s1");
    let guard = mutex.lock().unwrap();

    let map2 = Arc::clone(&map);
    let blocked = thread::spawn(move || map2.get("s1").try_lock().is_err())
        .join()
        .unwrap();
    assert!(blocked, "s1 持锁期间同会话 try_lock 应失败");

    let map3 = Arc::clone(&map);
    let parallel = thread::spawn(move || map3.get("s2").try_lock().is_ok())
        .join()
        .unwrap();
    assert!(parallel, "s1 持锁不应影响 s2 获锁");

    drop(guard);
    let _again = mutex.try_lock().expect("释放后 s1 应可再次获锁");
}

// 断言：释放后再次取同一会话的锁并加锁成功（同一 id 拿到的是同一把锁且可复用）。
#[test]
fn session_lock_reentrant_after_release() {
    let map = LockMap::default();
    drop(map.get("s1").lock().unwrap());
    let mutex = map.get("s1");
    let _again = mutex.lock().unwrap();
}
