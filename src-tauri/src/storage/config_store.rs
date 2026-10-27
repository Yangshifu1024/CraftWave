//! 配置协调器（§7.6，FR-14-03）：config.json 的唯一写入口。
//!
//! 写流程：入参校验/归一化 →（涉 newKey 时先走 §9.4 钥匙串协议）→ 临时文件
//! 写入 → fsync → rename → 内存缓存热替换 → 返回归一化结果。
//! 意向日志（intent.json）在外部落子（钥匙串）之前落盘，成功后删除；
//! 崩溃后重启发现意向日志则提示用户重试（不自动重放写操作）。
//!
//! 并发写协调（§7.4）：M1 单进程 + 单实例插件 → 无跨进程写并发；
//! 进程内以互斥锁串行化全部提交（含无外部落子的 save），跨进程
//! CW FileCoordinator 留 1.x。原子 rename 保证读侧永不看到半行。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::util::atomic;
use crate::util::errors::IpcError;

/// 协调器载荷约束：绑定具体配置类型的模块提供归一化实现
/// （providers::types::AppConfig，providers → storage 单向依赖）。
/// `Default` 用于全新安装（config.json 不存在）的首次装载；
/// `Clone` 用于锁内读-改-写时从缓存克隆出工作副本。
pub trait ConfigPayload:
    Serialize + DeserializeOwned + Default + Clone + Send + Sync + 'static
{
    fn normalize(&mut self) -> Result<(), IpcError>;
}

pub struct ConfigStore<T: ConfigPayload> {
    path: PathBuf,
    intent_path: PathBuf,
    /// 写串行锁（进程内并发写协调，§7.4）。
    write_lock: Mutex<()>,
    /// 内存缓存（热替换：提交成功后整体换成新 Arc）。
    cache: Mutex<Option<Arc<T>>>,
}

impl<T: ConfigPayload> ConfigStore<T> {
    /// `path`：config.json；`intent_path`：intent.json（同目录，§7.1）。
    pub fn new(path: PathBuf, intent_path: PathBuf) -> Self {
        Self {
            path,
            intent_path,
            write_lock: Mutex::new(()),
            cache: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn intent_path(&self) -> &Path {
        &self.intent_path
    }

    /// 读取当前配置（缓存优先；缓存 miss 时读盘 + 归一化 + 回填缓存）。
    /// 文件不存在（全新安装）→ 类型默认值，不落盘（首次保存才产生文件）。
    pub fn load(&self) -> Result<Arc<T>, IpcError> {
        {
            let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(hit) = cache.as_ref() {
                return Ok(hit.clone());
            }
        }
        let value = if self.path.exists() {
            let raw = fs::read(&self.path)
                .map_err(|e| IpcError::internal(format!("读取配置失败：{e}")))?;
            let mut parsed: T = serde_json::from_slice(&raw).map_err(|e| {
                IpcError::invalid_arg(format!("配置文件解析失败（schema v2）：{e}"))
            })?;
            parsed.normalize()?;
            parsed
        } else {
            T::default()
        };
        let arc = Arc::new(value);
        *self.cache.lock().unwrap_or_else(|p| p.into_inner()) = Some(arc.clone());
        Ok(arc)
    }

    /// 意向日志残留探测（崩溃恢复）：存在则返回其内容，由上层提示用户重试；
    /// 不自动重放写操作（§7.6）。
    pub fn pending_intent(&self) -> Result<Option<Value>, IpcError> {
        if !self.intent_path.exists() {
            return Ok(None);
        }
        let raw = fs::read(&self.intent_path)
            .map_err(|e| IpcError::internal(format!("读取意向日志失败：{e}")))?;
        let value: Value = serde_json::from_slice(&raw)
            .map_err(|e| IpcError::internal(format!("意向日志解析失败：{e}")))?;
        Ok(Some(value))
    }

    /// 无外部落子的整份提交（save_config）：归一化 → 原子写 → 缓存热替换。
    pub fn save(&self, mut next: T) -> Result<Arc<T>, IpcError> {
        let guard = self.write_lock.lock().unwrap_or_else(|p| p.into_inner());
        next.normalize()?;
        let arc = self.write_through(next)?;
        drop(guard);
        Ok(arc)
    }

    /// 锁内读-改-写（并发写协调的正确性前提：读当前配置必须在写锁内）。
    /// 用于"改一份既有配置"的增量语义（如 save_provider/delete_provider）。
    pub fn update(
        &self,
        f: impl FnOnce(&mut T) -> Result<(), IpcError>,
    ) -> Result<Arc<T>, IpcError> {
        self.commit_from_current(None, f)
    }

    /// 锁内读-改-写 + 外部落子（save_provider，§9.4）：
    /// 起点为锁内读到的当前配置 → intent 落盘 → `external(&mut next)`
    /// （钥匙串写入，可回填凭据字段）→ 归一化 → 原子替换 → 缓存热替换 → 删意向。
    /// external 失败：config 不变更、意向保留（下轮保存重试，opId 幂等去重）。
    pub fn commit_from_current(
        &self,
        intent: Option<Value>,
        external: impl FnOnce(&mut T) -> Result<(), IpcError>,
    ) -> Result<Arc<T>, IpcError> {
        let guard = self.write_lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut next = self.load()?.as_ref().clone();
        let wrote_intent = match intent {
            Some(record) => {
                atomic::write_json(&self.intent_path, &record)
                    .map_err(|e| IpcError::internal(format!("意向日志落盘失败：{e}")))?;
                true
            }
            None => false,
        };
        if let Err(e) = external(&mut next) {
            // 意向保留，config 不变更（§9.4 失败语义）
            drop(guard);
            return Err(e);
        }
        next.normalize()?;
        let arc = self.write_through(next)?;
        if wrote_intent {
            let _ = fs::remove_file(&self.intent_path);
        }
        drop(guard);
        Ok(arc)
    }

    /// 调用方自带完整新配置的提交（测试/特殊路径用）：
    /// intent 落盘 → `external(&mut next)` → 归一化 → 原子替换 → 缓存热替换 → 删意向。
    /// 注意：不走锁内读，读-改-写并发场景请用 `commit_from_current`/`update`。
    pub fn commit(
        &self,
        mut next: T,
        intent: Option<Value>,
        external: impl FnOnce(&mut T) -> Result<(), IpcError>,
    ) -> Result<Arc<T>, IpcError> {
        let guard = self.write_lock.lock().unwrap_or_else(|p| p.into_inner());
        let wrote_intent = match intent {
            Some(record) => {
                atomic::write_json(&self.intent_path, &record)
                    .map_err(|e| IpcError::internal(format!("意向日志落盘失败：{e}")))?;
                true
            }
            None => false,
        };
        if let Err(e) = external(&mut next) {
            // 意向保留，config 不变更（§9.4 失败语义）
            drop(guard);
            return Err(e);
        }
        next.normalize()?;
        let arc = self.write_through(next)?;
        if wrote_intent {
            let _ = fs::remove_file(&self.intent_path);
        }
        drop(guard);
        Ok(arc)
    }

    /// 前置：归一化已完成、写锁已持有。原子落盘 + 缓存热替换。
    fn write_through(&self, next: T) -> Result<Arc<T>, IpcError> {
        let bytes = serde_json::to_vec_pretty(&next)
            .map_err(|e| IpcError::internal(format!("配置序列化失败：{e}")))?;
        atomic::write_atomic(&self.path, &bytes)
            .map_err(|e| IpcError::internal(format!("配置原子落盘失败：{e}")))?;
        let arc = Arc::new(next);
        *self.cache.lock().unwrap_or_else(|p| p.into_inner()) = Some(arc.clone());
        Ok(arc)
    }
}

#[cfg(test)]
#[path = "config_store_test.rs"]
mod config_store_test;
