//! 原子写单元测试（独立测试文件，源码零测试）。
//! 合并自 storage 侧（write_atomic 基线）与 providers 侧（json/失败路径）两套用例。

use std::fs;

use super::{write_atomic, write_json};

/// 断言两次写入均为覆盖生效，且完成后目录里只剩目标文件、无临时文件残留。
#[test]
fn atomic_write_overwrites_no_temp_left() {
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

/// 断言目标父目录整段缺失时，会自动逐级创建后写入成功。
#[test]
fn atomic_write_missing_target_creates_parent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("histories/s1/.segmeta.json");
    write_atomic(&path, b"{}").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"{}");
}

/// 断言对没有父目录的根路径写入时返回 InvalidInput 错误，而不是 panic。
#[test]
fn atomic_write_invalid_target_errors() {
    // 根路径没有父目录，应返回 InvalidInput 而非 panic
    let err = write_atomic(std::path::Path::new("/"), b"x").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

/// 断言 JSON 写入会先建深层目录，且产物能被原样解析回等值 JSON（人读可用）。
#[test]
fn atomic_write_json_creates_dirs_readable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a/b/config.json");

    write_json(&path, &serde_json::json!({ "schemaVersion": 2 })).unwrap();
    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["schemaVersion"], 2);
}

/// 断言父目录创建失败（路径被同名文件挡住）时整体报错，且失败路径不留临时文件。
#[test]
fn atomic_write_parent_blocked_fails_no_residue() {
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
