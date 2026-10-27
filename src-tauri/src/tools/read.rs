//! `read` 工具（ReadOnly，§8.3）：文本按行窗口（默认 2000 行，offset/limit）；
//! kind:"image" 走 base64（≤3MiB）；文本解码与 read_workspace_file 共用
//! util::encoding（C-10）。

use base64::Engine;
use serde::Deserialize;
use serde_json::json;

use crate::safety::tier::Capability;
use crate::tools::{ToolAnnotations, ToolOutcome, ToolSpec, ToolsContext};
use crate::util::encoding::{MAX_RESPONSE_BYTES, decode_text, is_probably_binary};
use crate::util::errors::ToolError;

/// 默认行窗口（§8.3）。
pub const DEFAULT_LINE_LIMIT: u64 = 2000;
/// 图片单文件上限（§8.3）。
pub const MAX_IMAGE_BYTES: usize = 3 * 1024 * 1024;

/// read 工具参数：目标路径 + 可选行窗口与读取类型。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadArgs {
    /// 工作区内目标文件路径（相对或绝对，须在 roots 内）。
    pub path: String,
    /// 行窗口起点，1-based（缺省 1；0 容错为 1）。
    pub offset: Option<u64>,
    /// 行数（缺省 2000）。
    pub limit: Option<u64>,
    /// 缺省 text。
    pub kind: Option<ReadKind>,
}

/// 读取类型：文本（按行窗口）或图片（base64 供视觉模型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReadKind {
    /// 文本读取（行窗口 + 编码探测解码）。
    Text,
    /// 图片读取（base64，≤3MiB）。
    Image,
}

/// 执行 read。headless 纯逻辑：vision 模型前置校验由调用方（run loop）
/// 在选择 kind:"image" 前完成，本函数只做大小与格式校验（票 Answer 已记录）。
pub fn run(ctx: &ToolsContext, args: &ReadArgs) -> ToolOutcome {
    match args.kind.unwrap_or(ReadKind::Text) {
        ReadKind::Text => read_text(ctx, args),
        ReadKind::Image => read_image(ctx, args),
    }
}

/// 文本读取：解码 → 行窗口切片 → 512KB 单响应上限截断（整行边界）。
fn read_text(ctx: &ToolsContext, args: &ReadArgs) -> ToolOutcome {
    let resolved = match ctx.guard().and_then(|g| g.resolve(&args.path)) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::from(e),
    };
    let bytes = match std::fs::read(&resolved) {
        Ok(b) => b,
        Err(e) => return ToolOutcome::from(ToolError::from(e)),
    };
    // 前 8KB 含 NUL 判二进制 → 报错不支持文本预览（C-10）。
    if is_probably_binary(&bytes) {
        return ToolOutcome::failure(
            ToolError::invalid_arg(format!(
                "二进制文件不支持文本读取：{}（图片请用 kind:\"image\"）",
                args.path
            )),
            format!("二进制文件不支持读取：{}", args.path),
        );
    }
    let decoded = decode_text(&bytes);
    let lines: Vec<&str> = decoded.text.lines().collect();
    let total_lines = lines.len() as u64;

    let offset = args.offset.unwrap_or(1).max(1);
    let limit = args.limit.unwrap_or(DEFAULT_LINE_LIMIT);
    // offset 超过总行数 → 空窗口 + truncated=false。
    let start_idx = (offset.saturating_sub(1)) as usize;
    let window: Vec<&str> = if start_idx >= lines.len() {
        Vec::new()
    } else {
        lines[start_idx..]
            .iter()
            .take(limit as usize)
            .copied()
            .collect()
    };

    // 单次响应 ≤512KB（C-10）：超限按整行边界截断。
    let mut content = String::new();
    let mut taken = 0usize;
    let mut byte_truncated = false;
    for line in &window {
        let needed = line.len() + 1;
        if content.len() + needed > MAX_RESPONSE_BYTES && taken > 0 {
            byte_truncated = true;
            break;
        }
        content.push_str(line);
        content.push('\n');
        taken += 1;
    }
    let lines_truncated = (start_idx as u64) + (taken as u64) < total_lines;
    let truncated = byte_truncated || lines_truncated;
    let returned_lines = taken as u64;

    ToolOutcome::success(
        json!({
            "kind": "text",
            "path": args.path,
            "encoding": decoded.encoding,
            "totalLines": total_lines,
            "offset": offset,
            "returnedLines": returned_lines,
            "content": content,
            "truncated": truncated,
        }),
        format!(
            "读取 {} 第 {}–{} 行（共 {} 行，{}）",
            args.path,
            offset,
            offset.saturating_add(returned_lines.saturating_sub(1)),
            total_lines,
            decoded.encoding
        ),
    )
}

/// 图片读取：格式与大小校验后 base64 编码返回（供视觉模型消费）。
fn read_image(ctx: &ToolsContext, args: &ReadArgs) -> ToolOutcome {
    let resolved = match ctx.guard().and_then(|g| g.resolve(&args.path)) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::from(e),
    };
    let Some(mime) = image_mime(&args.path) else {
        return ToolOutcome::failure(
            ToolError::invalid_arg(format!(
                "不支持的图片格式：{}（支持 png/jpeg/gif/webp）",
                args.path
            )),
            format!("不支持的图片格式：{}", args.path),
        );
    };
    let bytes = match std::fs::read(&resolved) {
        Ok(b) => b,
        Err(e) => return ToolOutcome::from(ToolError::from(e)),
    };
    if bytes.len() > MAX_IMAGE_BYTES {
        return ToolOutcome::failure(
            ToolError::invalid_arg(format!(
                "图片超过 3MiB 上限：{}（{} 字节）",
                args.path,
                bytes.len()
            )),
            format!("图片超过 3MiB 上限：{}", args.path),
        );
    }
    let base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    ToolOutcome::success(
        json!({
            "kind": "image",
            "path": args.path,
            "mediaType": mime,
            "base64": base64,
        }),
        format!(
            "读取图片 {}（{}，{} KB）",
            args.path,
            mime,
            bytes.len() / 1024
        ),
    )
}

/// 按扩展名推断图片 MIME 类型（仅支持 png/jpeg/gif/webp，其余 None）。
fn image_mime(path: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// read 工具规格（能力 ReadOnly，全档放行）。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "read",
        description: "读取工作区内文件。文本按行窗口返回（默认 2000 行，可用 offset/limit 翻页）；kind=\"image\" 以 base64 返回图片（≤3MiB）供视觉模型查看。二进制文件不支持文本读取。".to_string(),
        capability: Capability::ReadOnly,
        params_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "工作区内相对路径（或绝对路径，须在 roots 内）" },
                "offset": { "type": "integer", "description": "起始行号（1-based）" },
                "limit": { "type": "integer", "description": "返回行数，默认 2000" },
                "kind": { "type": "string", "enum": ["text", "image"] }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        annotations: ToolAnnotations {
            read_only_hint: true,
            destructive_hint: false,
            idempotent_hint: true,
            open_world_hint: false,
        },
    }
}

#[cfg(test)]
#[path = "read_test.rs"]
mod read_test;
