//! `render_html` 工具（ReadOnly*，§8.3 M2 表「*只读但产物可见」，FR-12-04）：
//! 接收 ≤50,000 字符的自包含 HTML，后端**不落盘、不解析**，只做长度校验与
//! 形态返回；渲染全在前端沙箱（iframe，`sandbox="allow-scripts"` 无
//! allow-same-origin，opaque origin 隔离）。
//!
//! 沙箱加载通道（14 号票定案）：srcdoc/blob: 帧会继承主窗 CSP
//! （`script-src 'self'` 拦预览页内联脚本，票面验证点证实），故走**自定义
//! 协议 `preview`**（lib.rs 注册）：协议响应自带预览专用 CSP
//! ([`PREVIEW_CSP`]，禁外联防内网探测、放内联脚本/样式），不继承主窗 CSP。
//! 内容传递：前端经 `preview_register` 命令把 HTML 存入进程级
//! [`PreviewRegistry`]（token = sha256 前 16 hex，幂等），iframe 以 token 取。
//! 注册表随应用退出清空；历史卡片重新展开时前端按参数里的 html 重注册。

use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::OnceLock;

use crate::safety::tier::Capability;
use crate::tools::{ToolAnnotations, ToolOutcome, ToolSpec};
use crate::util::errors::ToolError;

/// 单次 HTML 字符数上限（§8.3 render_html 行「≤50,000 字符」）。
pub const MAX_HTML_CHARS: usize = 50_000;
/// 预览注册表容量上限（每条 ≤50,000 字符，256 条 ≈ 12.8MB 内存上界）；
/// 超出按插入序淘汰最旧（长会话的旧预览由前端按参数重注册兜底）。
const MAX_PREVIEW_ENTRIES: usize = 256;

/// 预览页专用 CSP（14 号票定案，§9.5 回写）：
/// - `default-src 'none'` 兜底；仅放行内联脚本/样式与 data:/blob: 资源
///   （自包含 HTML 的合理需要）；
/// - **禁一切网络面**：connect-src/img-src 不放 http(s)——预览页脚本即便
///   被恶意构造也无法 fetch/XHR/加载远程图片（防借预览面板探测内网）；
/// - `frame-src 'none'` / `form-action 'none'` / `object-src 'none'`
///   防嵌套跳转与插件面；`base-uri 'none'` 防相对地址劫持。
///
/// 与 iframe 的 `sandbox="allow-scripts"`（无 allow-same-origin）双重隔离。
pub const PREVIEW_CSP: &str = "default-src 'none'; \
     script-src 'unsafe-inline' 'unsafe-eval' data: blob:; \
     style-src 'unsafe-inline' data: blob:; \
     img-src data: blob:; font-src data: blob:; media-src data: blob:; \
     connect-src data: blob:; worker-src data: blob:; \
     frame-src 'none'; form-action 'none'; object-src 'none'; base-uri 'none'";

/// 进程级预览注册表：token → HTML 全文。协议 handler 与 preview_register
/// 命令共享；应用退出即清空（不持久化——预览是会话内即时产物，历史卡片
/// 由前端按参数原文重注册）。
pub struct PreviewRegistry {
    inner: std::sync::Mutex<RegistryInner>,
}

struct RegistryInner {
    /// token → HTML 全文。
    map: HashMap<String, String>,
    /// 插入序（token），容量淘汰用。
    order: std::collections::VecDeque<String>,
}

impl PreviewRegistry {
    fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(RegistryInner {
                map: HashMap::new(),
                order: std::collections::VecDeque::new(),
            }),
        }
    }

    /// 注册一份 HTML（幂等：同内容同 token），容量超限淘汰最旧。
    pub fn register(&self, html: &str) -> String {
        let digest = Sha256::digest(html.as_bytes());
        let token: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !inner.map.contains_key(&token) {
            if inner.order.len() >= MAX_PREVIEW_ENTRIES
                && let Some(oldest) = inner.order.pop_front()
            {
                inner.map.remove(&oldest);
            }
            inner.order.push_back(token.clone());
        }
        inner.map.insert(token.clone(), html.to_string());
        token
    }

    /// 按 token 取 HTML（未命中 = 已失效/不存在）。
    pub fn lookup(&self, token: &str) -> Option<String> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .map
            .get(token)
            .cloned()
    }
}

/// 进程级单例（lib.rs 的协议 handler 与命令共用）。
pub fn preview_registry() -> &'static PreviewRegistry {
    static REGISTRY: OnceLock<PreviewRegistry> = OnceLock::new();
    REGISTRY.get_or_init(PreviewRegistry::new)
}

/// token 合法性（16 位小写 hex）；协议 handler 入参先过此校验再做查找。
pub fn is_valid_token(token: &str) -> bool {
    token.len() == 16 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 预览帧的加载地址（按平台拼：macOS/Linux 自定义 scheme，Windows/Android
/// 走 `http://<scheme>.localhost` 形态——Tauri 2 自定义协议的既定差异，
/// 见 tauri::Builder::register_uri_scheme_protocol 文档）。
pub fn preview_url(token: &str) -> String {
    if cfg!(any(windows, target_os = "android")) {
        format!("http://preview.localhost/{token}")
    } else {
        format!("preview://localhost/{token}")
    }
}

/// render_html 工具参数：自包含 HTML 全文。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderHtmlArgs {
    /// 要预览的自包含 HTML（≤50,000 字符）。
    pub html: String,
}

/// HTML 字符数校验（工具与 `preview_register` 命令共用同一上限口径）。
pub fn validate_html_length(html: &str) -> Result<usize, ToolError> {
    let char_count = html.chars().count();
    if char_count > MAX_HTML_CHARS {
        return Err(ToolError::invalid_arg(format!(
            "HTML 超过 {MAX_HTML_CHARS} 字符上限（实际 {char_count} 字符）：请精简内容或分块产出"
        )));
    }
    Ok(char_count)
}

/// 执行 render_html：只做字符数校验与形态返回（后端不落盘不解析，渲染全
/// 在前端沙箱）。
pub fn run(_ctx: &crate::tools::ToolsContext, args: &RenderHtmlArgs) -> ToolOutcome {
    let char_count = match validate_html_length(&args.html) {
        Ok(count) => count,
        Err(e) => return e.into(),
    };
    ToolOutcome::success(
        json!({
            "html": args.html,
            "charCount": char_count,
        }),
        format!("HTML 产出就绪（{char_count} 字符，可在预览卡查看）"),
    )
}

/// render_html 工具规格（ReadOnly*：只读但产物可见，全档放行）。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "render_html".to_string(),
        description: "产出自包含 HTML（≤50,000 字符）供用户在沙箱预览卡查看效果。后端不落盘\
             不解析；预览在无网络权限的隔离沙箱中渲染。写 HTML 页面、原型、图表后用它让用户直接看到结果。"
            .to_string(),
        capability: Capability::ReadOnly,
        params_schema: json!({
            "type": "object",
            "properties": {
                "html": { "type": "string", "description": "完整的自包含 HTML 文档（含 <!DOCTYPE html> 与内联样式/脚本）" }
            },
            "required": ["html"],
            "additionalProperties": false
        }),
        annotations: ToolAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: false,
        },
        origin: crate::tools::ToolOrigin::Static,
    }
}

#[cfg(test)]
#[path = "render_html_test.rs"]
mod render_html_test;
