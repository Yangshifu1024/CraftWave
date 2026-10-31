//! 窗口几何记忆的单元测试（29 号票）：越界判定纯函数表驱动、文件损坏
//! 容错、serde 往返。全部本地零依赖，不碰真实窗口。

use std::path::PathBuf;

use super::*;

/// 表驱动：越界判定（§10.7 口径——与任一显示器可见交集 ≥120×80 逻辑像素）。
/// 主显示器 1920×1080 @ (0,0)，副显示器 2560×1440 @ (1920,0)（逻辑坐标）。
#[test]
fn is_recoverable_table() {
    let monitors = vec![
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        },
        Rect {
            x: 1920.0,
            y: 0.0,
            width: 2560.0,
            height: 1440.0,
        },
    ];
    let rect = |x: f64, y: f64, width: f64, height: f64| Rect {
        x,
        y,
        width,
        height,
    };
    let cases: Vec<(&str, Rect, bool)> = vec![
        // 正常位：完全在主显示器内
        ("正常位", rect(100.0, 100.0, 1280.0, 800.0), true),
        // 右半出界：与主显示器交集仍有 960×800，达标
        ("右半出界", rect(960.0, 100.0, 1280.0, 800.0), true),
        // 拔掉副显示器后窗口留在原副屏区域：只剩主屏时零交集 → 全出界丢弃
        // （双屏清单下该位置在副屏内属合法，见"副屏内可见"用例）
        (
            "全出界（副屏区域，单屏清单）",
            rect(2000.0, 100.0, 1280.0, 800.0),
            false,
        ),
        // 窗口整体在副屏内：多显示器场景正常恢复
        ("副屏内可见", rect(2000.0, 100.0, 1280.0, 800.0), true),
        // 负坐标：向左上拖出大半，残留 100×500——宽不足 120 丢弃
        (
            "负坐标残留不足",
            rect(-1180.0, -300.0, 1280.0, 800.0),
            false,
        ),
        // 负坐标：残留 200×400——达标恢复
        ("负坐标残留达标", rect(-1080.0, -300.0, 1280.0, 800.0), true),
        // 高度不足：交集 960×60（<80）丢弃
        ("纵向残留不足", rect(100.0, 1050.0, 1280.0, 800.0), false),
        // 极小窗口但完全可见：位置合法即可恢复（窗口尺寸本身不受此判定管）
        ("小窗可见", rect(500.0, 500.0, 150.0, 100.0), true),
        // 恰好压线：交集 120×80（边界含）达标
        ("压线达标", rect(1800.0, 1000.0, 1280.0, 800.0), true),
        // 单显示器也能命中：清单只留主屏时右半出界仍达标
        (
            "单显示器右半出界",
            {
                let _ = &monitors;
                rect(960.0, 100.0, 1280.0, 800.0)
            },
            true,
        ),
    ];
    for (name, window_rect, expected) in cases {
        let list: &[Rect] = match name {
            // "单显示器右半出界" 与 "全出界（副屏区域…）"用单屏清单
            // （模拟拔掉副显示器后重启的场景），其余用双屏清单
            "单显示器右半出界" | "全出界（副屏区域，单屏清单）" => {
                &monitors[..1]
            }
            _ => &monitors,
        };
        assert_eq!(
            is_recoverable(&window_rect, list),
            expected,
            "用例「{name}」判定不符"
        );
    }
}

/// 最大化布尔不参与越界判定：is_recoverable 只看几何，maximized 的语义由
/// restore 阶段"恢复还原前几何后再最大化"承接（此处钉住判定不受污染）。
#[test]
fn maximized_flag_not_part_of_geometry_check() {
    let monitors = vec![Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    }];
    let state = WindowState {
        version: STATE_VERSION,
        x: 100.0,
        y: 100.0,
        width: 1280.0,
        height: 800.0,
        maximized: true,
    };
    assert!(is_recoverable(&state.rect(), &monitors));
}

/// 空显示器清单（available_monitors 异常）→ 保守丢弃：调用方判空返回。
/// 此处钉住纯函数对空清单返回 false（restore 入口的判空与它双保险）。
#[test]
fn empty_monitor_list_never_recoverable() {
    let rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1280.0,
        height: 800.0,
    };
    assert!(!is_recoverable(&rect, &[]));
}

/// serde 往返：save → load 原样取回（临时目录，util::atomic 真实写盘）。
#[test]
fn save_load_roundtrip() {
    let dir = tempfile::tempdir().expect("临时目录");
    let path = dir.path().join("window-state.json");
    let state = WindowState {
        version: STATE_VERSION,
        x: -42.5,
        y: 300.25,
        width: 1600.0,
        height: 900.0,
        maximized: true,
    };
    let value = serde_json::to_value(&state).expect("序列化");
    atomic::write_json(&path, &value).expect("写盘");
    assert_eq!(load_state(&path), Some(state));
}

/// 文件不存在 → None（首次启动回默认，不报错）。
#[test]
fn missing_file_is_none() {
    let dir = tempfile::tempdir().expect("临时目录");
    assert_eq!(load_state(&dir.path().join("absent.json")), None);
}

/// 表驱动：损坏文件各形态整体丢弃——非法 JSON / 缺字段 / 版本不符 / 尺寸非正。
#[test]
fn corrupt_file_table() {
    let dir = tempfile::tempdir().expect("临时目录");
    let cases: Vec<(&str, String)> = vec![
        ("非法 JSON", "not json at all".to_string()),
        ("空文件", String::new()),
        (
            "缺 width 字段",
            r#"{"version":1,"x":1,"y":2,"height":800,"maximized":false}"#.to_string(),
        ),
        (
            "缺 maximized 字段",
            r#"{"version":1,"x":1,"y":2,"width":1280,"height":800}"#.to_string(),
        ),
        (
            "版本不符",
            r#"{"version":99,"x":1,"y":2,"width":1280,"height":800,"maximized":false}"#.to_string(),
        ),
        (
            "宽为零",
            r#"{"version":1,"x":1,"y":2,"width":0,"height":800,"maximized":false}"#.to_string(),
        ),
        (
            "高为负",
            r#"{"version":1,"x":1,"y":2,"width":1280,"height":-800,"maximized":false}"#.to_string(),
        ),
    ];
    for (name, body) in cases {
        let path: PathBuf = dir.path().join("window-state.json");
        std::fs::write(&path, body).expect("写损坏样本");
        assert_eq!(load_state(&path), None, "用例「{name}」应整体丢弃");
    }
}
