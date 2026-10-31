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

/// 断言 attach_image 的承载与校验语义：base64 入口内容寻址落盘（哈希 =
/// SHA-256 前 32 hex、重复上传幂等去重、文件落会话 imgblob 目录 §7.7 v1）；
/// 拖拽路径入口与 base64 同内容同哈希（拖入与粘贴等价）、路径不存在
/// E_NOT_FOUND；校验面表驱动——非白名单 mime、超 5MiB、魔数不过、非法
/// base64、空会话 id、入口形态缺失/双填全部 E_INVALID_ARG；
/// 以及 read_image_blob 的读取与边界语义：本会话读回字节与上传一致且 mime
/// 按魔数嗅探；会话 B 读会话 A 的 blob 不可见（E_NOT_FOUND）；非法哈希
/// （含路径穿越向量）E_INVALID_ARG；合法格式但不存在的哈希 E_NOT_FOUND
/// （前端灰显占位的判定面）。
#[test]
fn attach_and_read_image_blob_semantics() {
    let (app, dir) = app();
    // base64 入口：内容寻址 + 幂等去重。
    let first = attach_image_core(&app, "sess-1", Some(PNG_1X1), None, "image/png").unwrap();
    let second = attach_image_core(&app, "sess-1", Some(PNG_1X1), None, "image/png").unwrap();
    assert_eq!(first.blob_hash, second.blob_hash, "重复上传同图幂等去重");
    assert_eq!(first.blob_hash.len(), 32);
    // 文件落在会话 imgblob 目录（§7.7 v1 路径口径）
    let path = app
        .store
        .root()
        .join("sessions")
        .join("sess-1.imgblob")
        .join(format!("{}.bin", first.blob_hash));
    assert!(path.exists());

    // 拖拽入口形态：后端读路径（不做 roots 校验——用户显式拖入与文件选择器
    // 等价的 v1 决策），mime 按魔数嗅探；两种入口形态同内容同哈希。
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
    assert_eq!(from_path.blob_hash, first.blob_hash, "拖入与粘贴等价");
    let err = attach_image_core(
        &app,
        "sess-1",
        None,
        Some("/nonexistent/x.png"),
        "image/png",
    )
    .unwrap_err();
    assert_eq!(err.code, "E_NOT_FOUND");

    // 校验面表驱动。
    let big = vec![0xffu8; 5 * 1024 * 1024 + 1];
    let big_b64 = base64::engine::general_purpose::STANDARD.encode(&big);
    for (why, b64, path, mime) in [
        (
            "非白名单 mime 声明拒绝（防误传非图片内容）",
            Some(PNG_1X1),
            None,
            "image/svg+xml",
        ),
        (
            "超过 5MiB 拒绝（解码后字节数判定）",
            Some(big_b64.as_str()),
            None,
            "image/png",
        ),
        (
            "声明 png 但字节是文本（魔数嗅探不过）——mime 声明与内容双保险",
            Some("bm90IGFuIGltYWdl"),
            None,
            "image/png",
        ),
        (
            "非法 base64 拒绝",
            Some("!!!not-base64!!!"),
            None,
            "image/png",
        ),
        ("空会话 id 拒绝", Some(PNG_1X1), None, "image/png"),
        ("两种入口形态都空按参数错误拒绝", None, None, "image/png"),
        (
            "两种入口形态都有按参数错误拒绝",
            Some(PNG_1X1),
            Some("/x.png"),
            "image/png",
        ),
    ] {
        let session = if why.starts_with("空会话") {
            ""
        } else {
            "sess-1"
        };
        let err = attach_image_core(&app, session, b64, path, mime).unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARG", "「{why}」应 E_INVALID_ARG");
    }

    // read_image_blob 的读取与边界语义（独立数据根，避免与前段互相污染）。
    let dir = tempfile::tempdir().unwrap();
    let app = AppState::new_in(
        dir.path().to_path_buf(),
        std::sync::Arc::new(RecordingEventSink::default()),
    );
    let attached = attach_image_core(&app, "sess-1", Some(PNG_1X1), None, "image/png").unwrap();

    // 本会话读回一致。
    let got = read_image_blob_core(&app, "sess-1", &attached.blob_hash).unwrap();
    assert_eq!(got.mime_type, "image/png");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&got.data_base64)
        .unwrap();
    let expected = base64::engine::general_purpose::STANDARD
        .decode(PNG_1X1)
        .unwrap();
    assert_eq!(decoded, expected, "读回字节与上传一致");

    // root 归属：会话 B 读会话 A 的 blob 必须不可见。
    let err = read_image_blob_core(&app, "sess-2", &attached.blob_hash).unwrap_err();
    assert_eq!(err.code, "E_NOT_FOUND");
    // 非法哈希（含路径穿越向量）按参数错误拒绝。
    for bad in ["../evil", "zzz", "AAAA"] {
        let err = read_image_blob_core(&app, "sess-1", bad).unwrap_err();
        assert_eq!(err.code, "E_INVALID_ARG", "非法哈希 {bad:?} 应被拒绝");
    }
    // 合法格式但不存在的哈希按 E_NOT_FOUND。
    let missing = "0".repeat(32);
    let err = read_image_blob_core(&app, "sess-1", &missing).unwrap_err();
    assert_eq!(err.code, "E_NOT_FOUND");
}
