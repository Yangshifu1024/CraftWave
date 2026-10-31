//! 图片附件承载命令（`attach_image` / `read_image_blob`，
//! ）：会话图片 blob 的上传与读取。
//!
//! 职责边界：本模块只做参数校验与转调（校验在 `storage::imgblob`），
//! 数据落在会话专属目录 `sessions/<sid>.imgblob/<hash>.bin`（v1 口径：
//! 内容寻址 + 写入去重 + 会话删除时随目录整体删除）。root 归属：仅本会话
//! imgblob 可读（`read_image_blob` 按调用方给的 sessionId 定位目录，跨会话
//! 哈希查不到即 NotFound——blob 不跨会话共享）。
//!
//! 日志纪律（spec「图片与日志」条）：两条命令都**不把图片字节写进日志**，
//! 也不回传到事件面——字节只在 IPC 请求/响应体内传输一次。

use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::State;

use craftwave_engine::app_state::AppState;
use craftwave_engine::util::errors::IpcError;
use craftwave_engine::util::errors::{E_NOT_FOUND, E_STORAGE_IO};

/// `attach_image` 参数契约：`dataBase64` 与 `filePath` 二选一
/// （附件按钮/粘贴传字节；拖拽入口只有绝对路径，传 filePath 由后端读取）。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AttachImageArgs {
    pub session_id: String,
    /// 图片字节的 base64 编码（与 filePath 二选一；解码后 ≤5MiB，png/jpeg/
    /// gif/webp 白名单按文件头魔数识别——mimeType 参数仅作声明，落盘 mime
    /// 以嗅探结果为准）。
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub data_base64: Option<String>,
    /// 图片绝对路径（与 dataBase64 二选一；Tauri 拖拽事件的入口形态——
    /// HTML 拖放被 webview 原生接管时前端拿不到字节）。后端读取后照走
    /// 魔数嗅探与 5MiB 校验；**不做 roots 校验**：用户显式拖入与文件选择器
    /// 选图等价，模型侧文件可见性仍由 read 管线把关。
    #[serde(default)]
    #[cfg_attr(test, ts(optional))]
    pub file_path: Option<String>,
    /// 前端声明的 MIME 类型（须落在 png/jpeg/gif/webp 白名单内，防误传
    /// 非图片内容；filePath 形态时忽略——以魔数嗅探为准）。
    pub mime_type: String,
}

/// `attach_image` 返回契约：内容寻址哈希（SHA-256 前 32 hex）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct AttachImageResult {
    pub blob_hash: String,
}

/// MIME 白名单（与 `storage::imgblob::sniff_image_mime` 同一份口径）。
const MIME_WHITELIST: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// 上传图片附件：dataBase64 解码或 filePath 读取 → 白名单与
/// 字节数校验 → 内容寻址落盘（同哈希幂等去重）。返回 blobHash 供消息 part
/// 引用。会话日志不记图片字节（spec 日志纪律：仅元数据可入日志面）。
pub fn attach_image_core(
    app: &AppState,
    session_id: &str,
    data_base64: Option<&str>,
    file_path: Option<&str>,
    mime_type: &str,
) -> Result<AttachImageResult, IpcError> {
    if session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    // 两种入口形态二选一（都空/都有按参数错误拒绝——防调用方歧义）
    let bytes: Vec<u8> = match (data_base64, file_path) {
        (Some(_), Some(_)) => {
            return Err(IpcError::invalid_arg("dataBase64 与 filePath 只能二选一"));
        }
        (None, None) => return Err(IpcError::invalid_arg("dataBase64 与 filePath 至少给一个")),
        (Some(b64), None) => {
            if !MIME_WHITELIST.contains(&mime_type) {
                return Err(IpcError::invalid_arg(format!(
                    "不支持的图片类型：{mime_type}（仅 png/jpeg/gif/webp）"
                )));
            }
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| IpcError::invalid_arg(format!("dataBase64 不是合法 base64：{e}")))?
        }
        (None, Some(path)) => {
            // 拖拽入口：后端读路径（用户显式拖入与文件选择器等价，不做
            // roots 校验），mime 按魔数嗅探（mimeType 声明忽略）
            std::fs::read(path).map_err(|e| {
                IpcError::new(
                    craftwave_engine::util::errors::E_NOT_FOUND,
                    format!("读取拖入图片失败：{path}（{e}）"),
                )
            })?
        }
    };
    let blob_hash = craftwave_engine::storage::imgblob::put_blob(app.store.root(), session_id, &bytes)
        .map_err(|e| match e {
            craftwave_engine::storage::StorageError::InvalidArg(m) => IpcError::invalid_arg(m),
            other => IpcError::new(E_STORAGE_IO, other.to_string()),
        })?;
    Ok(AttachImageResult { blob_hash })
}

/// `attach_image` 命令。
#[tauri::command]
pub(crate) async fn attach_image(
    app: State<'_, AppState>,
    session_id: String,
    data_base64: Option<String>,
    file_path: Option<String>,
    mime_type: String,
) -> Result<AttachImageResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = AttachImageArgs {
        session_id,
        data_base64,
        file_path,
        mime_type,
    };
    attach_image_core(
        &app,
        &args.session_id,
        args.data_base64.as_deref(),
        args.file_path.as_deref(),
        &args.mime_type,
    )
}

/// `read_image_blob` 参数契约。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ReadImageBlobArgs {
    pub session_id: String,
    pub blob_hash: String,
}

/// `read_image_blob` 返回契约：base64 字节 + 嗅探出的 MIME。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ReadImageBlobResult {
    pub data_base64: String,
    pub mime_type: String,
}

/// 读取图片 blob：前端 `<img>` 渲染与历史恢复的取数通道。
/// root 归属：仅本会话 imgblob 可读；blob 缺失（会话目录被清等）按
/// E_NOT_FOUND 拒绝（前端据此灰显占位）。
pub fn read_image_blob_core(
    app: &AppState,
    session_id: &str,
    blob_hash: &str,
) -> Result<ReadImageBlobResult, IpcError> {
    if session_id.trim().is_empty() {
        return Err(IpcError::invalid_arg("sessionId 不能为空"));
    }
    let (bytes, mime) =
        craftwave_engine::storage::get_blob(app.store.root(), session_id, blob_hash).map_err(|e| match e {
            craftwave_engine::storage::StorageError::NotFound(m) => IpcError::new(E_NOT_FOUND, m),
            craftwave_engine::storage::StorageError::InvalidArg(m) => IpcError::invalid_arg(m),
            other => IpcError::new(E_STORAGE_IO, other.to_string()),
        })?;
    use base64::Engine;
    Ok(ReadImageBlobResult {
        data_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        mime_type: mime.to_owned(),
    })
}

/// `read_image_blob` 命令。
#[tauri::command]
pub(crate) async fn read_image_blob(
    app: State<'_, AppState>,
    session_id: String,
    blob_hash: String,
) -> Result<ReadImageBlobResult, IpcError> {
    // 契约命名单一事实源：平铺形参回组为文档契约结构
    let args = ReadImageBlobArgs {
        session_id,
        blob_hash,
    };
    read_image_blob_core(&app, &args.session_id, &args.blob_hash)
}
