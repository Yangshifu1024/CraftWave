//! 整文件原子写（§7.4）：临时文件同目录 + `fsync` + rename + 目录 fsync（尽力）。
//!
//! 文档指定机制：所有整文件（config/index/prefs/ui-state/segmeta）用此路径写。
//! 临时文件用 `tempfile::NamedTempFile`（同目录、文件名无碰撞），写完 `fsync`
//! 后 `persist`（即 rename 覆盖；Windows 侧语义为
//! `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`，无需自写 FFI）。
//! 目录 fsync 仅在 Unix 执行（Windows 打开目录句柄做 flush 不可行，rename
//! 原子性由 NTFS 元数据操作保证）。

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// 原子写 `contents` 到 `path`：同目录临时文件写入并 fsync 后 rename 覆盖；
/// Unix 侧追加尽力 fsync 父目录，保证 rename 条目本身持久化。
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "原子写目标路径缺少父目录"))?;
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(contents)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|err| err.error)?;
    #[cfg(unix)]
    if let Ok(dir_file) = fs::File::open(dir) {
        // 目录 fsync 失败不影响写入成功（尽力而为；Windows 无此语义）
        let _ = dir_file.sync_all();
    }
    Ok(())
}

/// 原子写 UTF-8 JSON（pretty 排版 + 尾部换行，供 config.json / intent.json 等人读文件）。
pub fn write_json(path: &Path, value: &serde_json::Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

#[cfg(test)]
#[path = "atomic_test.rs"]
mod atomic_test;
