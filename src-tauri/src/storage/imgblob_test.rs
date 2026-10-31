//! `storage/imgblob` 的单元测试：内容寻址与去重、哈希稳定、
//! 白名单外格式与超限拒绝、越权读取（他会话 blob 不可见）、非法哈希拒绝、
//! 会话删除 GC。全链路合并为单个 #[test]，断言消息带阶段标识。

use base64::Engine;

use super::*;

/// 1x1 透明 PNG（base64 内嵌，程序解码——不依赖外部样本文件）。
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

/// 1x1 JPEG（base64 内嵌最小样本）。
const JPEG_1X1: &str = "/9j/4AAQSkZJRgABAQEAYABgAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAACf/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AVN//2Q==";

/// 1x1 GIF（base64 内嵌最小样本）。
const GIF_1X1: &str = "R0lGODlhAQABAIAAAP///wAAACH5BAEAAAAALAAAAAABAAEAAAICRAEAOw==";

fn decode(b64: &str) -> Vec<u8> {
    use base64::engine::general_purpose::STANDARD;
    STANDARD.decode(b64).unwrap()
}

fn temp_root(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("cw-imgblob-{tag}-"))
        .tempdir()
        .unwrap()
}

// 断言：图片 blob 通道全链路——魔数嗅探白名单（png/jpeg/gif/webp）放行、
// 文本/SVG/空内容拒绝；put_blob 内容寻址（SHA-256 前 32 hex 小写定长、同
// 内容去重、文件名 <hash>.bin）；get_blob roundtrip 按魔数恢复 MIME；超限/
// 空/白名单外写入拒绝；他会话 blob 不可见（root 归属）、非法哈希（含路径
// 穿越向量）按参数错误拒绝、合法但不存在的哈希按 NotFound；SessionStore
// 删会话连带删除 blob 目录（v1 GC）。
#[test]
fn imgblob_dedup_roundtrip_and_guards() {
    // 嗅探：白名单放行（WebP 为 RIFF 容器 + 偏移 8..12 的 "WEBP" 标记，
    // 嗅探只看头不解析块）
    assert_eq!(sniff_image_mime(&decode(PNG_1X1)), Some("image/png"));
    assert_eq!(sniff_image_mime(&decode(JPEG_1X1)), Some("image/jpeg"));
    assert_eq!(sniff_image_mime(&decode(GIF_1X1)), Some("image/gif"));
    let mut webp = b"RIFF\x24\x00\x00\x00WEBPVP8 ".to_vec();
    webp.extend_from_slice(&[0u8; 20]);
    assert_eq!(sniff_image_mime(&webp), Some("image/webp"));
    assert_eq!(sniff_image_mime(b"plain text"), None, "文本拒绝");
    assert_eq!(sniff_image_mime(&[]), None, "空内容拒绝");
    assert_eq!(
        sniff_image_mime(b"<svg xmlns=...>"),
        None,
        "SVG 文本伪装不在白名单（文本型图片不进 blob 通道）"
    );

    // 内容寻址与去重：哈希口径钉住（SHA-256 前 16 字节）
    let root = temp_root("dedup");
    let bytes = decode(PNG_1X1);
    let hash1 = put_blob(root.path(), "sess-a", &bytes).unwrap();
    assert_eq!(hash1.len(), 32);
    assert!(
        hash1
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "小写定长 hex"
    );
    let hash2 = put_blob(root.path(), "sess-a", &bytes).unwrap();
    assert_eq!(hash1, hash2, "同内容重复写入必须命中去重");
    use sha2::Digest;
    let expect: String = sha2::Sha256::digest(&bytes)[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(hash1, expect, "哈希 = SHA-256 前 16 字节（口径钉住）");
    let stored =
        std::fs::read(imgblob_dir(root.path(), "sess-a").join(format!("{hash1}.bin"))).unwrap();
    assert_eq!(stored, bytes, "文件名 <hash>.bin 且内容一致");

    // roundtrip：写后读回逐字节一致、按魔数恢复 MIME
    for (bytes, mime) in [
        (decode(PNG_1X1), "image/png"),
        (decode(JPEG_1X1), "image/jpeg"),
        (decode(GIF_1X1), "image/gif"),
    ] {
        let hash = put_blob(root.path(), "sess-a", &bytes).unwrap();
        let (got, got_mime) = get_blob(root.path(), "sess-a", &hash).unwrap();
        assert_eq!(got, bytes);
        assert_eq!(got_mime, mime, "读取侧按魔数恢复 MIME 类型");
    }

    // 写入拒绝：超限 / 空 / 白名单外格式
    let big = vec![0xffu8; MAX_BLOB_BYTES + 1];
    assert!(
        matches!(
            put_blob(root.path(), "sess-a", &big),
            Err(StorageError::InvalidArg(_))
        ),
        "超过 5MiB 拒绝"
    );
    assert!(
        matches!(
            put_blob(root.path(), "sess-a", &[]),
            Err(StorageError::InvalidArg(_))
        ),
        "空内容拒绝"
    );
    assert!(
        matches!(
            put_blob(root.path(), "sess-a", b"not an image"),
            Err(StorageError::InvalidArg(_))
        ),
        "白名单外格式拒绝"
    );

    // 越权与非法哈希：他会话 blob 不可见；路径穿越等按参数错误；合法但不存在的按 NotFound
    let hash = put_blob(root.path(), "sess-a", &decode(PNG_1X1)).unwrap();
    assert!(
        matches!(
            get_blob(root.path(), "sess-b", &hash),
            Err(StorageError::NotFound(_))
        ),
        "root 归属：仅本会话 imgblob 可读"
    );
    for bad in ["../evil", "zzz", "", &"a".repeat(33), "AAAA"] {
        assert!(
            matches!(
                get_blob(root.path(), "sess-a", bad),
                Err(StorageError::InvalidArg(_))
            ),
            "非法哈希 {bad:?} 应被拒绝"
        );
    }
    let missing = "0".repeat(32);
    assert!(
        matches!(
            get_blob(root.path(), "sess-a", &missing),
            Err(StorageError::NotFound(_))
        ),
        "合法但不存在的哈希按 NotFound"
    );

    // GC：走 SessionStore 全链删会话，blob 目录随之消失
    let root = temp_root("gc");
    let store = crate::storage::SessionStore::with_root(root.path().to_path_buf());
    store
        .index()
        .upsert(crate::storage::SessionMeta {
            id: "sess-img".to_string(),
            project_id: None,
            title: "带图会话".to_string(),
            created_at: 0,
            updated_at: 0,
            revision: 1,
            message_count: 0,
            total_tokens: 0,
            authorized_external_dirs: Vec::new(),
        })
        .unwrap();
    let hash = put_blob(root.path(), "sess-img", &decode(PNG_1X1)).unwrap();
    let dir = imgblob_dir(root.path(), "sess-img");
    assert!(dir.join(format!("{hash}.bin")).exists());
    store.delete_session("sess-img").unwrap();
    assert!(!dir.exists(), "会话删除必须连带删除图片 blob 目录");
}
