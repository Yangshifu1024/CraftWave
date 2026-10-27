//! 原子写单元测试（独立测试文件，源码零测试）。
//! 合并自 storage 侧（write_atomic 基线）与 providers 侧（json/失败路径）两套用例。

use std::fs;

use super::{write_atomic, write_json};

#[test]
fn 原子写_覆盖既有内容且不留临时文件() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("segmeta.json");
    write_atomic(&path, b"v1").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"v1");

    write_atomic(&path, b"v2-longer-content").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"v2-longer-content");

    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers.len(), 1, "目录内只应剩目标文件，无临时残留");
}

#[test]
fn 原子写_目标不存在时自动建父目录() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("histories/s1/.segmeta.json");
    write_atomic(&path, b"{}").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"{}");
}

#[test]
fn 原子写_无效目标路径返回错误() {
    // 根路径没有父目录，应返回 InvalidInput 而非 panic
    let err = write_atomic(std::path::Path::new("/"), b"x").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn 原子写_json_自动创建缺失目录且内容可回读() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a/b/config.json");

    write_json(&path, &serde_json::json!({ "schemaVersion": 2 })).unwrap();
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["schemaVersion"], 2);
}

#[test]
fn 原子写_父目录被文件占位时失败且无残留() {
    let dir = tempfile::tempdir().unwrap();
    // 指向一个"文件当目录"的路径，使 create_dir_all 失败
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, b"x").unwrap();
    let path = blocker.join("sub").join("config.json");

    assert!(write_atomic(&path, b"x").is_err());
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "失败路径不应残留临时文件：{leftovers:?}"
    );
}
