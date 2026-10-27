//! 进程内会话锁（§7.4 `MutexMap<SessionId>`）：串行化同会话存储操作。
//!
//! M1 单进程 + 单实例插件，无跨进程写并发（§7.4）；锁序约定 session → index
//! （index 本切片未引入，届时全局 index_lock 必须在会话锁内获取，禁止逆序）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// 会话 id → 进程内互斥量的惰性注册表。
#[derive(Default)]
pub struct LockMap {
    inner: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl LockMap {
    /// 取会话互斥量（惰性注册）。守卫由调用方持有：
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
