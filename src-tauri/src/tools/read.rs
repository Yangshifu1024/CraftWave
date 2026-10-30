//! `read` 工具（ReadOnly，§8.3）：文本按行窗口（默认 2000 行，offset/limit）；
//! kind:"image" 走 base64（≤3MiB）；文本解码与 read_workspace_file 共用
//! util::encoding（C-10）。16 号票：.docx/.xlsx/.pdf 按扩展名分派进
//! tools/doc.rs 文档管线（§8.9 读侧），参数 page/pages（pdf 分页）与
//! sheet（xlsx 选表）随分派传入。

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

/// read 工具参数：目标路径 + 可选行窗口与读取类型（文档扩展名时附带
/// page/pages/sheet 文档参数）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadArgs {
    /// 工作区内目标文件路径（相对或绝对，须在 roots 内）。
    pub path: String,
    /// 行窗口起点，1-based（缺省 1；0 容错为 1）。仅文本路径有效。
    pub offset: Option<u64>,
    /// 行数（缺省 2000）。仅文本路径有效。
    pub limit: Option<u64>,
    /// 缺省 text；kind:"image" 对文档扩展名是误用（明确拒绝）。
    pub kind: Option<ReadKind>,
    /// pdf 单页页码（1-based，文档管线，16 号票 §8.3/§8.9）。
    pub page: Option<u32>,
    /// pdf 页码窗口 [起, 止]（1-based 闭区间，单次 ≤20 页）。
    pub pages: Option<(u32, u32)>,
    /// xlsx 要读取的 sheet 名（缺省输出清单 + 首 sheet 预览）。
    pub sheet: Option<String>,
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
    // 文档分派（16 号票，§8.3/§8.9）：.docx/.xlsx/.pdf 等扩展名走文档管线
    // （tools/doc.rs），与文本/图片路径并列；kind:"image" 对文档是误用。
    if crate::tools::doc::detect_format(&args.path).is_some() {
        if args.kind == Some(ReadKind::Image) {
            return ToolOutcome::failure(
                ToolError::invalid_arg(format!(
                    "文档文件不支持图片读取：{}（文档按扩展名自动走文档解析，无需 kind 参数）",
                    args.path
                )),
                format!("文档不支持图片读取：{}", args.path),
            );
        }
        return crate::tools::doc::run(
            ctx,
            &crate::tools::doc::DocArgs {
                path: args.path.clone(),
                page: args.page,
                pages: args.pages,
                sheet: args.sheet.clone(),
            },
        );
    }
    match args.kind.unwrap_or(ReadKind::Text) {
        ReadKind::Text => read_text(ctx, args),
        ReadKind::Image => read_image(ctx, args),
    }
}

/// 文本行窗口读取结果（read 与 batch_read 共用的内部形状，14 号票抽出）。
pub(crate) struct TextWindow {
    /// 解码判定出的编码名（BOM / UTF-8 / 探测结果，C-10）。
    pub encoding: String,
    /// 文件总行数。
    pub total_lines: u64,
    /// 本窗口起点行号（1-based）。
    pub offset: u64,
    /// 实际返回的行数。
    pub returned_lines: u64,
    /// 窗口内容（每行带换行拼接）。
    pub content: String,
    /// 内容未完整返回（预算截断或行窗口截断任一命中）。
    pub truncated: bool,
    /// 字节预算截断（512KB 整行边界截断；区别于行窗口截断）——batch_read
    /// 据此判定批内共享预算耗尽、后续文件让路。
    pub byte_truncated: bool,
    /// 截断时的续读起点行号（1-based，供模型直接当 offset 续读）；未截断为 None。
    pub next_offset: Option<u64>,
}

/// 文本行窗口读取的核心实现（read 工具与 batch_read 共用，14 号票抽出）：
/// 读字节 → 二进制检查（前 8KB 含 NUL 判二进制，C-10）→ 解码 → 行窗口
/// 切片 → `budget` 字节预算内按整行边界截断。
///
/// `display` 为报错与结果展示用的路径原文（相对/绝对保持调用方传入的形态）；
/// `budget` 为本次允许返回的最大内容字节数（read 单文件 = 512KB 上限；
/// batch_read 传批内剩余预算，14 号票共享上限语义）。
pub(crate) fn read_text_window(
    resolved: &std::path::Path,
    display: &str,
    offset: u64,
    limit: u64,
    budget: usize,
) -> Result<TextWindow, ToolError> {
    let bytes = std::fs::read(resolved).map_err(|e| {
        // io 错误带上目标路径原文（batch_read 逐文件标注需要能对上号）。
        match e.kind() {
            std::io::ErrorKind::NotFound => ToolError::not_found(format!("文件不存在：{display}")),
            _ => ToolError::new("E_INTERNAL_STATE", format!("读取 {display} 失败：{e}")),
        }
    })?;
    // 前 8KB 含 NUL 判二进制 → 报错不支持文本预览（C-10）。
    if is_probably_binary(&bytes) {
        return Err(ToolError::invalid_arg(format!(
            "二进制文件不支持文本读取：{display}（图片请用 read 的 kind:\"image\"）"
        )));
    }
    let decoded = decode_text(&bytes);
    let lines: Vec<&str> = decoded.text.lines().collect();
    let total_lines = lines.len() as u64;

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

    // 响应预算（C-10 单次 ≤512KB；batch_read 为批内剩余预算）：超限按整行
    // 边界截断。
    let mut content = String::new();
    let mut taken = 0usize;
    let mut byte_truncated = false;
    for line in &window {
        let needed = line.len() + 1;
        if content.len() + needed > budget && taken > 0 {
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
    let next_offset = truncated.then(|| offset.saturating_add(returned_lines));

    Ok(TextWindow {
        encoding: decoded.encoding.to_string(),
        total_lines,
        offset,
        returned_lines,
        content,
        truncated,
        byte_truncated,
        next_offset,
    })
}

/// 文本读取：解码 → 行窗口切片 → 512KB 单响应上限截断（整行边界）。
fn read_text(ctx: &ToolsContext, args: &ReadArgs) -> ToolOutcome {
    let resolved = match ctx.guard().and_then(|g| g.resolve(&args.path)) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::from(e),
    };
    let offset = args.offset.unwrap_or(1).max(1);
    let limit = args.limit.unwrap_or(DEFAULT_LINE_LIMIT);
    let window = match read_text_window(&resolved, &args.path, offset, limit, MAX_RESPONSE_BYTES) {
        Ok(w) => w,
        Err(e) => return ToolOutcome::from(e),
    };

    ToolOutcome::success(
        json!({
            "kind": "text",
            "path": args.path,
            "encoding": window.encoding,
            "totalLines": window.total_lines,
            "offset": window.offset,
            "returnedLines": window.returned_lines,
            "content": window.content,
            "truncated": window.truncated,
        }),
        format!(
            "读取 {} 第 {}–{} 行（共 {} 行，{}）",
            args.path,
            window.offset,
            window
                .offset
                .saturating_add(window.returned_lines.saturating_sub(1)),
            window.total_lines,
            window.encoding
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

/// read 工具规格（能力 ReadOnly，全档放行）。16 号票：params_schema 增
/// 文档参数（page/pages/sheet，§8.3 read 行回写），描述补文档分派语义；
/// 文档参数 schema 片段单一来源在 doc::params_schema_properties()。
pub fn spec() -> ToolSpec {
    let mut params_schema = json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "工作区内相对路径（或绝对路径，须在 roots 内）" },
            "offset": { "type": "integer", "description": "起始行号（1-based）" },
            "limit": { "type": "integer", "description": "返回行数，默认 2000" },
            "kind": { "type": "string", "enum": ["text", "image"] }
        },
        "required": ["path"],
        "additionalProperties": false
    });
    if let (Some(props), Some(doc_props)) = (
        params_schema
            .get_mut("properties")
            .and_then(|p| p.as_object_mut()),
        crate::tools::doc::params_schema_properties()
            .as_object()
            .cloned(),
    ) {
        for (key, value) in doc_props {
            props.insert(key, value);
        }
    }
    ToolSpec {
        name: "read".to_string(),
        description: "读取工作区内文件。文本按行窗口返回（默认 2000 行，可用 offset/limit 翻页）；\
             kind=\"image\" 以 base64 返回图片（≤3MiB）供视觉模型查看；\
             .docx/.xlsx/.pdf 等文档自动走文档解析：docx 输出段落与 Markdown 表格，xlsx 逐 sheet 表格（sheet 参数选表），\
             pdf 输出按页线性文本（>10 页须显式 page/pages，单次 ≤20 页）。二进制文件不支持文本读取。"
            .to_string(),
        capability: Capability::ReadOnly,
        params_schema,
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
#[path = "read_test.rs"]
mod read_test;
