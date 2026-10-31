//! 编码解码器测试：BOM 三态与快路径（表驱动）、GBK 探测与 fixture 端到端、
//! NUL 二进制判定（tests/fixtures/sample_gbk.txt 为 GBK 编码测试资产）。

use super::{BINARY_SNIFF_BYTES, decode_text, is_probably_binary};

/// 断言各 BOM 变体与无 BOM 输入的识别：编码标签、BOM 剥离与正文完整。
#[test]
fn bom_variants_and_edge_inputs_table() {
    // "中文abc" 的 UTF-16LE / "中文" 的 UTF-16BE 字节（含 BOM）。
    let mut utf16le = vec![0xFF, 0xFE];
    for unit in [0x4E2Du16, 0x6587, 0x0061, 0x0062, 0x0063] {
        utf16le.extend_from_slice(&unit.to_le_bytes());
    }
    let mut utf16be = vec![0xFE, 0xFF];
    for unit in [0x4E2Du16, 0x6587] {
        utf16be.extend_from_slice(&unit.to_be_bytes());
    }
    for (name, bytes, encoding, text, had_bom) in [
        (
            "UTF-8 BOM 剥离",
            b"\xEF\xBB\xBFhello \xE4\xB8\xAD\xE6\x96\x87" as &[u8],
            "UTF-8",
            "hello 中文",
            true,
        ),
        ("UTF-16LE BOM", &utf16le[..], "UTF-16LE", "中文abc", true),
        ("UTF-16BE BOM", &utf16be[..], "UTF-16BE", "中文", true),
        (
            "纯 ASCII 快路径",
            b"hello world" as &[u8],
            "UTF-8",
            "hello world",
            false,
        ),
        (
            "无 BOM UTF-8 中文",
            "无 BOM 的 UTF-8 中文".as_bytes(),
            "UTF-8",
            "无 BOM 的 UTF-8 中文",
            false,
        ),
        ("空输入", b"" as &[u8], "UTF-8", "", false),
    ] {
        let d = decode_text(bytes);
        assert_eq!(d.encoding, encoding, "编码识别[{name}]");
        assert_eq!(d.had_bom, had_bom, "BOM 标记[{name}]");
        assert_eq!(d.text, text, "解码正文[{name}]");
    }
}

/// 断言非 UTF-8 字节的三条探测路径：chardetng 识别 GBK、真实 GBK fixture
/// 端到端还原、无法可靠探测的 latin-1 兜底到 windows-1252 且绝不报错；
/// 以及 NUL 字节判二进制只看前 8KB 窗口。
#[test]
fn non_utf8_and_binary_detection_paths() {
    // 用 encoding_rs 的 GBK 编码器构造真实 GBK 字节（无 BOM）。
    let source = "中文内容：配置文件示例 123";
    let (encoded, _, _) = encoding_rs::GBK.encode(source);
    let d = decode_text(&encoded);
    assert_eq!(d.encoding, "GBK", "chardetng 应识别 GBK");
    assert_eq!(d.text, source, "GBK 正文应完整还原");

    // 端到端 fixture：真实 GBK 编码文件（由 iconv 生成，入库为测试资产）。
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample_gbk.txt");
    let bytes = std::fs::read(path).expect("GBK fixture 应存在");
    let d = decode_text(&bytes);
    assert_eq!(d.encoding, "GBK", "fixture 应识别为 GBK");
    assert!(
        d.text.contains("GBK 编码示例文件"),
        "fixture 首行应还原：{}",
        d.text
    );
    assert!(
        d.text.contains("第二行"),
        "fixture 第二行应还原：{}",
        d.text
    );

    // 单字节 latin-1 高位文本：chardetng 可能给 windows-1252 或探不出，
    // 两条路径的兜底标签都是 windows-1252，且绝不报错。
    let bytes = [0xE9, 0xE8, 0x20, 0x63, 0x61, 0x66, 0xE9]; // "èè café" in latin-1
    let d = decode_text(&bytes);
    assert!(!d.text.is_empty(), "兜底路径解码不应为空");
    assert_eq!(d.encoding, "windows-1252", "无法探测时应兜底 windows-1252");

    // NUL 字节判二进制只看前 8KB 窗口：窗口内命中、窗口外与纯文本不算。
    assert!(
        is_probably_binary(b"ok\x00binary"),
        "窗口内含 NUL 应判二进制"
    );
    assert!(
        !is_probably_binary(b"plain text only"),
        "纯文本不应判二进制"
    );
    let mut late = vec![b'a'; BINARY_SNIFF_BYTES + 16];
    late[BINARY_SNIFF_BYTES + 10] = 0;
    assert!(!is_probably_binary(&late), "NUL 在 8KB 窗口外不应判二进制");
}
