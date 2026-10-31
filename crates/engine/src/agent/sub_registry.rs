//! 子代理运行时注册表与并发治理。
//!
//! 名词解释：子代理（subagent）是主代理委派任务的专职分身——独立上下文、
//! 独立取消令牌、独立审批槽的完整 run；注册表（[`SubRegistry`]）记录进程内
//! 全部活跃子代理，供 `stop_subagent` 命令、审批应答路由与用量/状态查询。
//!
//! - **并发闸**（[`ConcurrencyGate`]）：应用内所有会话共用的子代理并发上限
//!   （缺省 4，设置页可改并持久化）。满额排队不拒绝；运行中修改上限的热
//!   调整语义：已启动任务继续不取消；降低后暂停补入直到运行数量
//!   低于新上限（降低时先抽走空闲名额，不足部分记欠账——在飞任务释放时
//!   不归还，直到补齐差口）；提高后立即按新上限允许新增。与工具批并发
//!   （`tools.maxParallel`）互相独立。
//! - **取消级联**：子代理的取消令牌是父 run 令牌的
//!   子令牌（`CancellationToken::child_token`）——父取消自动传播到全部子
//!   代理；`stop_subagent` 单独取消指定子代理。
//! - **审批路由**：子代理的审批挂起使用自己的 ask 槽（挂 [`SubHandle`]），
//!   `resolve_ask` 在主 run 槽未命中时落到注册表按 askId 反查（与
//!   `AppState.app_asks` 同一兜底链）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::backend::exec::AskSlotCell;

/// 派发中子代理的句柄（活跃期挂注册表；`stop_subagent` 与审批路由的定位面）。
#[derive(Debug)]
pub struct SubHandle {
    /// 子代理 id（uuid；帧面 subFrame/subDone 与 Inspector 列表的关联键）。
    pub sub_id: String,
    /// 角色名。
    pub role: String,
    /// 父会话 id（事件载荷）。
    pub session_id: String,
    /// 父 run id（事件载荷与历史目录组织）。
    pub parent_run_id: String,
    /// 取消令牌（父 run 令牌的子令牌：父取消级联；stop_subagent 单独置位）。
    pub cancel: CancellationToken,
    /// 子代理自己的审批询问槽（与主 run 的 RunHandle.ask 同构）。
    pub ask: Arc<AskSlotCell>,
    /// 开始时刻（Unix 毫秒）。
    pub started_at: i64,
}

/// 动态并发闸的内部账目（互斥锁保护）：`limit` 当前上限；`debt` 降档欠账
/// ——降低上限时空闲名额不足，差额记欠账，在飞任务释放名额时不归还信号
/// 量而是清欠账，保证总名额（可发 + 在飞）精确等于新上限。
#[derive(Debug, Default)]
struct GateInner {
    limit: usize,
    debt: usize,
}

/// 子代理并发闸：全局共享的"发名额"原语。
///
/// 实现采用教科书式动态信号量：tokio `Semaphore` 承载可发名额与等待队列
/// （acquire 天然排队不拒绝），上限变更时——
/// - 提高：`add_permits(差值)`，排队任务立即获得新增名额；
/// - 降低：先 `try_acquire` 抽走空闲名额；空闲不足的差额记 `debt`，在飞
///   任务释放时按欠账顺序扣减（名额湮灭而不归还）。
///
/// 这满足"降低不取消已有任务、低于新上限后才补入"的全部热调整语义，且
/// 无自研等待循环的丢失唤醒风险。
#[derive(Debug)]
pub struct ConcurrencyGate {
    inner: Mutex<GateInner>,
    semaphore: tokio::sync::Semaphore,
}

impl ConcurrencyGate {
    /// 构造：初始上限 = 初始可发名额。
    pub fn new(limit: usize) -> Self {
        let limit = limit.max(1);
        Self {
            inner: Mutex::new(GateInner { limit, debt: 0 }),
            semaphore: tokio::sync::Semaphore::new(limit),
        }
    }

    /// 当前上限。
    pub fn limit(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .limit
    }

    /// 热调整上限：提高立即 add permits；降低抽空闲 + 记欠账。
    /// 已启动任务不受影响（不取消）。
    pub fn set_limit(&self, limit: usize) {
        let limit = limit.max(1);
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let old = inner.limit;
        inner.limit = limit;
        if limit > old {
            // 提高：新增名额立即可发（排队任务被信号量唤醒补入）
            drop(inner);
            self.semaphore.add_permits(limit - old);
        } else if limit < old {
            // 降低：先抽走空闲名额；抽不足的差额记欠账（在飞释放时湮灭，
            // 直到可发名额与在飞名额之和等于新上限）
            let mut deficit = old - limit;
            while deficit > 0 {
                if self.semaphore.try_acquire().is_ok() {
                    deficit -= 1;
                } else {
                    inner.debt += deficit;
                    deficit = 0;
                }
            }
        }
    }

    /// 取一个名额（取消感知）：满额时在信号量等待队列排队（不拒绝）；
    /// `token` 触发即放弃排队返回 false。
    ///
    /// 取到的 permit 被**消耗**（`mem::forget`，不走 tokio 的 drop 自动归还
    /// ——那样名额永远满额）：归还统一由 [`Self::release`] 承担（降档欠账
    /// 湮灭或正常 add 回信号量），保证"可发 + 在飞"总数精确等于当前上限。
    pub async fn acquire(&self, token: &CancellationToken) -> bool {
        tokio::select! {
            biased;
            _ = token.cancelled() => false,
            permit = self.semaphore.acquire() => {
                // 信号量不会被 close，acquire 只会成功
                std::mem::forget(permit);
                true
            }
        }
    }

    /// 释放名额：有降档欠账先清欠账（名额湮灭），否则归还信号量并唤醒
    /// 等待者。
    pub fn release(&self) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.debt > 0 {
            inner.debt -= 1;
            return;
        }
        drop(inner);
        self.semaphore.add_permits(1);
    }
}

/// 子代理注册表：活跃子代理的进程内登记（Clone = 同源共享句柄，Clone 即
/// 同一份注册表，供宿主命令与工具执行两侧使用）。
#[derive(Clone)]
pub struct SubRegistry {
    gate: Arc<ConcurrencyGate>,
    subs: Arc<Mutex<HashMap<String, Arc<SubHandle>>>>,
}

impl SubRegistry {
    /// 创建空注册表（上限取缺省 4；启动装配时按配置 set_limit 覆盖）。
    pub fn new(limit: usize) -> Self {
        Self {
            gate: Arc::new(ConcurrencyGate::new(limit)),
            subs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 并发闸（读取上限 / 热调整 / 发名额）。
    pub fn gate(&self) -> &ConcurrencyGate {
        &self.gate
    }

    /// 登记子代理句柄。
    pub fn register(&self, handle: Arc<SubHandle>) {
        self.subs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(handle.sub_id.clone(), handle);
    }

    /// 收尾摘除（按 subId 比对，防误摘）。
    pub fn remove(&self, sub_id: &str) {
        self.subs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(sub_id);
    }

    /// 按子代理 id 查活跃句柄。
    pub fn get(&self, sub_id: &str) -> Option<Arc<SubHandle>> {
        self.subs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(sub_id)
            .cloned()
    }

    /// 按未决询问的 askId 反查持有它的子代理（resolve_ask 兜底链的一环，
    /// 与 RunManager::find_by_ask / AppState.app_asks 同构）。
    pub fn find_by_ask(&self, ask_id: &str) -> Option<Arc<SubHandle>> {
        self.subs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .find(|h| {
                h.ask
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .is_some_and(|slot| slot.ask_id == ask_id)
            })
            .cloned()
    }

    /// 活跃子代理数（Inspector/排障用）。
    pub fn active_count(&self) -> usize {
        self.subs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// 级联取消某会话的全部子代理（cancel_run 的级联面；子代理令牌本是父
    /// run 令牌的子令牌、父取消已自动传播——本方法是命令面的显式兜底，
    /// 供父 run 令牌引用已丢失（如重启后无父 run）的场景）。
    pub fn cancel_by_session(&self, session_id: &str) -> usize {
        let subs = self
            .subs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut n = 0;
        for handle in subs.values() {
            if handle.session_id == session_id {
                handle.cancel.cancel();
                n += 1;
            }
        }
        n
    }
}
