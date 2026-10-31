//! 会话偏好存取测试（18 号票）：roundtrip / 损坏兜底 / 缺失默认 / 原子写。
//! 全链路合并为单个 #[test]，断言消息带阶段标识。

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

// 断言：会话偏好全链路——完整 roundtrip（文件落在 §7.1 布局锚点、原样读回）；
// 部分字段文件形态（未设置键整段省略）；文件缺失 → 空偏好不报错；损坏文件 →
// 空偏好兜底且再次写入自愈（票面定案：不阻塞会话加载）；非法档位值（反序列化
// 失败）→ 空偏好兜底（宁可丢偏好不炸会话）。
#[test]
fn prefs_roundtrip_and_fallbacks() {
    // 完整 roundtrip
    let root = temp_root("roundtrip");
    let prefs = SessionPrefs {
        model_id: Some("model-uuid-1".to_owned()),
        thinking: Some(ThinkingLevel::High),
    };
    save_prefs(root.path(), "sess-1", &prefs).expect("写偏好失败");
    // 文件落在 sessions/<sid>.prefs.json（§7.1 布局锚点）
    assert!(prefs_path(root.path(), "sess-1").is_file());
    assert_eq!(load_prefs(root.path(), "sess-1"), prefs, "原样读回");

    // 部分字段：只写 thinking 时 modelId 键省略（skip_serializing_if）
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

    // 文件缺失 → 空偏好（不报错）
    let root = temp_root("missing");
    assert_eq!(
        load_prefs(root.path(), "no-such-session"),
        SessionPrefs::default(),
        "缺失按空偏好"
    );

    // 损坏文件 → 空偏好兜底，再次写入自愈（损坏文件被原子写覆盖）
    let root = temp_root("corrupt");
    fs::create_dir_all(root.path().join("sessions")).expect("建目录失败");
    fs::write(prefs_path(root.path(), "sess-3"), "{ 不是 JSON").expect("写坏文件失败");
    assert_eq!(load_prefs(root.path(), "sess-3"), SessionPrefs::default());
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
        Some(ThinkingLevel::Low),
        "损坏后写入自愈"
    );

    // 非法档位值（反序列化失败）→ 空偏好兜底
    let root = temp_root("unknown-level");
    fs::create_dir_all(root.path().join("sessions")).expect("建目录失败");
    fs::write(prefs_path(root.path(), "sess-4"), r#"{"thinking":"ultra"}"#).expect("写非法值失败");
    assert_eq!(
        load_prefs(root.path(), "sess-4"),
        SessionPrefs::default(),
        "非法档位按空偏好"
    );
}
