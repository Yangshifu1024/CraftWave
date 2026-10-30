//! 写后完整性自检（22 号票，FR-05-05 配套 / §8.4 步骤 6 EOL 口径 / C-10）。
//!
//! write/edit 落盘成功后**重读文件**做纯内存比对：
//! 1. 「哈希」：重读字节与写入内容逐字节一致（sha256 比对）——磁盘坏块、
//!    并发改写、杀毒软件篡改等静默写坏当场暴露；
//! 2. 「UTF-8」：重读内容严格 UTF-8 解码成功（C-10「落盘一律 UTF-8」；
//!    与哈希校验互为纵深——哈希一致时本项恒过，但独立校验保证本模块
//!    对任何写入通道单独成立）；
//! 3. 「EOL」（edit 场景）：原 CRLF 文件写回后仍为 CRLF（§8.4 步骤 6
//!    「EOL 保持」的落盘后实证）；LF 文件未被引入 CRLF。
//!
//! 任一阶段失败 → 工具结果 ok=false，错误码 E_INTERNAL（§3.6 通用兜底），
//! message 含路径与校验阶段（模型可据此重试或改写）。
//!
//! 诚实边界：本模块校验的是「落盘后的文件与预期一致」，无法防落盘**前**
//! 的逻辑错误（内容本身就不对）；真实磁盘级故障注入（坏块/外力篡改时序）
//! 在工具全链不可稳定复现，故障面以 [`verify_written`] 的单元注入测试
//! 覆盖（构造「重读内容与期望不一致」等价注入）。

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::util::errors::ToolError;

/// EOL 期望（edit 场景保持原文件风格，§8.4 步骤 6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EolExpect {
    /// 无 EOL 要求（write 新建：内容原样 UTF-8 落盘）。
    Any,
    /// 保持指定风格：`true` = 原文件为 CRLF（写回不得出现裸 LF）；
    /// `false` = 原文件为 LF（写回不得引入 CRLF）。
    Preserve(bool),
}

/// 校验失败：阶段名（哈希/UTF-8/EOL/重读）+ 人话描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostcheckFailure {
    pub stage: &'static str,
    pub message: String,
}

/// 写后自检：重读 `path` 并逐项校验（哈希 → UTF-8 → EOL）。
pub fn verify_written(
    path: &Path,
    expected: &[u8],
    eol: EolExpect,
) -> Result<(), PostcheckFailure> {
    // 阶段一「重读」：落盘后重读一次（纯内存比对，无配置面）。
    let actual = std::fs::read(path).map_err(|e| PostcheckFailure {
        stage: "重读",
        message: format!("写入后重读失败（{}）：{e}", path.display()),
    })?;

    // 阶段二「哈希」：与写入内容逐字节一致（sha256 比对）。
    if sha256_hex(&actual) != sha256_hex(expected) {
        return Err(PostcheckFailure {
            stage: "哈希",
            message: format!(
                "写入后内容与预期不一致（磁盘写入静默损坏？）：{}（预期 {} 字节，实得 {} 字节）",
                path.display(),
                expected.len(),
                actual.len()
            ),
        });
    }

    // 阶段三「UTF-8」：严格解码（C-10 落盘一律 UTF-8）。
    if let Err(e) = std::str::from_utf8(&actual) {
        return Err(PostcheckFailure {
            stage: "UTF-8",
            message: format!("落盘内容不是严格 UTF-8（{}）：{e}", path.display()),
        });
    }

    // 阶段四「EOL」：风格保持实证（仅 edit 场景要求）。
    match eol {
        EolExpect::Any => {}
        EolExpect::Preserve(true) => {
            if contains_bare_lf(&actual) {
                return Err(PostcheckFailure {
                    stage: "EOL",
                    message: format!(
                        "CRLF 文件写回后混入裸 LF（EOL 保持被破坏）：{}",
                        path.display()
                    ),
                });
            }
        }
        EolExpect::Preserve(false) => {
            if actual.windows(2).any(|w| w == b"\r\n") {
                return Err(PostcheckFailure {
                    stage: "EOL",
                    message: format!(
                        "LF 文件写回后混入 CRLF（EOL 保持被破坏）：{}",
                        path.display()
                    ),
                });
            }
        }
    }
    Ok(())
}

/// 自检失败 → 工具错误（E_INTERNAL，message 含路径与校验阶段，§3.6 通用
/// 兜底；专用错误码待评审定，若增设先回写 §3.6）。
pub fn postcheck_error(display_path: &str, failure: &PostcheckFailure) -> ToolError {
    ToolError::new(
        crate::util::errors::E_INTERNAL,
        format!(
            "写后完整性自检失败（阶段「{}」）：{}——{}",
            failure.stage, display_path, failure.message
        ),
    )
}

/// sha256 十六进制（全量 64 hex；比对用途，无需截断）。
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// 是否含裸 LF（不在 CRLF 对里的 \n）——CRLF 保持校验的判据。
fn contains_bare_lf(bytes: &[u8]) -> bool {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' && (i == 0 || bytes[i - 1] != b'\r') {
            return true;
        }
        i += 1;
    }
    false
}

#[cfg(test)]
#[path = "postcheck_test.rs"]
mod postcheck_test;
