//! HTML 沙箱预览命令（`preview_register`）：
//! 前端把待预览的 HTML 全文注册进进程级 [`PreviewRegistry`]，换取 token 后
//! 以 `preview://localhost/<token>`（Windows `http://preview.localhost/`）
//! 的 iframe 加载——自定义协议响应自带预览专用 CSP，不继承主窗 CSP
//! （srcdoc/blob: 帧会继承主窗 `script-src 'self'` 拦内联脚本，
//! 已实测验证）。
//!
//! 注册表是进程内存（不落盘），应用退出清空；同内容幂等（token =
//! sha256 前 16 hex），历史卡片重新展开时前端按参数原文重注册兜底。

use serde::Deserialize;

use craftwave_engine::tools::render_html::preview_registry;
use craftwave_engine::util::errors::IpcError;

/// `preview_register` 参数：待预览的 HTML 全文（≤50,000 字符，与 render_html
/// 工具同款上限口径）。
#[derive(Debug, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PreviewRegisterArgs {
    /// 自包含 HTML 全文。
    pub html: String,
}

/// `preview_register` 返回：token 与按平台拼好的加载地址（前端直接作
/// iframe src，不自行判断平台）。
#[derive(Debug, serde::Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PreviewRegisterResult {
    /// 内容寻址 token（sha256 前 16 hex）。
    pub token: String,
    /// iframe 加载地址（预览协议 URL）。
    pub url: String,
}

/// 注册预览内容：字符数校验（与 render_html 同款）→ 注册表 upsert →
/// 返回 token + URL。幂等：同内容重复注册返回同 token。
#[tauri::command]
pub fn preview_register(args: PreviewRegisterArgs) -> Result<PreviewRegisterResult, IpcError> {
    craftwave_engine::tools::render_html::validate_html_length(&args.html)
        .map_err(|e| IpcError::new(e.code, e.message))?;
    let registry = preview_registry();
    let token = registry.register(&args.html);
    Ok(PreviewRegisterResult {
        url: craftwave_engine::tools::render_html::preview_url(&token),
        token,
    })
}

#[cfg(test)]
#[path = "preview_test.rs"]
mod preview_test;
