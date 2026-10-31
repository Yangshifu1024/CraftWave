//! IpcError 单元测试（独立测试文件，源码零测试）。

use crate::host::IpcError;
use crate::storage::StorageError;

/// IPC 错误序列化形状：只含 code + message 两个字段（camelCase），
/// 不携带 details 等附加信息；存储层四种错误（NotFound / InvalidArg / Io /
/// RevisionConflict）经 `From` 映射后各自落在正确的 IPC 错误码上。
#[test]
fn error_shape_and_storage_error_mapping() {
    let err = IpcError::session_rev("基线过期");
    let json = serde_json::to_value(&err).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "code": "E_SESSION_REV", "message": "基线过期" })
    );

    assert_eq!(
        serde_json::to_value(IpcError::from(StorageError::NotFound("s1".into()))).unwrap()["code"],
        "E_NOT_FOUND"
    );
    assert_eq!(
        serde_json::to_value(IpcError::from(StorageError::InvalidArg("x".into()))).unwrap()["code"],
        "E_INVALID_ARG"
    );
    assert_eq!(
        serde_json::to_value(IpcError::from(StorageError::Io("disk".into()))).unwrap()["code"],
        "E_STORAGE_IO"
    );
    assert_eq!(
        serde_json::to_value(IpcError::from(StorageError::RevisionConflict {
            expected: 1,
            actual: 2
        }))
        .unwrap()["code"],
        "E_SESSION_REV"
    );
}
