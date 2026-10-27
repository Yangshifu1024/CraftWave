//! 原子写单元测试（独立测试文件，源码零测试）。

use std::fs;

use super::atomic::{write_bytes, write_json};

#[test]
fn 原子写覆盖既有内容且无临时残留() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    fs::write(&path, b"old").unwrap();

    write_bytes(&path, b"new-bytes").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new-bytes");

    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "不应残留临时文件：{leftovers:?}");
}

#[test]
fn 原子写自动创建缺失目录并写合法json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a/b/config.json");

    write_json(&path, &serde_json::json!({ "schemaVersion": 2 })).unwrap();
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["schemaVersion"], 2);
}

#[test]
fn 写入到不存在的深层路径失败时无残留() {
    let dir = tempfile::tempdir().unwrap();
    // 指向一个"文件当目录"的路径，使 create_dir_all 失败
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, b"x").unwrap();
    let path = blocker.join("sub").join("config.json");

    assert!(write_bytes(&path, b"x").is_err());
}
