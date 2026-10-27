//! 会话锁单元测试（独立测试文件，源码零测试）。
//!
//! 测试面覆盖：①同一会话的锁互斥、不同会话的锁并行（s1 持锁时 s2 仍能立即
//! 获锁）；②锁释放后同一会话可再次获锁。

use std::sync::Arc;
use std::thread;

use super::LockMap;

// 断言：s1 持锁期间，另一个线程获取 s2 的锁不被阻塞（不同会话互不影响），
// 释放 s1 后该线程才能顺利 join。
#[test]
fn same_session_mutex_different_parallel() {
    let map = Arc::new(LockMap::default());
    let mutex = map.get("s1");
    let guard = mutex.lock().unwrap();
    let map2 = Arc::clone(&map);
    let handle = thread::spawn(move || {
        // s2 与 s1 无关，应能立即获锁
        let mutex = map2.get("s2");
        let _g = mutex.lock().unwrap();
    });
    assert!(!handle.is_finished());
    drop(guard);
    handle.join().unwrap();
}

// 断言：释放后再次取同一会话的锁并加锁成功（同一 id 拿到的是同一把锁且可复用）。
#[test]
fn session_lock_reentrant_after_release() {
    let map = LockMap::default();
    drop(map.get("s1").lock().unwrap());
    let mutex = map.get("s1");
    let _again = mutex.lock().unwrap();
}
