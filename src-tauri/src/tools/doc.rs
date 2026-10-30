//! 文档读取管线（16 号票，§8.9 读侧）：docx / xlsx / pdf 三类二进制文档
//! 解析为模型可读的结构化 Markdown 文本，挂在 `read` 工具的扩展名分派上
//! （§8.3 read 行——文档读取不设独立工具，与文本/图片路径并列）。
//!
//! 选型（15 号票定案，全部纯 Rust、MIT/Apache 兼容、零系统依赖）：
//! - docx → docx-rs（bokuweb）`read_docx`：段落线性输出 + 表格转 Markdown；
//!   已知开放缺陷 #873（合法文件解 zip 失败）与 #933（非默认命名空间前缀
//!   丢文本）走降级路径——zip + quick-xml 手解 `word/document.xml`（按本地
//!   名匹配，天然免疫命名空间前缀差异），降级结果带 warnings「降级解析」
//!   （§8.2：warnings 仅 UI 显示不进模型文本）。
//! - xlsx → calamine：逐 sheet `worksheet_range`（天然二维表）转 Markdown
//!   表格；公式单元格取文件内缓存值（calamine 的 `worksheet_formula` 备而
//!   不用，v1 不输出公式文本）；xls/xlsm/xlsb/ods 顺带覆盖。
//! - pdf → pdf-extract：`extract_text_from_mem_by_pages` 按页提取线性文本
//!   （v1 无表格重建，15 号票口径）；`catch_unwind` 包裹（#97/#141 对不可信
//!   输入可能 panic）→ panic 转结构化错误；内容头部标注「提取可能不完整」
//!   （部分中文 PDF CMap 限制 #103）；>10 页须显式页码、单次窗口 ≤20 页
//!   （§8.3 read 行 / §8.9 口径）。
//!
//! 大小口径（与 read_workspace_file #28 同款）：单次响应 ≤512KB，超限按整行
//! 边界截断 + truncated + 续读指引（pdf 用 page/pages、xlsx 用 sheet；docx
//! v1 无分页参数，截断如实说明）。文档本体先检查大小再读取（超 50MiB 拒绝），
//! 路径守卫复用 read 的 safe_join roots。

use std::io::{Cursor, Read as _};

use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::json;

use crate::tools::{ToolOutcome, ToolsContext};
use crate::util::encoding::MAX_RESPONSE_BYTES;
use crate::util::errors::ToolError;

/// 文档文件本体大小上限（读取前拒绝，防超大文件与 zip 炸弹进入解析）。
pub const MAX_DOC_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// docx 降级路径解压 `word/document.xml` 的输入字节上限（zip 炸弹防线）。
const MAX_DOCUMENT_XML_BYTES: u64 = 64 * 1024 * 1024;
/// PDF「须显式页码」的页数门槛（§8.3 read 行：PDF>10 页须显式页码）。
pub const PDF_EXPLICIT_PAGE_THRESHOLD: usize = 10;
/// PDF 单次页码窗口上限（§8.9：单次 ≤20 页）。
pub const PDF_MAX_PAGE_WINDOW: u32 = 20;
/// xlsx 缺省预览行数（未给 sheet 参数时输出首 sheet 前 N 行，票面「前 N 行」
/// 未定值，取 100 平衡预览完整度与响应预算）。
pub const XLSX_DEFAULT_PREVIEW_ROWS: usize = 100;
/// docx 降级解析允许的表格嵌套深度上限（防恶意深嵌套 XML 打爆调用栈）。
const MAX_TABLE_DEPTH: usize = 16;

/// 支持的文档格式（按扩展名判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocFormat {
    /// Word 文档（.docx）。
    Docx,
    /// 电子表格（.xlsx 及 calamine 覆盖的 xls/xlsm/xlsb/ods）。
    Xlsx,
    /// PDF 文档（.pdf）。
    Pdf,
}

/// 按扩展名判定文档格式（大小写不敏感；非文档扩展名返回 None）。
pub fn detect_format(path: &str) -> Option<DocFormat> {
    let ext = std::path::Path::new(path)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    match ext.as_str() {
        "docx" => Some(DocFormat::Docx),
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" => Some(DocFormat::Xlsx),
        "pdf" => Some(DocFormat::Pdf),
        _ => None,
    }
}

/// 文档读取参数（read 工具参数的文档子集；由 read.rs 按扩展名分派构造）。
#[derive(Debug, Clone, Default)]
pub struct DocArgs {
    /// 工作区内目标文件路径（与 read 的 path 相同）。
    pub path: String,
    /// pdf：单页页码（1-based）。
    pub page: Option<u32>,
    /// pdf：页码窗口 [起, 止]（1-based、闭区间，单次 ≤20 页）。
    pub pages: Option<(u32, u32)>,
    /// xlsx：要读取的 sheet 名（缺省输出 sheet 名清单 + 首 sheet 预览）。
    pub sheet: Option<String>,
}

// ---------------------------------------------------------------------------
// 文档中间表示：块序列（段落 / 表格），三类格式共用一套 Markdown 渲染
// ---------------------------------------------------------------------------

/// 文档内容块：段落文本或表格（行 × 单元格文本）。
#[derive(Debug, Clone)]
enum DocBlock {
    /// 一个正文段落（原样文本，可能含 \t / \n）。
    Paragraph(String),
    /// 一个表格。`nested` 为 true 表示该表位于上一表格的某个单元格内
    /// （嵌套表格展平为独立块输出，渲染时带标注行）。
    Table {
        /// 行 × 单元格文本（单元格内换行已归一为空格、竖线已转义）。
        rows: Vec<Vec<String>>,
        /// 是否为嵌套表格（展平标注用）。
        nested: bool,
    },
}

/// 判断块序列是否不含任何非空白文本（#933 检测启发：docx-rs 成功解析但
/// 一个字都没提出来，而文件里其实有文本）。
fn blocks_have_no_text(blocks: &[DocBlock]) -> bool {
    blocks.iter().all(|b| match b {
        DocBlock::Paragraph(t) => t.trim().is_empty(),
        DocBlock::Table { rows, .. } => rows.iter().all(|r| r.iter().all(|c| c.trim().is_empty())),
    })
}

/// Markdown 表格单元格内的文本归一：换行转空格（Markdown 表格单元格不允许
/// 硬换行）、竖线转义（表格语法保留字符）。
fn normalize_cell(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' | '\r' => out.push(' '),
            '|' => out.push_str("\\|"),
            _ => out.push(ch),
        }
    }
    out.trim().to_string()
}

/// 把块序列渲染为 Markdown 文本，并在 `budget` 字节预算内按整行边界截断。
/// 返回（内容, 是否截断）。渲染顺序即文档流顺序；表格首行后插 Markdown
/// 表头分隔行（保证按表格渲染）。
fn render_blocks(blocks: &[DocBlock], budget: usize) -> (String, bool) {
    let mut out = String::new();
    let mut truncated = false;
    for block in blocks {
        // 预算检查：本块渲染前若已接近预算，直接停（整块让位，避免半行）。
        if out.len() >= budget {
            truncated = true;
            break;
        }
        let piece = match block {
            DocBlock::Paragraph(text) => {
                if text.trim().is_empty() {
                    String::new()
                } else {
                    // 段落可能本身含多行（w:br），按原样保留行结构。
                    let mut s = String::new();
                    for line in text.lines() {
                        let l = line.trim_end();
                        if out.len() + s.len() + l.len() + 1 > budget {
                            // 预算内放不下整行：停在整行边界。
                            if !l.is_empty() {
                                truncated = true;
                            }
                            break;
                        }
                        s.push_str(l);
                        s.push('\n');
                    }
                    s
                }
            }
            DocBlock::Table { rows, nested } => {
                let mut s = String::new();
                if *nested && out.len() + 48 <= budget {
                    s.push_str("> （嵌套表格，展平展示）\n");
                }
                for (idx, row) in rows.iter().enumerate() {
                    let cells: Vec<String> = row.iter().map(|c| normalize_cell(c)).collect();
                    let line = format!("| {} |\n", cells.join(" | "));
                    if out.len() + s.len() + line.len() > budget {
                        truncated = true;
                        break;
                    }
                    s.push_str(&line);
                    if idx == 0 {
                        // 首行后插表头分隔行（Markdown 表格语法要求）。
                        let sep = format!("|{}|\n", vec![" --- "; row.len()].join("|"));
                        if out.len() + s.len() + sep.len() > budget {
                            truncated = true;
                            break;
                        }
                        s.push_str(&sep);
                    }
                }
                if !s.is_empty() {
                    s.push('\n');
                }
                s
            }
        };
        if out.len() + piece.len() > budget {
            truncated = true;
            break;
        }
        out.push_str(&piece);
    }
    (out, truncated)
}

// ---------------------------------------------------------------------------
// 入口：read.rs 扩展名分派进来
// ---------------------------------------------------------------------------

/// 执行文档读取（read 工具的文档分派路径）。
pub fn run(ctx: &ToolsContext, args: &DocArgs) -> ToolOutcome {
    // 路径守卫：与 read 文本路径共用 safe_join roots（§9.1）。
    let resolved = match ctx.guard().and_then(|g| g.resolve(&args.path)) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::from(e),
    };
    // 文档本体大小预检（读取前拒绝，票面「超大文件读取前拒并提示」）。
    let meta = match std::fs::metadata(&resolved) {
        Ok(m) => m,
        Err(e) => return ToolOutcome::from(ToolError::from(e)),
    };
    if !meta.is_file() {
        return ToolError::invalid_arg(format!("不是常规文件：{}", args.path)).into();
    }
    if meta.len() > MAX_DOC_FILE_BYTES {
        return ToolError::invalid_arg(format!(
            "文档超过 50MiB 上限：{}（{} 字节）；请压缩或拆分后再试",
            args.path,
            meta.len()
        ))
        .into();
    }
    let bytes = match std::fs::read(&resolved) {
        Ok(b) => b,
        Err(e) => return ToolOutcome::from(ToolError::from(e)),
    };
    match detect_format(&args.path) {
        Some(DocFormat::Docx) => run_docx(&args.path, &bytes),
        Some(DocFormat::Xlsx) => run_xlsx(args, &bytes),
        Some(DocFormat::Pdf) => run_pdf(args, &bytes),
        None => ToolError::invalid_arg(format!(
            "不支持的文档格式：{}（支持 docx/xlsx/xlsm/xlsb/xls/ods/pdf）",
            args.path
        ))
        .into(),
    }
}

// ---------------------------------------------------------------------------
// docx：docx-rs 首选 + zip+quick-xml 降级（#873/#933）
// ---------------------------------------------------------------------------

/// docx 读取主路径（16 号票）：docx-rs `read_docx` → 块序列；失败或零文本
/// 时落降级路径并记 warnings。整体 `catch_unwind` 包裹（解析器对畸形输入
/// 的 panic 防线，与 pdf 同一约束）。
fn run_docx(display: &str, bytes: &[u8]) -> ToolOutcome {
    let mut warnings: Vec<String> = Vec::new();
    let primary =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| docx_rs::read_docx(bytes)));
    let blocks = match primary {
        Ok(Ok(docx)) => {
            let blocks = blocks_from_docx_rs(&docx);
            if blocks_have_no_text(&blocks) {
                // #933 检测：docx-rs「成功」但零文本（非默认命名空间前缀
                // 丢文本的典型形态）→ 降级验证；降级能提出文本才采用。
                match fallback_docx_blocks(bytes) {
                    Ok(fb) if !blocks_have_no_text(&fb) => {
                        warnings.push(
                            "降级解析：docx-rs 未提取到文本（疑似命名空间前缀不兼容，#933），\
                             已用 zip+quick-xml 兜底解析"
                                .to_string(),
                        );
                        fb
                    }
                    _ => blocks, // 降级同样无文本 → 视作真空文档，保留首选结果
                }
            } else {
                blocks
            }
        }
        Ok(Err(e)) => {
            // #873 形态：合法 docx 解 zip 失败 → 降级兜底。
            match fallback_docx_blocks(bytes) {
                Ok(fb) => {
                    warnings.push(format!(
                        "降级解析：docx-rs 读取失败（{e}），已用 zip+quick-xml 兜底解析"
                    ));
                    fb
                }
                Err(fb_err) => {
                    return ToolError::invalid_arg(format!(
                        "docx 解析失败：{e}；兜底解析亦失败：{fb_err}（文档可能损坏或格式不受支持）"
                    ))
                    .into();
                }
            }
        }
        Err(_panic) => {
            // docx-rs 内部 panic：同样走降级（畸形输入的统一防线）。
            match fallback_docx_blocks(bytes) {
                Ok(fb) => {
                    warnings.push(
                        "降级解析：docx-rs 内部异常，已用 zip+quick-xml 兜底解析".to_string(),
                    );
                    fb
                }
                Err(fb_err) => {
                    return ToolError::invalid_arg(format!(
                        "docx 解析失败（文档可能损坏）：兜底解析亦失败：{fb_err}"
                    ))
                    .into();
                }
            }
        }
    };

    let mut paragraphs = 0usize;
    let mut tables = 0usize;
    for b in &blocks {
        match b {
            DocBlock::Paragraph(t) if !t.trim().is_empty() => paragraphs += 1,
            DocBlock::Table { .. } => tables += 1,
            _ => {}
        }
    }
    let (content, truncated) = render_blocks(&blocks, MAX_RESPONSE_BYTES);
    let mut data = json!({
        "kind": "docx",
        "path": display,
        "paragraphCount": paragraphs,
        "tableCount": tables,
        "content": content,
        "truncated": truncated,
    });
    if truncated {
        data["resumeHint"] =
            json!("内容超限已截断（docx 暂无分页参数）；如需其余内容，可请用户拆分文档后重读");
    }
    let mut outcome = ToolOutcome::success(
        data,
        format!(
            "读取 {display}（docx：{paragraphs} 段、{tables} 个表格{}）",
            if truncated { "，内容截断" } else { "" }
        ),
    );
    if !warnings.is_empty() {
        // §8.2：warnings 仅 UI 显示（黄条）不进模型文本。
        outcome = outcome.with_detail(json!({ "warnings": warnings }));
    }
    outcome
}

/// docx-rs 读侧模型 → 块序列（文档体 children 顺序遍历，保持文档流）。
fn blocks_from_docx_rs(docx: &docx_rs::Docx) -> Vec<DocBlock> {
    let mut blocks = Vec::new();
    for child in &docx.document.children {
        match child {
            docx_rs::DocumentChild::Paragraph(p) => {
                blocks.push(DocBlock::Paragraph(paragraph_text_rs(p)));
            }
            docx_rs::DocumentChild::Table(t) => push_table_rs(&mut blocks, t),
            _ => {}
        }
    }
    blocks
}

/// docx-rs 段落 → 文本（Run 内 Text 拼接；Tab/换行保留；超链接内文本一并提取）。
fn paragraph_text_rs(p: &docx_rs::Paragraph) -> String {
    let mut out = String::new();
    collect_paragraph_children_rs(&mut out, &p.children);
    out
}

/// docx-rs 段落子元素集合的文本提取（段落与超链接共用：Hyperlink 的
/// children 也是 ParagraphChild）。
fn collect_paragraph_children_rs(out: &mut String, children: &[docx_rs::ParagraphChild]) {
    for child in children {
        match child {
            docx_rs::ParagraphChild::Run(run) => {
                for rc in &run.children {
                    match rc {
                        docx_rs::RunChild::Text(t) => out.push_str(&t.text),
                        docx_rs::RunChild::Tab(_) => out.push('\t'),
                        docx_rs::RunChild::Break(_) | docx_rs::RunChild::CarriageReturn(_) => {
                            out.push('\n')
                        }
                        _ => {}
                    }
                }
            }
            docx_rs::ParagraphChild::Hyperlink(h) => {
                collect_paragraph_children_rs(out, &h.children);
            }
            _ => {}
        }
    }
}

/// docx-rs 表格 → 表格块（单元格内多段落以换行拼接；嵌套表格展平为独立
/// 块紧跟主表，标 nested=true）。
fn push_table_rs(blocks: &mut Vec<DocBlock>, table: &docx_rs::Table) {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut nested: Vec<DocBlock> = Vec::new();
    for row in &table.rows {
        let docx_rs::TableChild::TableRow(row) = row;
        let mut cells: Vec<String> = Vec::new();
        for cell in &row.cells {
            let docx_rs::TableRowChild::TableCell(cell) = cell;
            let mut parts: Vec<String> = Vec::new();
            for content in &cell.children {
                match content {
                    docx_rs::TableCellContent::Paragraph(p) => {
                        parts.push(paragraph_text_rs(p));
                    }
                    docx_rs::TableCellContent::Table(inner) => {
                        // 嵌套表格：递归展平为独立块（挂在当前表之后输出）。
                        let mut inner_blocks = Vec::new();
                        push_table_rs(&mut inner_blocks, inner);
                        // 首个块是内层主表：标 nested=true（渲染带展平标注）。
                        if let Some(DocBlock::Table { nested, .. }) = inner_blocks.first_mut() {
                            *nested = true;
                        }
                        nested.extend(inner_blocks);
                    }
                    _ => {}
                }
            }
            cells.push(parts.join("\n"));
        }
        rows.push(cells);
    }
    blocks.push(DocBlock::Table {
        rows,
        nested: false,
    });
    blocks.extend(nested);
}

// ---------------------------------------------------------------------------
// docx 降级路径：zip + quick-xml 手解 word/document.xml（按本地名匹配，
// 免疫命名空间前缀差异——#933 的根因）
// ---------------------------------------------------------------------------

/// 降级解析：从 docx（zip 包）取出 `word/document.xml`，quick-xml 流式
/// 解析为块序列。任何一步失败返回错误原文（调用方决定是否采用）。
fn fallback_docx_blocks(bytes: &[u8]) -> Result<Vec<DocBlock>, String> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor).map_err(|e| format!("打开 zip 失败：{e}"))?;
    let mut entry = archive
        .by_name("word/document.xml")
        .map_err(|e| format!("缺少 word/document.xml：{e}"))?;
    // zip 炸弹防线：解压输出按字节上限截断读取（超限报错不静默）。
    let mut xml = String::new();
    let mut limited = (&mut entry).take(MAX_DOCUMENT_XML_BYTES + 1);
    limited
        .read_to_string(&mut xml)
        .map_err(|e| format!("读取 document.xml 失败：{e}"))?;
    if xml.len() as u64 > MAX_DOCUMENT_XML_BYTES {
        return Err("document.xml 超过 64MiB 上限".to_string());
    }
    parse_document_xml(&xml)
}

/// 本地名提取：`w:document` / `ns0:document` / `document` 都取 `document`
/// （降级路径的核心免疫点）。
fn local_name(name: &str) -> &str {
    match name.rfind(':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// quick-xml 流式解析 document.xml 的主体（w:body 直下按文档流输出段落与
/// 表格块）。实现为显式栈的状态机（不用递归下降，嵌套深度由 MAX_TABLE_DEPTH
/// 限制防打爆调用栈）。
// allow(unused_assignments)：flush_paragraph! 置 in_paragraph=false 后、
// Start "p" 分支立即重置 true 的路径上，中间确无读取——状态机语义本就如此。
#[allow(unused_assignments)]
fn parse_document_xml(xml: &str) -> Result<Vec<DocBlock>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    // 表格构建栈：每层表格一个构建器（行 → 单元格 → 单元格内段落文本与
    // 嵌套表）。栈空时段落直接进输出块序列。
    struct CellBuilder {
        /// 单元格内各段落的文本（多段落以 \n 拼接进单元格）。
        paragraphs: Vec<String>,
    }
    struct TableBuilder {
        rows: Vec<Vec<String>>,
        row_cells: Option<Vec<String>>,
        cell: Option<CellBuilder>,
        /// 该表构建期间在内层表格里产生的嵌套表块（表结束时装回外层）。
        finished_nested: Vec<DocBlock>,
    }
    impl TableBuilder {
        fn new() -> Self {
            Self {
                rows: Vec::new(),
                row_cells: None,
                cell: None,
                finished_nested: Vec::new(),
            }
        }
    }

    let mut blocks: Vec<DocBlock> = Vec::new();
    let mut tables: Vec<TableBuilder> = Vec::new();
    // 段落内收集状态：in_paragraph 时文本追加到 current_text。
    let mut in_paragraph = false;
    let mut in_text = false; // <w:t> 内：Text 事件进缓冲
    let mut current_text = String::new();

    /// 把当前段落文本落到正确位置（表格单元格内 / 文档体）。
    macro_rules! flush_paragraph {
        () => {
            if in_paragraph {
                if let Some(cell) = tables.last_mut().and_then(|t| t.cell.as_mut()) {
                    cell.paragraphs.push(std::mem::take(&mut current_text));
                } else {
                    let text = std::mem::take(&mut current_text);
                    blocks.push(DocBlock::Paragraph(text));
                }
                in_paragraph = false;
            }
        };
    }

    let mut buf = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| format!("XML 解析失败：{e}"))?;
        match event {
            Event::Start(e) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                match name {
                    "tbl" => {
                        flush_paragraph!();
                        if tables.len() >= MAX_TABLE_DEPTH {
                            return Err(format!("表格嵌套超过 {MAX_TABLE_DEPTH} 层上限"));
                        }
                        tables.push(TableBuilder::new());
                    }
                    "tr" => {
                        if let Some(t) = tables.last_mut() {
                            t.row_cells = Some(Vec::new());
                        }
                    }
                    "tc" => {
                        if let Some(t) = tables.last_mut() {
                            t.cell = Some(CellBuilder {
                                paragraphs: Vec::new(),
                            });
                        }
                    }
                    "p" => {
                        // 段落可能嵌套在单元格内；重复 <w:p> 容错为重开。
                        flush_paragraph!();
                        in_paragraph = true;
                        current_text.clear();
                    }
                    "t" => {
                        in_text = true;
                    }
                    _ => {}
                }
            }
            Event::Empty(e) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                match name {
                    // 自闭合的 tab/br 也产生空白语义（<w:tab/>、<w:br/>）。
                    "tab" if in_paragraph || in_text => current_text.push('\t'),
                    "br" if in_paragraph || in_text => current_text.push('\n'),
                    _ => {}
                }
            }
            Event::Text(t) => {
                if in_text {
                    // 0.42 的 Text 事件原文是转义形态：into_inner 取原文后
                    // 模块级 unescape 反转义（&amp; 等实体还原）。
                    let piece = quick_xml::escape::unescape(&t.into_inner())
                        .map_err(|e| format!("文本反转义失败：{e}"))?
                        .to_string();
                    current_text.push_str(&piece);
                }
            }
            Event::CData(t) => {
                if in_text {
                    current_text.push_str(&t.into_inner());
                }
            }
            Event::End(e) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                match name {
                    "t" => in_text = false,
                    "p" => flush_paragraph!(),
                    "tc" => {
                        if let Some(t) = tables.last_mut()
                            && let Some(cell) = t.cell.take()
                        {
                            let text = cell.paragraphs.join("\n");
                            if let Some(cells) = t.row_cells.as_mut() {
                                cells.push(text);
                            }
                        }
                    }
                    "tr" => {
                        if let Some(t) = tables.last_mut()
                            && let Some(cells) = t.row_cells.take()
                        {
                            t.rows.push(cells);
                        }
                    }
                    "tbl" => {
                        // 表结束：弹栈；外层是表格则挂嵌套表，否则进输出块。
                        if let Some(done) = tables.pop() {
                            let block = DocBlock::Table {
                                rows: done.rows,
                                nested: false,
                            };
                            if let Some(outer) = tables.last_mut() {
                                outer.finished_nested.push(block);
                                outer.finished_nested.extend(done.finished_nested);
                            } else {
                                blocks.push(block);
                                blocks.extend(done.finished_nested);
                            }
                        }
                    }
                    "body" | "document" => {
                        // 主体结束：后面只剩结束标签，直接收工。
                        flush_paragraph!();
                        break;
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    flush_paragraph!();
    Ok(blocks)
}

// ---------------------------------------------------------------------------
// xlsx：calamine 逐 sheet → Markdown 表格
// ---------------------------------------------------------------------------

/// xlsx 读取主路径（16 号票）：calamine `worksheet_range` 逐 sheet 转
/// Markdown 表格。缺省（无 sheet 参数）输出 sheet 名清单 + 首 sheet 前
/// [`XLSX_DEFAULT_PREVIEW_ROWS`] 行；给 sheet 参数时输出该表全量（预算内）。
/// 公式单元格取文件内缓存值（worksheet_range 给的就是缓存值；公式文本
/// v1 不输出，`worksheet_formula` 备而不用）。
fn run_xlsx(args: &DocArgs, bytes: &[u8]) -> ToolOutcome {
    // trait 导入在函数内（与 quick_xml::Reader 类型名区分）。
    use calamine::Reader as _;
    // 整体 catch_unwind：calamine 对畸形文件的 panic 防线（与 pdf 同约束）。
    // 闭包错误统一为 String（E_INVALID_ARG 的 message）；catch_unwind 外层
    // Result 为 panic 分支。
    let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut wb = calamine::open_workbook_auto_from_rs(Cursor::new(bytes))
            .map_err(|e| format!("工作簿打开失败（文件可能损坏）：{e}"))?;
        let sheet_names = wb.sheet_names();
        if sheet_names.is_empty() {
            return Err(String::from("工作簿不含任何 sheet（文件可能损坏）"));
        }
        // 选择要输出的 sheet（缺省首 sheet 预览模式）。
        let (selected, preview_only) = match &args.sheet {
            Some(name) => {
                if !sheet_names.contains(name) {
                    return Err(format!(
                        "sheet「{name}」不存在；可用 sheet：{}",
                        sheet_names.join("、")
                    ));
                }
                (vec![name.clone()], false)
            }
            None => (vec![sheet_names[0].clone()], true),
        };

        let mut sections = Vec::new();
        let mut truncated = false;
        let mut total_rows = 0usize;
        for name in &selected {
            let range = wb
                .worksheet_range(name)
                .map_err(|e| format!("读取 sheet「{name}」失败：{e}"))?;
            if range.is_empty() {
                sections.push(format!("## {name}\n（空 sheet）\n"));
                continue;
            }
            let max_rows = if preview_only {
                XLSX_DEFAULT_PREVIEW_ROWS
            } else {
                usize::MAX
            };
            let (table_md, rows_taken, sheet_truncated) =
                render_range_rows(&range, max_rows, MAX_RESPONSE_BYTES);
            total_rows += rows_taken;
            truncated = truncated || sheet_truncated;
            sections.push(format!("## {name}\n{table_md}"));
        }
        Ok((
            sheet_names,
            selected,
            preview_only,
            sections.join("\n"),
            total_rows,
            truncated,
        ))
    }));
    let (sheet_names, selected, preview_only, body, total_rows, mut truncated) = match parsed {
        Ok(Ok(v)) => v,
        Ok(Err(msg)) => {
            return ToolError::invalid_arg(msg).into();
        }
        Err(_panic) => {
            return ToolError::invalid_arg("xlsx 解析失败（文件可能损坏或加密）：内部解析异常")
                .into();
        }
    };

    let mut content = body;
    let mut resume_hint: Option<String> = None;
    if preview_only {
        // 缺省预览：列出全部 sheet 名供模型选表续读（§8.2：模型据此行动的
        // 结论写入 data）。
        let list = sheet_names
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n");
        content.push_str(&format!(
            "\n全部 sheet（{} 个）：\n{list}\n（缺省仅预览首 sheet 前 {} 行；用 sheet 参数选表读全量）\n",
            sheet_names.len(),
            XLSX_DEFAULT_PREVIEW_ROWS
        ));
        if sheet_names.len() > 1 {
            truncated = true;
        }
    }
    if truncated {
        resume_hint = Some(match &args.sheet {
            Some(name) => format!(
                "内容超限已截断；当前 sheet「{name}」暂无行偏移参数，可请用户拆分表格后重读"
            ),
            None => format!(
                "内容超限已截断；用 sheet 参数选表续读（可用：{}）",
                sheet_names.join("、")
            ),
        });
    }

    let sheet_total = sheet_names.len();
    let mut data = json!({
        "kind": "xlsx",
        "path": args.path,
        "sheets": sheet_names,
        "selectedSheets": selected,
        "sheetCount": sheet_total,
        "rowCount": total_rows,
        "content": content,
        "truncated": truncated,
    });
    if let Some(hint) = resume_hint {
        data["resumeHint"] = json!(hint);
    }
    let scope = match &args.sheet {
        Some(name) => name.clone(),
        None => format!(
            "首 sheet「{}」前 {} 行",
            selected[0], XLSX_DEFAULT_PREVIEW_ROWS
        ),
    };
    ToolOutcome::success(
        data,
        format!(
            "读取 {}（xlsx：{sheet_total} 个 sheet，输出 {scope}{}）",
            args.path,
            if truncated { "，内容截断" } else { "" }
        ),
    )
}

/// calamine Range → Markdown 表格文本（逐行渲染，`max_rows` 限制行数，
/// `budget` 限制字节）。返回（表格文本, 实际输出行数, 是否截断）。
fn render_range_rows(
    range: &calamine::Range<calamine::Data>,
    max_rows: usize,
    budget: usize,
) -> (String, usize, bool) {
    let mut out = String::new();
    let mut taken = 0usize;
    let mut truncated = false;
    let width = range.width().max(1);
    for (idx, row) in range.rows().enumerate() {
        if idx >= max_rows || out.len() >= budget {
            truncated = true;
            break;
        }
        let cells: Vec<String> = (0..width)
            .map(|c| row.get(c).map(data_to_string).unwrap_or_default())
            .collect();
        let line = format!("| {} |\n", cells.join(" | "));
        if out.len() + line.len() > budget {
            truncated = true;
            break;
        }
        out.push_str(&line);
        if idx == 0 {
            let sep = format!("|{}|\n", vec![" --- "; width].join("|"));
            if out.len() + sep.len() > budget {
                truncated = true;
                break;
            }
            out.push_str(&sep);
        }
        taken += 1;
    }
    (out, taken, truncated)
}

/// calamine 单元格值 → 字符串（日期时间走 Display；错误值带 #ERR 前缀）。
fn data_to_string(d: &calamine::Data) -> String {
    match d {
        calamine::Data::Empty => String::new(),
        calamine::Data::Int(i) => i.to_string(),
        calamine::Data::Float(f) => {
            // Display 即最短往返表示（1.5 → "1.5"，不输出多余的 0）。
            f.to_string()
        }
        calamine::Data::String(s) => s.clone(),
        calamine::Data::Bool(b) => b.to_string(),
        calamine::Data::DateTime(dt) => dt.to_string(),
        calamine::Data::DateTimeIso(s) | calamine::Data::DurationIso(s) => s.clone(),
        calamine::Data::Error(e) => format!("#ERR({e})"),
    }
}

// ---------------------------------------------------------------------------
// pdf：pdf-extract 按页线性文本 + catch_unwind + 页码门槛
// ---------------------------------------------------------------------------

/// pdf 读取主路径（16 号票）：pdf-extract 按页提取线性文本（v1 无表格重建）。
/// `catch_unwind` 包裹（#97/#141 对不可信输入可能 panic）；输出头部标注
/// 「提取可能不完整」（部分中文 PDF CMap 限制 #103）；>10 页须显式页码、
/// 单次窗口 ≤20 页（§8.3/§8.9）。
fn run_pdf(args: &DocArgs, bytes: &[u8]) -> ToolOutcome {
    // 加密检测（票面「加密/损坏文件 E_INVALID_ARG 明确话术」）：trailer 带
    // /Encrypt 即加密文档——即便解析器可能解出内容，也按不支持处理（口令
    // 不在工具参数面，弱加密解出的内容亦不可信）。检测本身失败不拦路
    //（损坏文件的错误由下方解析路径统一给）。
    if let Ok(doc) = lopdf::Document::load_mem(bytes)
        && doc.trailer.get(b"Encrypt").is_ok()
    {
        return ToolError::invalid_arg("文档已加密，暂不支持读取加密 PDF；请先解密后再试").into();
    }
    let pages: Vec<String> = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem_by_pages(bytes)
    })) {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => {
            return ToolError::invalid_arg(format!("文档解析失败（可能损坏或加密）：{e}")).into();
        }
        Err(_panic) => {
            return ToolError::invalid_arg("文档解析失败（可能损坏或加密）：内部解析异常").into();
        }
    };
    if pages.is_empty() {
        return ToolError::invalid_arg(
            "未从 PDF 提取到任何页（可能为纯扫描件或损坏）：文本层为空，OCR 不在支持范围",
        )
        .into();
    }
    let page_count = pages.len() as u32;

    // 页码解析（§8.3 read 行：page?/pages?）。窗口越界部分自然裁剪（与
    // read 文本 offset 超界行为一致），from ≥1 强制。
    let (start, end) = match (args.page, args.pages) {
        (Some(_), Some(_)) => {
            return ToolError::invalid_arg("page 与 pages 只能二选一").into();
        }
        (Some(p), None) => (p, p),
        (None, Some((f, t))) => (f, t),
        (None, None) => {
            if page_count > PDF_EXPLICIT_PAGE_THRESHOLD as u32 {
                return ToolError::invalid_arg(format!(
                    "PDF 共 {page_count} 页（>10 页须显式页码）：用 page 读单页，或 pages:[起, 止] 读窗口（单次 ≤{PDF_MAX_PAGE_WINDOW} 页）"
                ))
                .into();
            }
            (1, page_count)
        }
    };
    if start < 1 {
        return ToolError::invalid_arg(format!("页码从 1 开始（实际 {start}）")).into();
    }
    let start = start.min(page_count + 1);
    let end = end.min(page_count);
    if end < start {
        // from > pageCount：与 read 文本 offset 超界一致，返回空窗口。
        let data = json!({
            "kind": "pdf",
            "path": args.path,
            "pageCount": page_count,
            "pages": [start, end],
            "content": "",
            "truncated": false,
        });
        return ToolOutcome::success(
            data,
            format!(
                "读取 {}（pdf：共 {page_count} 页，页码超出范围，空内容）",
                args.path
            ),
        );
    }
    if end - start + 1 > PDF_MAX_PAGE_WINDOW {
        return ToolError::invalid_arg(format!(
            "单次页码窗口 ≤{} 页（实际 {} 页）：缩小 pages 范围分批读取",
            PDF_MAX_PAGE_WINDOW,
            end - start + 1
        ))
        .into();
    }

    // 窗口切片拼线性文本（页间以分页标记分隔）。
    let mut body = String::new();
    for p in start..=end {
        let text = pages[(p - 1) as usize].trim_end();
        body.push_str(&format!("--- 第 {p} 页 ---\n{text}\n"));
    }
    // 头部标注（#103 口径：票面「输出头部标注提取可能不完整」）。
    let header = "（PDF 线性文本提取：表格无法结构化重建；部分中文 PDF 因 CMap 映射限制可能乱码或缺字，提取可能不完整）\n\n".to_string();
    let full = format!("{header}{body}");
    // 512KB 预算按整行边界截断（含页标记行）。
    let (content, truncated) = truncate_at_line_boundary(&full, MAX_RESPONSE_BYTES);
    let mut data = json!({
        "kind": "pdf",
        "path": args.path,
        "pageCount": page_count,
        "pages": [start, end],
        "content": content,
        "truncated": truncated,
    });
    if truncated {
        data["resumeHint"] = json!(format!(
            "内容超限已截断；用 page 或 pages 参数从第 {} 页继续（单次 ≤{} 页）",
            end.saturating_sub(PDF_MAX_PAGE_WINDOW) + 1,
            PDF_MAX_PAGE_WINDOW
        ));
    }
    ToolOutcome::success(
        data,
        format!(
            "读取 {}（pdf：共 {page_count} 页，输出第 {start}–{end} 页{}）",
            args.path,
            if truncated { "，内容截断" } else { "" }
        ),
    )
}

/// 通用整行边界截断（`budget` 字节内保留完整行；与 read 文本路径同语义，
/// 供 pdf 线性文本用——docx/xlsx 的截断在块渲染内做）。
fn truncate_at_line_boundary(text: &str, budget: usize) -> (String, bool) {
    if text.len() <= budget {
        return (text.to_string(), false);
    }
    let mut taken = 0usize;
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        if taken + line.len() > budget {
            return (out, true);
        }
        out.push_str(line);
        taken += line.len();
    }
    (out, true)
}

// ---------------------------------------------------------------------------
// 规格（read 工具本体，见 read.rs spec；本模块只提供参数结构与执行）
// ---------------------------------------------------------------------------

/// read 工具的文档参数 schema 片段（read.rs spec() 的 params_schema 引用，
/// 前端 toolkit.ts 逐字镜像——contract.test 双向锁定）。
pub fn params_schema_properties() -> serde_json::Value {
    json!({
        "page": { "type": "integer", "minimum": 1, "description": "（pdf）单页页码（1-based）；与 pages 二选一" },
        "pages": {
            "type": "array",
            "items": { "type": "integer", "minimum": 1 },
            "minItems": 2,
            "maxItems": 2,
            "description": "（pdf）页码窗口 [起, 止]（1-based 闭区间，单次 ≤20 页）；>10 页的 PDF 必须给 page 或 pages"
        },
        "sheet": { "type": "string", "description": "（xlsx）要读取的 sheet 名；缺省输出 sheet 名清单 + 首 sheet 前 100 行" }
    })
}

#[cfg(test)]
#[path = "doc_test.rs"]
mod doc_test;
