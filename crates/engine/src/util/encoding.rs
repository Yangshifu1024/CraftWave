//! 共用文本解码器（2026-10-27 定案）。
//!
//! BOM（Byte Order Mark，字节顺序标记）：文件开头的一小段特殊字节，用来
//! 标明文本的编码与字节序（如 UTF-16LE 以 FF FE 开头）。chardetng：Mozilla
//! 出品的编码探测库，按字节分布统计"猜"出最可能的编码。
//!
//! 判定顺序：BOM（UTF-8/UTF-16LE/BE）→ 严格 UTF-8 校验 → chardetng 探测
//! → 均不中按最佳猜测渲染（预览宁乱码不报错，无效序列替换 U+FFFD）。
//!
//! agent 的 `read` 工具与 `read_workspace_file` 命令
//! 必须共用本模块，不得各写一套。

use encoding_rs::Encoding;

/// 二进制嗅探窗口：前 8KB 含 NUL 判二进制。
pub const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// 读工具文本通道单次响应上限（单次响应 ≤512KB）。
pub const MAX_RESPONSE_BYTES: usize = 512 * 1024;

/// 解码结果：文本 + 实际使用的编码标签 + 是否命中 BOM。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedText {
    /// 解码出的正文（无效字节已按替换策略转为 U+FFFD，永不报错）。
    pub text: String,
    /// encoding_rs 的编码名（"UTF-8"、"UTF-16LE"、"GBK"、"windows-1252"…）。
    pub encoding: &'static str,
    /// 输入是否以 BOM 开头（BOM 本身已剥离，不会留在 text 里）。
    pub had_bom: bool,
}

/// 前 8KB 含 NUL 字节判二进制（图片/Office 各有专门通道，文本预览报错）。
pub fn is_probably_binary(bytes: &[u8]) -> bool {
    let window = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    window.contains(&0u8)
}

/// 按 四步判定解码。任何路径都不报错：最终兜底 windows-1252
/// （单字节编码，任何字节序列均合法，配合替换语义即「最佳猜测渲染」）。
pub fn decode_text(bytes: &[u8]) -> DecodedText {
    // ① BOM 直接定（encoding_rs decode 自会剥离 BOM；decode 的第三返回值
    //    是 had_errors 而非 had_bom，此处以 for_bom 命中为准）。
    if Encoding::for_bom(bytes).is_some() {
        let (enc, _) = Encoding::for_bom(bytes).expect("刚判定命中");
        let (text, _, _) = enc.decode(bytes);
        return DecodedText {
            text: text.into_owned(),
            encoding: enc.name(),
            had_bom: true,
        };
    }

    // ② 无 BOM 先严格 UTF-8 校验（最快路径，绝大多数现代文本命中）。
    if let Ok(text) = std::str::from_utf8(bytes) {
        return DecodedText {
            text: text.to_string(),
            encoding: encoding_rs::UTF_8.name(),
            had_bom: false,
        };
    }

    // ③ chardetng 探测（GBK/Big5/Shift_JIS 等）；UTF-8 已在②严格校验失败，
    //    不允许探测结果回退成 UTF-8（Utf8Detection::Deny）；ISO-2022-JP
    //    误判率高，本地文件场景一并拒绝（Iso2022JpDetection::Deny）。
    // ④ chardetng 恒给出候选，配合替换语义渲染即「最佳猜测」。
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    let enc = detector.guess(None, chardetng::Utf8Detection::Deny);
    let (text, _, _) = enc.decode(bytes);
    DecodedText {
        text: text.into_owned(),
        encoding: enc.name(),
        had_bom: false,
    }
}

#[cfg(test)]
#[path = "encoding_test.rs"]
mod encoding_test;
