//! 原子写单元测试（独立测试文件，源码零测试）。
//! 合并自 storage 侧（write_atomic 基线）与 providers 侧（json/失败路径）两套用例。

use std::fs;

use super::{write_atomic, write_json};

/// 断言成功路径（两次写入覆盖生效、目录里无临时残留、深父目录自动逐级创建、
/// write_json 产物可原样解析回等值 JSON）与失败路径（无父目录的根路径返回
/// InvalidInput 而非 panic；父目录创建失败整体报错，且失败路径不留临时文件）。
#[test]
fn atomic_write_success_and_failure_paths() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("segmeta.json");
    write_atomic(&path, b"v1").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"v1", "首次写入应生效");
    write_atomic(&path, b"v2-longer-content").unwrap();
    assert_eq!(
        fs::read(&path).unwrap(),
        b"v2-longer-content",
        "二次写入应覆盖"
    );
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers.len(), 1, "目录内只应剩目标文件，无临时残留");

    // 深父目录缺失时自动逐级创建。
    let deep = dir.path().join("histories/s1/.segmeta.json");
    write_atomic(&deep, b"{}").unwrap();
    assert_eq!(fs::read(&deep).unwrap(), b"{}", "深父目录应自动创建并写入");

    // write_json 建深层目录，产物可解析回等值 JSON。
    let json_path = dir.path().join("a/b/config.json");
    write_json(&json_path, &serde_json::json!({ "schemaVersion": 2 })).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).expect("JSON 产物应可解析");
    assert_eq!(v["schemaVersion"], 2, "JSON 产物应保持等值");

    // 失败路径：根路径没有父目录，应返回 InvalidInput 而非 panic。
    let err = write_atomic(std::path::Path::new("/"), b"x").unwrap_err();
    assert_eq!(
        err.kind(),
        std::io::ErrorKind::InvalidInput,
        "根路径应返回 InvalidInput"
    );

    // 指向一个"文件当目录"的路径，使 create_dir_all 失败。
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    fs::write(&blocker, b"x").unwrap();
    let path = blocker.join("sub").join("config.json");
    assert!(write_atomic(&path, b"x").is_err(), "父目录被挡应整体报错");
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
