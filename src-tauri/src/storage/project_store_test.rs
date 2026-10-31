//! project_store 测试（独立测试文件）：注册/更新/重绑/删除与一目录一项目约束。
//!
//! 测试面覆盖：新建项目同时落注册表与主目录注册清单、重复 homeDir 拒绝、
//! homeDir 变更时清单重绑（旧目录移除新目录出现）、仅改名不动清单、删除只清
//! 注册痕迹而保留项目数据文件、home_of 根解析、更新不存在的项目报 NotFound。
//! 同型用例在单个 #[test] 内顺序合并，断言消息带用例标识。

use std::fs;
use std::path::{Path, PathBuf};

use super::*;

// 建一个互不冲突的临时根目录（目录名带随机 uuid，支持并行跑测试）。
fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "craftwave-project-store-{tag}-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/** 主目录注册清单路径（与 project_store::manifest_path 布局一致）。 */
fn manifest_of(home: &str) -> PathBuf {
    Path::new(home).join(".craftwave").join("project.json")
}

// 建一个临时主目录并返回其路径字符串。
fn temp_home(tag: &str) -> String {
    let dir = temp_root(tag);
    dir.to_string_lossy().into_owned()
}

// 断言：注册与更新全语义——新建同时落注册表与主目录清单（rootsSnapshot=
// [homeDir]）；同一主目录注册第二个项目被拒（一目录一项目 DuplicateHome）；
// homeDir 变更触发清单重绑（旧目录移除新目录出现、项目 id 不变）；仅改名
// 清单原地更新不搬移；更新不存在的项目报 NotFound（带原 id）。
#[test]
fn save_update_and_rebind_semantics() {
    let root = temp_root("life");
    let store = ProjectStore::new(&root);

    // 新建：注册表列出该条目，主目录写入清单
    let home = temp_home("life-home");
    let meta = store.save(None, "演示项目", &home).unwrap();
    assert!(!meta.id.is_empty());
    let listed = store.list().unwrap();
    assert_eq!(listed, vec![meta.clone()]);
    let manifest: ProjectManifest =
        serde_json::from_str(&fs::read_to_string(manifest_of(&home)).unwrap()).unwrap();
    assert_eq!(manifest.id, meta.id);
    assert_eq!(manifest.roots_snapshot, vec![home.clone()]);

    // 重复 homeDir：一目录一项目
    match store.save(None, "乙", &home) {
        Err(ProjectError::DuplicateHome(h)) => assert_eq!(h, home),
        other => panic!("应拒绝重复 homeDir，实际 {other:?}"),
    }

    // homeDir 变更：旧目录清单消失、新目录清单出现、项目 id 不变
    let old_home = home;
    let new_home = temp_home("life-new");
    let updated = store.save(Some(&meta.id), "演示项目", &new_home).unwrap();
    assert_eq!(updated.id, meta.id);
    assert!(!manifest_of(&old_home).exists(), "旧目录清单移除");
    assert!(manifest_of(&new_home).exists(), "新目录清单出现");

    // 仅改名（homeDir 未变）：清单原地更新为新名，不触发搬移
    let updated = store.save(Some(&meta.id), "新名", &new_home).unwrap();
    assert_eq!(updated.name, "新名");
    let manifest: ProjectManifest =
        serde_json::from_str(&fs::read_to_string(manifest_of(&new_home)).unwrap()).unwrap();
    assert_eq!(manifest.name, "新名");

    // 更新不存在的项目：NotFound 带原 id
    match store.save(Some("不存在的项目"), "名", &temp_home("missing")) {
        Err(ProjectError::NotFound(id)) => assert_eq!(id, "不存在的项目"),
        other => panic!("应返回 NotFound，实际 {other:?}"),
    }
}

// 断言：删除与查询语义——删除项目清除注册表条目与主目录清单但项目数据文件
// 原样保留、重复删除返回 None；home_of 按 id 解析项目主目录、未知 id 返回 None。
#[test]
fn delete_and_home_of_semantics() {
    let root = temp_root("del");
    let store = ProjectStore::new(&root);

    // 删除：清注册痕迹、保数据文件、重复删除 None
    let home = temp_home("del-home");
    let marker = Path::new(&home).join("keep.txt");
    fs::write(&marker, b"keep").unwrap();
    let meta = store.save(None, "待删项目", &home).unwrap();
    let removed = store.delete(&meta.id).unwrap();
    assert_eq!(
        removed.as_ref().map(|m| m.id.as_str()),
        Some(meta.id.as_str())
    );
    assert!(store.list().unwrap().is_empty(), "注册表条目清除");
    assert!(!manifest_of(&home).exists(), "主目录清单清除");
    assert!(marker.exists(), "绝不触碰项目数据目录");
    assert_eq!(store.delete(&meta.id).unwrap(), None, "重复删除 None");

    // home_of：按 id 解析，未知 id None
    let home2 = temp_home("homeof-home");
    let meta = store.save(None, "根解析", &home2).unwrap();
    assert_eq!(store.home_of(&meta.id), Some(PathBuf::from(home2.as_str())));
    assert_eq!(store.home_of("不存在的项目"), None);
}
