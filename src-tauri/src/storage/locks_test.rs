//! 会话锁单元测试（独立测试文件，源码零测试）。

use std::sync::Arc;
use std::thread;

use super::LockMap;

#[test]
fn 同一会话互斥_不同会话并行() {
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

#[test]
fn 会话锁释放后可重入() {
    let map = LockMap::default();
    drop(map.get("s1").lock().unwrap());
    let mutex = map.get("s1");
    let _again = mutex.lock().unwrap();
}
