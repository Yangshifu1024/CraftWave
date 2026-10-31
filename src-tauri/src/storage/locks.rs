//! 进程内会话锁（`MutexMap<SessionId>`）：串行化同会话存储操作。
//!
//! 术语解释：「串行化」指让同一会话的多个操作排队逐个执行，而不是同时进行，
//! 防止两个线程交错写坏会话文件；「会话锁」是按会话 id 区分的互斥量——同一会话
//! 互斥，不同会话互不影响。
//!
//! M1 单进程 + 单实例插件，无跨进程写并发；锁序约定 session → index
//! （先拿会话锁再拿索引锁，index 本切片未引入；届时全局 index_lock 必须在会话锁
//! 内获取，禁止逆序，否则两个操作以相反顺序各持一把锁会互相等待、造成死锁）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 会话 id → 进程内互斥量的惰性注册表。「惰性」指第一次访问某个会话 id 时
/// 才创建它对应的互斥量，之后同一 id 总是返回同一个（克隆的 Arc 指向同一把锁）。
#[derive(Default)]
pub struct LockMap {
    inner: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

/// `LockMap` 的实现：只提供取锁一个入口，加锁与释放由调用方管理。
impl LockMap {
    /// 取会话互斥量（惰性注册，同一会话 id 永远拿到同一把锁）。守卫由调用方持有：
    /// `let m = locks.get(id); let _g = m.lock().unwrap();`（Arc 存活期即锁存活期）。
    pub fn get(&self, session_id: &str) -> Arc<Mutex<()>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(session_id.to_string())
            .or_default()
            .clone()
    }
}

#[cfg(test)]
#[path = "locks_test.rs"]
mod locks_test;
