//! 整文件原子写（§7.4）：临时文件同目录 + `fsync` + rename。
//!
//! 文档指定机制：所有整文件（config/index/prefs/ui-state/segmeta）用此路径写；
//! Windows 侧 rename 语义为 `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`
//! （`tempfile::NamedTempFile::persist` 的既有行为，无需自写 FFI）。

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

#[cfg(test)]
#[path = "atomic_test.rs"]
mod atomic_test;
