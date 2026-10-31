//! 整文件原子写：临时文件同目录 + `fsync` + rename + 目录 fsync（尽力）。
//!
//! 原子写：先把内容完整写进同目录下的一个临时文件，落盘后再把临时文件
//! 改名（rename）成目标文件。改名这一步在操作系统层面是瞬间完成的，
//! 因此任何时刻去读目标文件，看到的要么是完整的旧内容、要么是完整的
//! 新内容，绝不会读到写了一半的中间状态。
//!
//! 文档指定机制：所有整文件（config/index/prefs/ui-state/segmeta）用此路径写。
//! 临时文件用 `tempfile::NamedTempFile`（同目录、文件名无碰撞），写完 `fsync`
//! 后 `persist`（即 rename 覆盖；Windows 侧语义为
//! `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`，无需自写 FFI）。
//! `fsync`：强制操作系统把缓冲区里的数据真正写到磁盘的系统调用——没有它，
//! "写入成功"可能只停在内存缓冲区，断电即丢失。
//! 目录 fsync 仅在 Unix 执行（Windows 打开目录句柄做 flush 不可行，rename
//! 原子性由 NTFS 元数据操作保证）。

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// 原子写 `contents` 到 `path`：同目录临时文件写入并 fsync 后 rename 覆盖；
/// Unix 侧追加尽力 fsync 父目录，保证 rename 条目本身持久化。
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    // 目标必须落在某个目录内；没有父目录（如根路径 "/"）直接判参数错误
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "原子写目标路径缺少父目录"))?;
    // 目标目录不存在则先逐级创建，允许直接写深层新路径
    fs::create_dir_all(dir)?;
    // 临时文件必须建在目标同一目录：rename 只在同一分区内才是原子操作，
    // 跨分区退化为"复制+删除"，中间状态会暴露
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(contents)?;
    // 先把内容强制刷进磁盘再改名——否则改名后断电可能留下空的目标文件
    tmp.as_file().sync_all()?;
    // persist = 关闭并删除临时句柄、rename 到目标路径（已存在则覆盖），不留残留
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
