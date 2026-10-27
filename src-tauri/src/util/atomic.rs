//! 原子写（§7.4）：整文件（config/index/prefs/ui-state/segmeta）统一写法——
//! 临时文件同目录 + fsync + rename。
//!
//! Windows 上 `std::fs::rename` 即 `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`，
//! 与设计文档 §7.4 的口径一致；目录 fsync 仅在支持的平台执行（Windows 打开
//! 目录句柄做 flush 不可行，rename 原子性由 NTFS 元数据操作保证）。

use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// 原子写字节流：写 `<dir>/.<name>.tmp-<纳秒>` → fsync 文件 → rename → fsync 目录。
pub fn write_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "目标路径无父目录"))?;
    fs::create_dir_all(dir)?;
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(
        ".{file_name}.tmp-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));

    let result = (|| -> io::Result<()> {
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        // rename 覆盖既有目标：Unix 原子；Windows 经 MoveFileExW(REPLACE_EXISTING)。
        fs::rename(&tmp, path)?;
        sync_dir(dir);
        Ok(())
    })();

    // 失败清理残留临时文件；成功路径 tmp 已不存在，忽略错误。
    let _ = fs::remove_file(&tmp);
    result
}

/// 原子写 UTF-8 JSON（紧凑或带缩进由调用方决定）。
pub fn write_json(path: &Path, value: &serde_json::Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    write_bytes(path, &bytes)
}

#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}
