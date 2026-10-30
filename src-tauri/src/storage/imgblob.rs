//! 会话图片 blob 存储（§7.7 v1 口径，17 号票）：图片字节数据的落盘与读取。
//!
//! 术语解释：「blob」指一整块二进制数据（binary large object），这里就是
//! 用户附图或读取的图片文件的原始字节；「内容寻址」指文件名取自内容本身
//! 的哈希值——相同内容的图片必然落在同一个文件名下，因此同一会话内重复
//! 附同一张图天然去重（第二次写入直接跳过）。
//!
//! 目录与命名（§7.7 行逐字）：数据根下的 `sessions/<sid>.imgblob/<hash>.bin`，
//! `hash` 为 SHA-256 前 32 hex（16 字节十六进制展开，小写）；MIME 类型不单独
//! 落盘——读取时对字节做魔数嗅探（magic bytes，文件头部特征字节）恢复，
//! 白名单外格式在写入入口即被拒绝，目录里不可能出现嗅探不出的图片。
//!
//! GC（垃圾回收）v1 口径（2026-10-30 本票取舍，已回写 §7.7 注记）：会话删除
//! 时随 `.imgblob` 目录整体删除（`SessionStore::delete_session` 一并执行）；
//! 「按引用计数的精确 GC（删掉不再被任何消息引用的 blob）」与保留期清理的
//! 联动归 M3 保留期票。v1 下同一会话内重复附图去重、跨会话不共享（各会话
//! 目录独立），因此整目录删除不会误删他会话的数据。
//!
//! 日志纪律（spec「图片与日志」条）：本模块与调用方都**不把图片字节写进
//! 任何日志**——只允许记录哈希、格式、字节数这类元数据。

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::StorageError;

/// 单张图片的字节上限（FR-12-02：≤5MiB，与前端附件校验同口径）。
pub const MAX_BLOB_BYTES: usize = 5 * 1024 * 1024;

/// blob 哈希的十六进制长度（SHA-256 前 32 hex = 16 字节展开）。
pub const BLOB_HASH_HEX_LEN: usize = 32;

/// 按文件头魔数嗅探图片 MIME 类型（png/jpeg/gif/webp 四种，§6.2 白名单；
/// 其余返回 None——调用方按参数错误拒绝）。
pub fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

/// blob 哈希合法性（恰好 32 个小写十六进制字符）：直接用作文件名，校验
/// 防路径穿越与异常输入。
pub fn is_valid_blob_hash(hash: &str) -> bool {
    hash.len() == BLOB_HASH_HEX_LEN
        && hash
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// 会话 id 合法性（与 `session_tree::validate_session_id` 同规则：uuid 风格
/// 字符，仅允许字母数字与 `-_`，≤128 长度——会话 id 直接进入目录名）。
fn validate_session_id(session_id: &str) -> Result<(), StorageError> {
    let ok_len = !session_id.is_empty() && session_id.len() <= 128;
    let ok_chars = session_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok_len && ok_chars {
        Ok(())
    } else {
        Err(StorageError::InvalidArg(format!(
            "非法 sessionId：{session_id:?}"
        )))
    }
}

/// 会话的 blob 目录（数据根下 `sessions/<sid>.imgblob/`，§7.7）。
pub fn imgblob_dir(root: &Path, session_id: &str) -> PathBuf {
    root.join("sessions").join(format!("{session_id}.imgblob"))
}

/// 计算图片字节的 blob 哈希（SHA-256 前 32 hex，内容寻址）。
pub fn blob_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    hex
}

/// 写入一份图片 blob（内容寻址 + 写入去重）：校验字节数（非空、≤5MiB）与
/// 格式白名单（魔数嗅探），哈希命中已存在文件时跳过写入（幂等）。返回
/// blob 哈希。
pub fn put_blob(root: &Path, session_id: &str, bytes: &[u8]) -> Result<String, StorageError> {
    validate_session_id(session_id)?;
    if bytes.is_empty() {
        return Err(StorageError::InvalidArg("图片内容为空".to_owned()));
    }
    if bytes.len() > MAX_BLOB_BYTES {
        return Err(StorageError::InvalidArg(format!(
            "图片超过 5MiB 上限（{} 字节）",
            bytes.len()
        )));
    }
    sniff_image_mime(bytes).ok_or_else(|| {
        StorageError::InvalidArg(
            "不支持的图片格式（仅 png/jpeg/gif/webp，按文件头识别）".to_owned(),
        )
    })?;
    let hash = blob_hash(bytes);
    let dir = imgblob_dir(root, session_id);
    let path = dir.join(format!("{hash}.bin"));
    if path.exists() {
        return Ok(hash); // 内容寻址天然幂等：同哈希已落盘即去重
    }
    crate::util::atomic::write_atomic(&path, bytes)
        .map_err(|e| StorageError::Io(format!("写入图片 blob 失败：{e}")))?;
    Ok(hash)
}

/// 读取一份图片 blob：返回（字节, 嗅探出的 MIME 类型）。哈希非法按参数
/// 错误拒绝（防路径穿越）；文件缺失按 NotFound（前端灰显占位）。
pub fn get_blob(
    root: &Path,
    session_id: &str,
    hash: &str,
) -> Result<(Vec<u8>, &'static str), StorageError> {
    validate_session_id(session_id)?;
    if !is_valid_blob_hash(hash) {
        return Err(StorageError::InvalidArg(format!("非法 blobHash：{hash:?}")));
    }
    let path = imgblob_dir(root, session_id).join(format!("{hash}.bin"));
    let bytes = std::fs::read(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => StorageError::NotFound(format!(
            "图片 blob 不存在（会话 {session_id}，hash {hash}）"
        )),
        _ => StorageError::Io(format!("读取图片 blob 失败：{e}")),
    })?;
    let mime = sniff_image_mime(&bytes).ok_or_else(|| {
        // 写入入口已按白名单校验，正常不会走到这里；目录被手改时诚实报错
        StorageError::Io(format!("图片 blob 内容损坏（无法识别格式）：{hash}"))
    })?;
    Ok((bytes, mime))
}

#[cfg(test)]
#[path = "imgblob_test.rs"]
mod imgblob_test;
