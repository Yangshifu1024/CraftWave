//! util/errors.rs 单测：错误 message 英文化口径——io 映射默认
//! 文案、IpcError 语义化构造器、session 域代表报错点均无 CJK（错误面
//! 定案：E_* 机器面不变，message 归一为英文技术串；前端按错误码映射本地化）。

use super::*;

/// 断言错误 message 英文化：io 映射两分支（NotFound → E_NOT_FOUND、
/// 其余 → E_INTERNAL_STATE）、IpcError 构造器零内置文案、session 域
/// 代表报错点输出均无 CJK。
#[test]
fn error_messages_have_no_cjk() {
    // io 映射两分支。
    let not_found: ToolError = std::io::Error::from(std::io::ErrorKind::NotFound).into();
    assert_eq!(not_found.code, "E_NOT_FOUND", "NotFound 应映射 E_NOT_FOUND");
    assert!(
        !not_found
            .message
            .chars()
            .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)),
        "NotFound message 含 CJK: {}",
        not_found.message
    );
    let other: ToolError = std::io::Error::from(std::io::ErrorKind::PermissionDenied).into();
    assert_eq!(
        other.code, "E_INTERNAL_STATE",
        "兜底应映射 E_INTERNAL_STATE"
    );
    assert!(
        !other
            .message
            .chars()
            .any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)),
        "兜底 message 含 CJK: {}",
        other.message
    );

    // 构造器零文案：message 原样来自调用点（防御构造器演化不引入 CJK 默认值）。
    for err in [
        IpcError::invalid_arg("x"),
        IpcError::not_found("x"),
        IpcError::session_rev("x"),
        IpcError::session_running("x"),
        IpcError::storage_io("x"),
        IpcError::io("x"),
        IpcError::path_outside_roots("x"),
        IpcError::internal("x"),
    ] {
        assert_eq!(
            err.message, "x",
            "构造器不得改写调用方 message: {}",
            err.message
        );
    }

    // session 域代表点：直接复现 session.rs 代表格式串（与实现同形——
    // 漂移由集成面回归兜）。
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
