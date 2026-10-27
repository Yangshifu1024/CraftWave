//! project_store 测试：注册/更新/重绑/删除与一目录一项目约束。

use std::fs;
use std::path::{Path, PathBuf};

use super::*;

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

fn temp_home(tag: &str) -> String {
    let dir = temp_root(tag);
    dir.to_string_lossy().into_owned()
}

#[test]
fn save_new_persists_registry_and_manifest() {
    let root = temp_root("new");
    let store = ProjectStore::new(&root);
    let home = temp_home("new-home");
    let meta = store.save(None, "演示项目", &home).unwrap();
    assert!(!meta.id.is_empty());

    let listed = store.list().unwrap();
    assert_eq!(listed, vec![meta.clone()]);

    // 主目录注册清单：rootsSnapshot=[homeDir]
    let manifest: ProjectManifest =
        serde_json::from_str(&fs::read_to_string(manifest_of(&home)).unwrap()).unwrap();
    assert_eq!(manifest.id, meta.id);
    assert_eq!(manifest.roots_snapshot, vec![home]);
}

#[test]
fn save_rejects_duplicate_home_dir() {
    let root = temp_root("dup");
    let store = ProjectStore::new(&root);
    let home = temp_home("dup-home");
    store.save(None, "甲", &home).unwrap();
    match store.save(None, "乙", &home) {
        Err(ProjectError::DuplicateHome(h)) => assert_eq!(h, home),
        other => panic!("应拒绝重复 homeDir，实际 {other:?}"),
    }
}

#[test]
fn home_change_rebinds_manifest() {
    let root = temp_root("rebind");
    let store = ProjectStore::new(&root);
    let old_home = temp_home("rebind-old");
    let new_home = temp_home("rebind-new");
    let meta = store.save(None, "迁移项目", &old_home).unwrap();

    let updated = store.save(Some(&meta.id), "迁移项目", &new_home).unwrap();
    assert_eq!(updated.id, meta.id);
    assert!(!manifest_of(&old_home).exists());
    assert!(manifest_of(&new_home).exists());
}

#[test]
fn update_without_home_change_keeps_manifest() {
    let root = temp_root("rename");
    let store = ProjectStore::new(&root);
    let home = temp_home("rename-home");
    let meta = store.save(None, "旧名", &home).unwrap();
    let updated = store.save(Some(&meta.id), "新名", &home).unwrap();
    assert_eq!(updated.name, "新名");
    let manifest: ProjectManifest =
        serde_json::from_str(&fs::read_to_string(manifest_of(&home)).unwrap()).unwrap();
    assert_eq!(manifest.name, "新名");
}

#[test]
fn delete_removes_registry_and_manifest_but_not_data() {
    let root = temp_root("delete");
    let store = ProjectStore::new(&root);
    let home = temp_home("delete-home");
    let marker = Path::new(&home).join("keep.txt");
    fs::write(&marker, b"keep").unwrap();
    let meta = store.save(None, "待删项目", &home).unwrap();

    let removed = store.delete(&meta.id).unwrap();
    assert_eq!(
        removed.as_ref().map(|m| m.id.as_str()),
        Some(meta.id.as_str())
    );
    assert!(store.list().unwrap().is_empty());
    assert!(!manifest_of(&home).exists());
    assert!(marker.exists(), "绝不触碰项目数据目录");
    assert_eq!(store.delete(&meta.id).unwrap(), None);
}

#[test]
fn home_of_resolves_project_root() {
    let root = temp_root("homeof");
    let store = ProjectStore::new(&root);
    let home = temp_home("homeof-home");
    let meta = store.save(None, "根解析", &home).unwrap();
    assert_eq!(store.home_of(&meta.id), Some(PathBuf::from(home.as_str())));
    assert_eq!(store.home_of("不存在的项目"), None);
}

#[test]
fn update_missing_project_is_not_found() {
    let root = temp_root("missing");
    let store = ProjectStore::new(&root);
    let home = temp_home("missing-home");
    match store.save(Some("不存在的项目"), "名", &home) {
        Err(ProjectError::NotFound(id)) => assert_eq!(id, "不存在的项目"),
        other => panic!("应返回 NotFound，实际 {other:?}"),
    }
}
