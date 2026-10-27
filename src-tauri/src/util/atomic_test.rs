//! 原子写单元测试（独立测试文件，源码零测试）。

use std::fs;

use super::write_atomic;

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
