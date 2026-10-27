//! 配置协调器（§7.6，FR-14-03）：config.json 的唯一写入口。
//!
//! 写流程：入参校验/归一化 → 临时文件写入 → fsync → rename →（unix 下
//! 权限收紧 0600，Key 明文承载后的边界，2026-10-28 用户决策）→ 内存缓存
//! 热替换 → 返回归一化结果。
//!
//! 并发写协调（§7.4）：M1 单进程 + 单实例插件 → 无跨进程写并发；
//! 进程内以互斥锁串行化全部提交；跨进程 CW FileCoordinator 留 1.x。
//! 原子 rename 保证读侧永不看到半行。
//!
//! （历史注记：keyring 时代的 intent.json 意向日志随 §9.4 废除一并移除。）

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde::de::DeserializeOwned;

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
    /// 写串行锁（进程内并发写协调，§7.4）。
    write_lock: Mutex<()>,
    /// 内存缓存（热替换：提交成功后整体换成新 Arc）。
    cache: Mutex<Option<Arc<T>>>,
}

impl<T: ConfigPayload> ConfigStore<T> {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            write_lock: Mutex::new(()),
            cache: Mutex::new(None),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读取当前配置（缓存优先；缓存 miss 时读盘 + 归一化 + 回填缓存）。
    /// 文件不存在（全新安装）→ 类型默认值，不落盘（首次保存才产生文件）。
    /// 既有文件权限宽于 0600 时顺手收紧（unix；旧文件一次性矫正）。
    pub fn load(&self) -> Result<Arc<T>, IpcError> {
        {
            let cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(hit) = cache.as_ref() {
                return Ok(hit.clone());
            }
        }
        let value = if self.path.exists() {
            restrict_permissions(&self.path);
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
        let guard = self.write_lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut next = self.load()?.as_ref().clone();
        f(&mut next)?;
        next.normalize()?;
        let arc = self.write_through(next)?;
        drop(guard);
        Ok(arc)
    }

    /// 前置：归一化已完成、写锁已持有。原子落盘 + 权限收紧 + 缓存热替换。
    fn write_through(&self, next: T) -> Result<Arc<T>, IpcError> {
        let bytes = serde_json::to_vec_pretty(&next)
            .map_err(|e| IpcError::internal(format!("配置序列化失败：{e}")))?;
        atomic::write_atomic(&self.path, &bytes)
            .map_err(|e| IpcError::internal(format!("配置原子落盘失败：{e}")))?;
        restrict_permissions(&self.path);
        let arc = Arc::new(next);
        *self.cache.lock().unwrap_or_else(|p| p.into_inner()) = Some(arc.clone());
        Ok(arc)
    }
}

/// config.json 权限收紧（unix：0600；windows：no-op）。
/// 失败不阻断（best-effort：权限是纵深防御层，主边界是本机用户会话）。
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path)
            && meta.permissions().mode() & 0o077 != 0
            && let Ok(mut perms) = fs::metadata(path).map(|m| m.permissions())
        {
            perms.set_mode(0o600);
            let _ = fs::set_permissions(path, perms);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
#[path = "config_store_test.rs"]
mod config_store_test;
