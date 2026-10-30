//! 图片附件承载命令的单元测试（17 号票）：attach_image 的白名单/超限/合法
//! base64 校验与去重、read_image_blob 的本会话读取/跨会话隔离/非法哈希拒绝。

use base64::Engine;

use super::image::{attach_image_core, read_image_blob_core};
use crate::host::AppState;
use crate::util::events::RecordingEventSink;

/// 1x1 透明 PNG（base64 内嵌，程序解码——测试样本不依赖外部文件）。
const PNG_1X1: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

fn app() -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    (
        AppState::new_in(
            dir.path().to_path_buf(),
            std::sync::Arc::new(RecordingEventSink::default()),
        ),
        dir,
    )
}

#[test]
fn attach_image_puts_content_addressed_blob() {
    let (app, _dir) = app();
    let first = attach_image_core(&app, "sess-1", Some(PNG_1X1), None, "image/png").unwrap();
    // 哈希 = SHA-256 前 32 hex，重复上传同图幂等去重（同哈希返回）
    let second = attach_image_core(&app, "sess-1", Some(PNG_1X1), None, "image/png").unwrap();
    assert_eq!(first.blob_hash, second.blob_hash);
    assert_eq!(first.blob_hash.len(), 32);
    // 文件落在会话 imgblob 目录（§7.7 v1 路径口径）
    let path = app
        .store
        .root()
        .join("sessions")
        .join("sess-1.imgblob")
        .join(format!("{}.bin", first.blob_hash));
    assert!(path.exists());
}

#[test]
fn attach_image_from_file_path_matches_base64_form() {
    let (app, dir) = app();
    // 拖拽入口形态：后端读路径（不做 roots 校验——用户显式拖入与文件选择器
    // 等价的 v1 决策），mime 按魔数嗅探
    let png_path = dir.path().join("dropped.png");
    let png_bytes = base64::engine::general_purpose::STANDARD
        .decode(PNG_1X1)
        .unwrap();
    std::fs::write(&png_path, &png_bytes).unwrap();
    let from_path = attach_image_core(
        &app,
        "sess-1",
        None,
        Some(png_path.to_str().unwrap()),
        "image/png",
    )
    .unwrap();
    let from_b64 = attach_image_core(&app, "sess-1", Some(PNG_1X1), None, "image/png").unwrap();
    // 内容寻址：两种入口形态同内容同哈希（拖入与粘贴等价）
    assert_eq!(from_path.blob_hash, from_b64.blob_hash);
    // 路径不存在按 E_NOT_FOUND
    let err = attach_image_core(
        &app,
        "sess-1",
        None,
        Some("/nonexistent/x.png"),
        "image/png",
    )
    .unwrap_err();
    assert_eq!(err.code, "E_NOT_FOUND");
}

#[test]
fn attach_image_rejects_bad_mime_oversize_and_base64() {
    let (app, _dir) = app();
    // 非白名单 mime 声明拒绝（防误传非图片内容）
    let err = attach_image_core(&app, "sess-1", Some(PNG_1X1), None, "image/svg+xml").unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
    // 超过 5MiB 拒绝（解码后字节数判定）
    let big = vec![0xffu8; 5 * 1024 * 1024 + 1];
    let big_b64 = base64::engine::general_purpose::STANDARD.encode(&big);
    let err = attach_image_core(&app, "sess-1", Some(&big_b64), None, "image/png").unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
    // 声明 png 但字节是文本（魔数嗅探不过）拒绝——mime 声明与内容双保险
    let err = attach_image_core(
        &app,
        "sess-1",
        Some(&base64::engine::general_purpose::STANDARD.encode(b"not an image")),
        None,
        "image/png",
    )
    .unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
    // 非法 base64 拒绝
    let err =
        attach_image_core(&app, "sess-1", Some("!!!not-base64!!!"), None, "image/png").unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
    // 空会话 id 拒绝
    let err = attach_image_core(&app, "", Some(PNG_1X1), None, "image/png").unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
    // 两种入口形态都空/都有按参数错误拒绝
    let err = attach_image_core(&app, "sess-1", None, None, "image/png").unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
    let err =
        attach_image_core(&app, "sess-1", Some(PNG_1X1), Some("/x.png"), "image/png").unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
}

#[test]
fn read_image_blob_roundtrips_with_sniffed_mime() {
    let (app, _dir) = app();
    let attached = attach_image_core(&app, "sess-1", Some(PNG_1X1), None, "image/png").unwrap();
    let got = read_image_blob_core(&app, "sess-1", &attached.blob_hash).unwrap();
    assert_eq!(got.mime_type, "image/png");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&got.data_base64)
        .unwrap();
    let expected = base64::engine::general_purpose::STANDARD
        .decode(PNG_1X1)
        .unwrap();
    assert_eq!(decoded, expected, "读回字节与上传一致");
}

#[test]
fn read_image_blob_enforces_session_root_and_hash_shape() {
    let (app, _dir) = app();
    let attached = attach_image_core(&app, "sess-1", Some(PNG_1X1), None, "image/png").unwrap();
    // root 归属：会话 B 读会话 A 的 blob 必须不可见（E_NOT_FOUND）
    let err = read_image_blob_core(&app, "sess-2", &attached.blob_hash).unwrap_err();
    assert_eq!(err.code, "E_NOT_FOUND");
    // 非法哈希（含路径穿越向量）按参数错误拒绝
    for bad in ["../evil", "zzz", "AAAA"] {
        let err = read_image_blob_core(&app, "sess-1", bad).unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARG", "非法哈希 {bad:?} 应被拒绝");
    }
    // 合法格式但不存在的哈希按 E_NOT_FOUND（前端灰显占位的判定面）
    let missing = "0".repeat(32);
    let err = read_image_blob_core(&app, "sess-1", &missing).unwrap_err();
    assert_eq!(err.code, "E_NOT_FOUND");
}
