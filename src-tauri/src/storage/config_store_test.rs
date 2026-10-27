//! 配置协调器（§7.6）测试：tempdir + 内存类型。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{ConfigPayload, ConfigStore};
use crate::util::error::{E_INVALID_ARG, IpcError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DemoConfig {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    items: Vec<String>,
}

fn default_version() -> u32 {
    2
}

impl Default for DemoConfig {
    fn default() -> Self {
        Self {
            version: 2,
            items: Vec::new(),
        }
    }
}

impl ConfigPayload for DemoConfig {
    fn normalize(&mut self) -> Result<(), IpcError> {
        if self.version != 2 {
            return Err(IpcError::invalid_arg("version 必须为 2"));
        }
        self.items.retain(|i| !i.is_empty());
        Ok(())
    }
}

fn store_in(dir: &std::path::Path) -> ConfigStore<DemoConfig> {
    ConfigStore::new(dir.join("config.json"), dir.join("intent.json"))
}

#[test]
fn 全新安装load返回默认值且不落盘() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let config = store.load().unwrap();
    assert_eq!(config.version, 2);
    assert!(!dir.path().join("config.json").exists(), "不主动落盘");
}

#[test]
fn save归一化并原子落盘_load还原() {
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

#[test]
fn save校验失败config不变更() {
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

#[test]
fn commit外部落子失败_意向保留_config不变更() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());

    let err = store
        .commit(
            DemoConfig {
                version: 2,
                items: vec!["next".into()],
            },
            Some(json!({ "opId": "op-1", "providerId": "p1" })),
            |_cfg| Err(IpcError::keyring("模拟钥匙串失败")),
        )
        .unwrap_err();
    assert_eq!(err.code, "E_KEYRING");

    // 意向保留；config 不变更
    let pending = store.pending_intent().unwrap().expect("意向应保留");
    assert_eq!(pending["opId"], "op-1");
    assert_eq!(store.load().unwrap().items, Vec::<String>::new());
}

#[test]
fn commit成功后意向删除且缓存热替换() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let arc = store
        .commit(
            DemoConfig {
                version: 2,
                items: vec!["x".into()],
            },
            Some(json!({ "opId": "op-2" })),
            |cfg| {
                cfg.items.push("from-external".into());
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(
        arc.items,
        vec!["x".to_string(), "from-external".to_string()]
    );
    assert!(store.pending_intent().unwrap().is_none(), "成功后删意向");
    // 缓存热替换：load 命中提交后的同一 Arc
    assert!(Arc::ptr_eq(&store.load().unwrap(), &arc));
}

#[test]
fn 并发提交全部成功_终态为合法整文件() {
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
