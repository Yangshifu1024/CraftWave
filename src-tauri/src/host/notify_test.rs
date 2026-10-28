//! notify.rs 的纯逻辑单测（19 号票）：macOS 授权态到前端口径的映射。
//! FFI 查询本体（query_macos_authorization）依赖主线程与系统回调，不在
//! 单测覆盖内；这里钉死的是映射口径（前端引导文案按 status 分支）。

#![cfg(target_os = "macos")]

use objc2_user_notifications::UNAuthorizationStatus;

/// 全部五种系统授权态的映射口径：prompt/denied 保留，其余都视为可送达。
#[test]
fn authorization_status_maps_to_frontend_vocabulary() {
    assert_eq!(
        super::authorization_word(UNAuthorizationStatus::NotDetermined),
        super::AUTH_PROMPT,
        "未决定 → prompt（设置页显示首次授权引导）"
    );
    assert_eq!(
        super::authorization_word(UNAuthorizationStatus::Denied),
        super::AUTH_DENIED,
        "被拒 → denied（设置页显示去系统设置的引导）"
    );
    assert_eq!(
        super::authorization_word(UNAuthorizationStatus::Authorized),
        super::AUTH_GRANTED,
        "已授权 → granted"
    );
    assert_eq!(
        super::authorization_word(UNAuthorizationStatus::Provisional),
        super::AUTH_GRANTED,
        "Provisional（静音送达）能送达 → granted"
    );
    assert_eq!(
        super::authorization_word(UNAuthorizationStatus::Ephemeral),
        super::AUTH_GRANTED,
        "Ephemeral 能送达 → granted"
    );
}

/// 四个前端口径常量的字面值即 IPC 契约面（与 ui 侧 types 判别值一致）。
#[test]
fn authorization_vocabulary_contract_values() {
    assert_eq!(super::AUTH_GRANTED, "granted");
    assert_eq!(super::AUTH_DENIED, "denied");
    assert_eq!(super::AUTH_PROMPT, "prompt");
    assert_eq!(super::AUTH_UNKNOWN, "unknown");
}
