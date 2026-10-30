//! 会话偏好存取测试（18 号票）：roundtrip / 损坏兜底 / 缺失默认 / 原子写。

use std::fs;

use super::prefs::{SessionPrefs, load_prefs, prefs_path, save_prefs};
use crate::providers::reasoning::ThinkingLevel;

/// 临时数据根（每用例独立目录，退出自动清理）。
fn temp_root(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("cw-prefs-{tag}-"))
        .tempdir()
        .expect("建临时目录失败")
}

/// 完整 roundtrip：写两个偏好 → 原样读回；文件名与路径符合 §7.1 布局。
#[test]
fn prefs_roundtrip() {
    let root = temp_root("roundtrip");
    let prefs = SessionPrefs {
        model_id: Some("model-uuid-1".to_owned()),
        thinking: Some(ThinkingLevel::High),
    };
    save_prefs(root.path(), "sess-1", &prefs).expect("写偏好失败");
    // 文件落在 sessions/<sid>.prefs.json（§7.1 布局锚点）
    assert!(prefs_path(root.path(), "sess-1").is_file());
    assert_eq!(load_prefs(root.path(), "sess-1"), prefs);
}

/// 部分字段的文件形态：只写 thinking 时 modelId 键省略（skip_serializing_if），
/// 读回 model_id 为 None。
#[test]
fn partial_prefs_roundtrip_keeps_only_set_fields() {
    let root = temp_root("partial");
    save_prefs(
        root.path(),
        "sess-2",
        &SessionPrefs {
            model_id: None,
            thinking: Some(ThinkingLevel::Off),
        },
    )
    .expect("写偏好失败");
    let raw = fs::read_to_string(prefs_path(root.path(), "sess-2")).expect("读文件失败");
    assert!(
        !raw.contains("modelId"),
        "未设置字段不应出现在文件里：{raw}"
    );
    assert!(raw.contains("thinking"));
    assert_eq!(
        load_prefs(root.path(), "sess-2"),
        SessionPrefs {
            model_id: None,
            thinking: Some(ThinkingLevel::Off),
        }
    );
}

/// 文件缺失 → 空偏好（不报错）。
#[test]
fn missing_file_defaults_to_empty() {
    let root = temp_root("missing");
    assert_eq!(
        load_prefs(root.path(), "no-such-session"),
        SessionPrefs::default()
    );
}

/// 损坏文件 → 空偏好兜底（票面定案：不阻塞会话加载），且再次写入可自愈。
#[test]
fn corrupted_file_falls_back_to_empty_and_recovers() {
    let root = temp_root("corrupt");
    fs::create_dir_all(root.path().join("sessions")).expect("建目录失败");
    fs::write(prefs_path(root.path(), "sess-3"), "{ 不是 JSON").expect("写坏文件失败");
    assert_eq!(load_prefs(root.path(), "sess-3"), SessionPrefs::default());
    // 写入合法偏好后恢复正常（损坏文件被原子写覆盖）
    save_prefs(
        root.path(),
        "sess-3",
        &SessionPrefs {
            model_id: Some("m".to_owned()),
            thinking: Some(ThinkingLevel::Low),
        },
    )
    .expect("覆写偏好失败");
    assert_eq!(
        load_prefs(root.path(), "sess-3").thinking,
        Some(ThinkingLevel::Low)
    );
}

/// 非法档位值（反序列化失败）→ 空偏好兜底（容错口径：宁可丢偏好不炸会话）。
#[test]
fn unknown_thinking_level_falls_back_to_empty() {
    let root = temp_root("unknown-level");
    fs::create_dir_all(root.path().join("sessions")).expect("建目录失败");
    fs::write(prefs_path(root.path(), "sess-4"), r#"{"thinking":"ultra"}"#).expect("写非法值失败");
    assert_eq!(load_prefs(root.path(), "sess-4"), SessionPrefs::default());
}
