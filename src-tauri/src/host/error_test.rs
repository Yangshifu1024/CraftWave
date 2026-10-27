//! IpcError 单元测试（独立测试文件，源码零测试）。

use crate::host::IpcError;
use crate::storage::StorageError;

#[test]
fn error_shape_camel_case_detail_omitted() {
    let err = IpcError::session_rev("基线过期");
    let json = serde_json::to_value(&err).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "code": "E_SESSION_REV", "message": "基线过期" })
    );
}

#[test]
fn storage_error_to_ipc_error_code_mapping() {
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
