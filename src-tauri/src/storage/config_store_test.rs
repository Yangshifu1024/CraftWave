//! 配置协调器测试（独立测试文件）：tempdir + 内存类型。
//!
//! 测试面覆盖：全新安装不落盘直接给默认值、保存归一化后原子落盘可还原、
//! 校验失败时既有配置不受影响、update 失败/成功两种路径（含缓存热替换）、
//! unix 下文件权限收紧到 0600、多线程并发提交全部成功且终态文件完整。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{ConfigPayload, ConfigStore};
use crate::util::errors::{E_INVALID_ARG, IpcError};

// 测试用的演示配置类型：一个版本号 + 一个字符串列表，足以覆盖归一化与校验语义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DemoConfig {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    items: Vec<String>,
}

// 旧配置文件缺 version 字段时的默认值。
fn default_version() -> u32 {
    2
}

// 全新安装（无文件）时的默认配置。
impl Default for DemoConfig {
    fn default() -> Self {
        Self {
            version: 2,
            items: Vec::new(),
        }
    }
}

// 演示配置的归一化实现：version 必须为 2，items 去掉空字符串。
impl ConfigPayload for DemoConfig {
    fn normalize(&mut self) -> Result<(), IpcError> {
        if self.version != 2 {
            return Err(IpcError::invalid_arg("version 必须为 2"));
        }
        self.items.retain(|i| !i.is_empty());
        Ok(())
    }
}

// 在临时目录里建一个指向 config.json 的协调器实例。
fn store_in(dir: &std::path::Path) -> ConfigStore<DemoConfig> {
    ConfigStore::new(dir.join("config.json"))
}

// 断言：全新安装（文件不存在）load 返回类型默认值，且不主动创建 config.json。
#[test]
fn fresh_install_load_default_no_write() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let config = store.load().unwrap();
    assert_eq!(config.version, 2);
    assert!(!dir.path().join("config.json").exists(), "不主动落盘");
}

// 断言：save 前归一化生效（空项被剔除），新实例（无内存缓存）读盘能还原同值。
#[test]
fn save_normalized_atomic_load_restores() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let saved = store
        .save(DemoConfig {
            version: 2,
            items: vec!["a".into(), String::new(), "b".into()],
        })
        .unwrap();
    assert_eq!(
        saved.items,
        vec!["a".to_string(), "b".to_string()],
        "归一化生效"
    );

    // 新实例（无缓存）读盘还原
    let store2 = store_in(dir.path());
    let loaded = store2.load().unwrap();
    assert_eq!(loaded.items, vec!["a".to_string(), "b".to_string()]);
}

// 断言：save 归一化校验失败时报 E_INVALID_ARG，且磁盘上的既有配置原样保留。
#[test]
fn save_validation_failure_config_intact() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    store
        .save(DemoConfig {
            version: 2,
            items: vec!["keep".into()],
        })
        .unwrap();

    let err = store
        .save(DemoConfig {
            version: 9,
            items: vec![],
        })
        .unwrap_err();
    assert_eq!(err.code, E_INVALID_ARG);
    assert_eq!(store.load().unwrap().items, vec!["keep".to_string()]);
}

// 断言：update 的修改闭包返回错误时整体失败，既有配置原样保留。
#[test]
fn update_failure_keeps_config_intact() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    store
        .save(DemoConfig {
            version: 2,
            items: vec!["keep".into()],
        })
        .unwrap();

    let err = store
        .update(|cfg| {
            cfg.items.push("next".into());
            Err(IpcError::internal("模拟更新失败"))
        })
        .unwrap_err();
    assert_eq!(err.code, "E_INTERNAL");
    assert_eq!(store.load().unwrap().items, vec!["keep".to_string()]);
}

// 断言：update 成功后返回归一化结果，且后续 load 命中的是同一份缓存对象
//（指针相等，证明发生了缓存热替换而非重新读盘）。
#[test]
fn update_success_cache_swapped() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    store
        .save(DemoConfig {
            version: 2,
            items: vec!["x".to_string()],
        })
        .unwrap();
    let arc = store
        .update(|cfg| {
            cfg.items.push("from-update".into());
            Ok(())
        })
        .unwrap();
    assert_eq!(arc.items, vec!["x".to_string(), "from-update".to_string()]);
    // 缓存热替换：load 命中提交后的同一 Arc
    assert!(Arc::ptr_eq(&store.load().unwrap(), &arc));
}

// 断言（仅 unix）：加载既有宽权限文件时顺手收紧到 0600；save 落盘的新文件同样是 0600。
#[cfg(unix)]
#[test]
fn config_file_permissions_tightened_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");

    // 既有宽权限文件：load 时顺手收紧
    std::fs::write(&path, r#"{"version":2,"items":["a"]}"#).unwrap();
    let wide = 0o644;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(wide)).unwrap();
    let store = ConfigStore::new(path.clone());
    let _ = store.load().unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "加载时收紧宽权限旧文件");

    // save 落盘同样 0600
    store
        .save(DemoConfig {
            version: 2,
            items: vec!["b".into()],
        })
        .unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

// 断言：8 个线程并发提交全部成功，终态文件是某一次完整写入的结果
//（归一化后单元素），而不是多次写入互相撕裂的混合 JSON。
#[test]
fn concurrent_commits_all_succeed_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(store_in(dir.path()));
    let mut handles = Vec::new();
    for i in 0..8 {
        let store = store.clone();
        handles.push(std::thread::spawn(move || {
            store
                .save(DemoConfig {
                    version: 2,
                    items: vec![format!("w-{i}")],
                })
                .unwrap();
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    // 终态必须是某个完整写入（归一化后单元素），不能是撕裂/混合 JSON
    let final_config = store.load().unwrap();
    assert_eq!(final_config.items.len(), 1);
    assert!(final_config.items[0].starts_with("w-"));
}
