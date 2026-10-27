//! 编码解码器测试：BOM 三态、严格 UTF-8、GBK 探测、NUL 二进制判定、
//! fixture 端到端（tests/fixtures/sample_gbk.txt，GBK 编码）。

use super::{BINARY_SNIFF_BYTES, decode_text, is_probably_binary};

#[test]
fn utf8_bom_is_detected_and_stripped() {
    let bytes: &[u8] = b"\xEF\xBB\xBFhello \xE4\xB8\xAD\xE6\x96\x87";
    let d = decode_text(bytes);
    assert_eq!(d.encoding, "UTF-8");
    assert!(d.had_bom);
    assert_eq!(d.text, "hello 中文");
}

#[test]
fn utf16le_bom_is_detected() {
    // "中文abc" 的 UTF-16LE 字节（含 BOM）。
    let mut bytes = vec![0xFF, 0xFE];
    for unit in [0x4E2Du16, 0x6587, 0x0061, 0x0062, 0x0063] {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let d = decode_text(&bytes);
    assert_eq!(d.encoding, "UTF-16LE");
    assert!(d.had_bom);
    assert_eq!(d.text, "中文abc");
}

#[test]
fn utf16be_bom_is_detected() {
    let mut bytes = vec![0xFE, 0xFF];
    for unit in [0x4E2Du16, 0x6587] {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    let d = decode_text(&bytes);
    assert_eq!(d.encoding, "UTF-16BE");
    assert_eq!(d.text, "中文");
}

#[test]
fn plain_ascii_and_valid_utf8_take_fast_path() {
    let d = decode_text(b"hello world");
    assert_eq!(d.encoding, "UTF-8");
    assert!(!d.had_bom);

    let d = decode_text("无 BOM 的 UTF-8 中文".as_bytes());
    assert_eq!(d.encoding, "UTF-8");
    assert_eq!(d.text, "无 BOM 的 UTF-8 中文");
}

#[test]
fn gbk_bytes_are_detected_by_chardetng() {
    // 用 encoding_rs 的 GBK 编码器构造真实 GBK 字节（无 BOM）。
    let (encoded, _, _) = encoding_rs::GBK.encode("中文内容：配置文件示例 123");
    let d = decode_text(&encoded);
    assert_eq!(d.encoding, "GBK");
    assert_eq!(d.text, "中文内容：配置文件示例 123");
}

#[test]
fn gbk_fixture_file_decodes_end_to_end() {
    // 端到端 fixture：真实 GBK 编码文件（由 iconv 生成，入库为测试资产）。
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample_gbk.txt");
    let bytes = std::fs::read(path).expect("GBK fixture 应存在");
    let d = decode_text(&bytes);
    assert_eq!(d.encoding, "GBK");
    assert!(d.text.contains("GBK 编码示例文件"));
    assert!(d.text.contains("第二行"));
}

#[test]
fn undetectable_bytes_fall_back_to_best_guess() {
    // 单字节 latin-1 高位文本：chardetng 可能给 windows-1252 或探不出，
    // 两条路径的兜底标签都是 windows-1252，且绝不报错。
    let bytes = [0xE9, 0xE8, 0x20, 0x63, 0x61, 0x66, 0xE9]; // "èè café" in latin-1
    let d = decode_text(&bytes);
    assert!(!d.text.is_empty());
    assert_eq!(d.encoding, "windows-1252");
}

#[test]
fn nul_byte_means_binary() {
    assert!(is_probably_binary(b"ok\x00binary"));
    assert!(!is_probably_binary(b"plain text only"));
    // NUL 只看前 8KB 窗口。
    let mut late = vec![b'a'; BINARY_SNIFF_BYTES + 16];
    late[BINARY_SNIFF_BYTES + 10] = 0;
    assert!(!is_probably_binary(&late));
}

#[test]
fn empty_bytes_decode_to_empty_utf8() {
    let d = decode_text(&[]);
    assert_eq!(d.encoding, "UTF-8");
    assert_eq!(d.text, "");
    assert!(!is_probably_binary(&[]));
}
