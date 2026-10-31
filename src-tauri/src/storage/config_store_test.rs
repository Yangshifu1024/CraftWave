//! 配置协调器测试（独立测试文件）：tempdir + 内存类型。
//!
//! 测试面覆盖：全新安装不落盘直接给默认值、保存归一化后原子落盘可还原、
//! 校验失败时既有配置不受影响、update 失败/成功两种路径（含缓存热替换）、
//! unix 下文件权限收紧到 0600、多线程并发提交全部成功且终态文件完整。
//! 同型用例在单个 #[test] 内顺序合并，断言消息带用例标识。

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

// 断言：load/save 全语义——全新安装返回默认值且不主动落盘；save 前归一化
// 生效（空项剔除）、新实例读盘还原同值；归一化校验失败报 E_INVALID_ARG 且
// 磁盘既有配置原样保留。
#[test]
fn load_save_normalize_and_validation() {
    // 全新安装：默认值 + 不落盘
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let config = store.load().unwrap();
    assert_eq!(config.version, 2);
    assert!(!dir.path().join("config.json").exists(), "不主动落盘");

    // save：归一化生效 + 新实例（无缓存）读盘还原
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
    let store2 = store_in(dir.path());
    assert_eq!(
        store2.load().unwrap().items,
        vec!["a".to_string(), "b".to_string()]
    );

    // 校验失败：报 E_INVALID_ARG，磁盘既有配置原样保留
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
    assert_eq!(
        store.load().unwrap().items,
        vec!["keep".to_string()],
        "既有配置不受失败影响"
    );
}

// 断言：update 与环境守卫——修改闭包返回错误时整体失败既有配置保留；成功后
// 返回归一化结果且 load 命中同一份缓存对象（缓存热替换）；unix 下加载宽权限
// 旧文件顺手收紧 0600、save 落盘同样 0600；8 线程并发提交全部成功且终态是
// 某一次完整写入（非撕裂混合 JSON）。
#[test]
fn update_cache_swap_permissions_and_concurrency() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    store
        .save(DemoConfig {
            version: 2,
            items: vec!["keep".to_string()],
        })
        .unwrap();

    // update 失败：整体失败、既有配置保留
    let err = store
        .update(|cfg| {
            cfg.items.push("next".into());
            Err(IpcError::internal("模拟更新失败"))
        })
        .unwrap_err();
    assert_eq!(err.code, "E_INTERNAL");
    assert_eq!(
        store.load().unwrap().items,
        vec!["keep".to_string()],
        "失败不落盘"
    );

    // update 成功：返回归一化结果 + 缓存热替换（load 命中提交后的同一 Arc）
    let arc = store
        .update(|cfg| {
            cfg.items.push("from-update".into());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        arc.items,
        vec!["keep".to_string(), "from-update".to_string()]
    );
    assert!(Arc::ptr_eq(&store.load().unwrap(), &arc), "缓存热替换");

    // unix：加载既有宽权限文件收紧 0600；save 落盘同样 0600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.path().join("config.json");
        let wide = 0o644;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(wide)).unwrap();
        // 宽权限收紧发生在 load 既有文件时；这里重建一个宽权限文件再 load
        std::fs::write(&path, r#"{"version":2,"items":["a"]}"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(wide)).unwrap();
        let store3: ConfigStore<DemoConfig> = ConfigStore::new(path.clone());
        let _ = store3.load().unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "加载时收紧宽权限旧文件");
        store3
            .save(DemoConfig {
                version: 2,
                items: vec!["b".into()],
            })
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "save 落盘 0600");
    }

    // 并发提交：全部成功，终态是某一次完整写入（归一化后单元素）
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
    let final_config = store.load().unwrap();
    assert_eq!(final_config.items.len(), 1, "终态是某次完整写入");
    assert!(final_config.items[0].starts_with("w-"), "非撕裂混合 JSON");
}
