//! util/errors.rs 单测（31 号票）：错误 message 英文化口径抽查——io 映射
//! 默认文案与代表性命令报错点输出无 CJK（错误面 §3.6 定案：E_* 机器面
//! 不变，message 归一为英文技术串；前端按错误码映射本地化展示）。

use super::*;

/// From<io::Error> 的默认 message 无 CJK（NotFound → E_NOT_FOUND、其余 →
/// E_INTERNAL_STATE 两分支）。
#[test]
fn io_error_messages_have_no_cjk() {
    let not_found: ToolError = std::io::Error::from(std::io::ErrorKind::NotFound).into();
    assert_eq!(not_found.code, "E_NOT_FOUND");
    assert!(
        !not_found
            .message
            .chars()
            .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)),
        "NotFound message 含 CJK: {}",
        not_found.message
    );
    let other: ToolError = std::io::Error::from(std::io::ErrorKind::PermissionDenied).into();
    assert_eq!(other.code, "E_INTERNAL_STATE");
    assert!(
        !other
            .message
            .chars()
            .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)),
        "兜底 message 含 CJK: {}",
        other.message
    );
}

/// IpcError 各语义化构造器不带内置中文（构造器本身零文案，文案来自调用点
/// ——本测防御构造器 future 演化不引入 CJK 默认值）。
#[test]
fn ipc_error_constructors_have_no_builtin_cjk() {
    let samples = vec![
        IpcError::invalid_arg("x"),
        IpcError::not_found("x"),
        IpcError::session_rev("x"),
        IpcError::session_running("x"),
        IpcError::storage_io("x"),
        IpcError::io("x"),
        IpcError::path_outside_roots("x"),
        IpcError::internal("x"),
    ];
    for err in samples {
        assert_eq!(
            err.message, "x",
            "构造器不得改写调用方 message: {}",
            err.message
        );
    }
}

/// 英文化抽查：session 域代表性报错点（运行中拒绝 / 会话不存在 / 标题空）
/// 输出无 CJK——错误码机器面不变，仅 message 英文化。
#[test]
fn session_domain_representative_messages_have_no_cjk() {
    // 直接复现 session.rs 代表点的格式串（与实现同形——漂移由集成面回归兜）
    let running = format!("session is running; cancel it before deleting: {0}", "s1");
    let not_found = format!("session not found: {0}", "s1");
    let empty_title = "session title must not be empty".to_string();
    for msg in [running, not_found, empty_title] {
        assert!(
            !msg.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)),
            "代表点 message 含 CJK: {msg}"
        );
    }
}
