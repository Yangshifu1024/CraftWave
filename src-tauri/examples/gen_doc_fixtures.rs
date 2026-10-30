//! 测试样本生成器（16 号票，一次性工具）：生成入库到
//! `tests/fixtures/docs/` 的文档样本——真实生成器（docx-rs 写侧）与
//! 手工构造（zip+XML 最小 xlsx、手写字节 PDF）混合，保证可复现。
//!
//! 用法：`cargo run --example gen_doc_fixtures`（在 src-tauri/ 下执行）。
//! 样本一经入库即是回归基线（CI 跑的就是入库字节），本文件仅用于重新生成。
//!
//! 中文 PDF 与加密 PDF 由 `gen_pdf_fixtures.swift`（macOS CoreGraphics，
//! 系统文本引擎导出）生成——纯手写 PDF 无法内置中文字体与加密字典。

use std::fs::File;
use std::io::Write;
use std::path::Path;

use docx_rs::{Docx, Paragraph, Run, Table, TableCell, TableRow};
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

/// 样本输出目录（src-tauri/tests/fixtures/docs/）。
const OUT_DIR: &str = "tests/fixtures/docs";

fn main() {
    std::fs::create_dir_all(OUT_DIR).expect("建样本目录");
    // 真实写侧样本：docx-rs 生成（与库自身往返兼容的「标准形态」）。
    gen_writer_docx(&format!("{OUT_DIR}/sample_zh_writer.docx"));
    gen_nested_table_docx(&format!("{OUT_DIR}/sample_nested_table.docx"));
    // 手工构造样本：#933 命名空间前缀形态、最小 xlsx、最小 PDF、损坏 PDF。
    gen_ns_prefix_docx(&format!("{OUT_DIR}/sample_ns_prefix.docx"));
    gen_multisheet_xlsx(&format!("{OUT_DIR}/sample_multisheet.xlsx"));
    gen_pages_pdf(&format!("{OUT_DIR}/sample_25pages.pdf"), 25);
    gen_corrupted_pdf(&format!("{OUT_DIR}/corrupted.pdf"));
    println!("样本生成完毕：{OUT_DIR}/（中文 PDF 与加密 PDF 请另行运行 gen_pdf_fixtures.swift）");
}

// ---------------------------------------------------------------------------
// docx-rs 写侧：真实 docx（首选路径样本）
// ---------------------------------------------------------------------------

/// 中文段落 + 表格（docx-rs 写侧生成，docx 首选路径的正样本）。
fn gen_writer_docx(path: &str) {
    // 0.4.22 写侧：Table::new / TableRow::new 接收 Vec 参数。
    let table = Table::new(vec![
        TableRow::new(vec![
            TableCell::new().add_paragraph(paragraph("项目")),
            TableCell::new().add_paragraph(paragraph("进度")),
            TableCell::new().add_paragraph(paragraph("负责人")),
        ]),
        TableRow::new(vec![
            TableCell::new().add_paragraph(paragraph("后端解析")),
            TableCell::new().add_paragraph(paragraph("已完成")),
            TableCell::new().add_paragraph(paragraph("张三")),
        ]),
        TableRow::new(vec![
            TableCell::new().add_paragraph(paragraph("前端卡片")),
            TableCell::new().add_paragraph(paragraph("进行中")),
            TableCell::new().add_paragraph(paragraph("李四")),
        ]),
    ]);
    let docx = Docx::new()
        .add_paragraph(paragraph("项目周报"))
        .add_paragraph(paragraph("本周完成文档读取管线的解析与测试，含中文回归。"))
        .add_paragraph(paragraph("下表为任务进度明细："))
        .add_table(table)
        .add_paragraph(paragraph("以上。"));
    write_docx(path, docx);
}

/// 嵌套表格样本（docx-rs 写侧 TableCell::add_table，验证嵌套展平标注）。
fn gen_nested_table_docx(path: &str) {
    let inner = Table::new(vec![
        TableRow::new(vec![
            TableCell::new().add_paragraph(paragraph("内层甲")),
            TableCell::new().add_paragraph(paragraph("内层乙")),
        ]),
        TableRow::new(vec![
            TableCell::new().add_paragraph(paragraph("内层丙")),
            TableCell::new().add_paragraph(paragraph("内层丁")),
        ]),
    ]);
    let outer = Table::new(vec![TableRow::new(vec![
        TableCell::new().add_paragraph(paragraph("外层单元格一")),
        TableCell::new()
            .add_paragraph(paragraph("单元格内含嵌套表："))
            .add_table(inner),
    ])]);
    let docx = Docx::new()
        .add_paragraph(paragraph("嵌套表格测试文档"))
        .add_table(outer);
    write_docx(path, docx);
}

/// 构造一个居中不重要、仅含文本的段落（生成器内部辅助）。
fn paragraph(text: &str) -> Paragraph {
    Paragraph::new().add_run(Run::new().add_text(text))
}

/// 写侧 Docx → 打包为 docx 文件。
fn write_docx(path: &str, docx: Docx) {
    let file = File::create(path).expect("建样本文件");
    docx.build().pack(file).expect("打包 docx");
}

// ---------------------------------------------------------------------------
// 手工构造：#933 形态（非默认命名空间前缀）的最小 docx
// ---------------------------------------------------------------------------

/// 最小 docx（zip 三件套），document.xml 全部用 `ns0:` 命名空间前缀——
/// docx-rs #933 的触发形态（其读侧按 `w:` 前缀严格匹配，非默认前缀丢文本）。
/// 预期：首选路径提不出文本 → 降级路径按本地名解析出全部文本。
fn gen_ns_prefix_docx(path: &str) {
    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;
    // 全部元素与属性用 ns0: 前缀（文本内容为中文）。
    let document = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<ns0:document xmlns:ns0="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<ns0:body>
<ns0:p><ns0:r><ns0:t>命名空间前缀测试</ns0:t></ns0:r></ns0:p>
<ns0:p><ns0:r><ns0:t>第二段：中文文本应完整保留</ns0:t></ns0:r></ns0:p>
<ns0:tbl>
<ns0:tr><ns0:tc><ns0:p><ns0:r><ns0:t>甲</ns0:t></ns0:r></ns0:p></ns0:tc><ns0:tc><ns0:p><ns0:r><ns0:t>乙</ns0:t></ns0:r></ns0:p></ns0:tc></ns0:tr>
<ns0:tr><ns0:tc><ns0:p><ns0:r><ns0:t>一</ns0:t></ns0:r></ns0:p></ns0:tc><ns0:tc><ns0:p><ns0:r><ns0:t>二</ns0:t></ns0:r></ns0:p></ns0:tc></ns0:tr>
</ns0:tbl>
</ns0:body>
</ns0:document>"#;
    write_zip(
        path,
        &[
            ("[Content_Types].xml", content_types),
            ("_rels/.rels", rels),
            ("word/document.xml", document),
        ],
    );
}

// ---------------------------------------------------------------------------
// 手工构造：最小 xlsx（3 sheet，含公式缓存值与中文 inlineStr）
// ---------------------------------------------------------------------------

/// 最小多 sheet xlsx：inlineStr 中文单元格（免共享字符串表）+ SUM 公式
/// 带缓存值（验证「公式单元格取缓存值」）。
fn gen_multisheet_xlsx(path: &str) {
    let content_types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
<Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
<Override PartName="/xl/worksheets/sheet3.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#;
    let workbook = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets>
<sheet name="销售数据" sheetId="1" r:id="rId1"/>
<sheet name="汇总" sheetId="2" r:id="rId2"/>
<sheet name="说明" sheetId="3" r:id="rId3"/>
</sheets>
</workbook>"#;
    let workbook_rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet2.xml"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet3.xml"/>
</Relationships>"#;
    // sheet1：表头 + 两行数据 + SUM 公式（缓存值 130）。
    let sheet1 = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData>
<row r="1"><c r="A1" t="inlineStr"><is><t>产品</t></is></c><c r="B1" t="inlineStr"><is><t>销售额</t></is></c></row>
<row r="2"><c r="A2" t="inlineStr"><is><t>Keyboard 键盘</t></is></c><c r="B2"><v>50</v></c></row>
<row r="3"><c r="A3" t="inlineStr"><is><t>Mouse 鼠标</t></is></c><c r="B3"><v>80</v></c></row>
<row r="4"><c r="A4" t="inlineStr"><is><t>合计</t></is></c><c r="B4"><f>SUM(B2:B3)</f><v>130</v></c></row>
</sheetData>
</worksheet>"#;
    let sheet2 = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData>
<row r="1"><c r="A1" t="inlineStr"><is><t>指标</t></is></c><c r="B1" t="inlineStr"><is><t>数值</t></is></c></row>
<row r="2"><c r="A2" t="inlineStr"><is><t>达成率</t></is></c><c r="B2"><v>0.92</v></c></row>
</sheetData>
</worksheet>"#;
    let sheet3 = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData>
<row r="1"><c r="A1" t="inlineStr"><is><t>数据截至 2026-10-30，仅供测试使用</t></is></c></row>
</sheetData>
</worksheet>"#;
    write_zip(
        path,
        &[
            ("[Content_Types].xml", content_types),
            ("_rels/.rels", rels),
            ("xl/workbook.xml", workbook),
            ("xl/_rels/workbook.xml.rels", workbook_rels),
            ("xl/worksheets/sheet1.xml", sheet1),
            ("xl/worksheets/sheet2.xml", sheet2),
            ("xl/worksheets/sheet3.xml", sheet3),
        ],
    );
}

/// 把若干 (归档名, 文本内容) 打包成 zip 文件（生成器内部辅助）。
fn write_zip(path: &str, entries: &[(&str, &str)]) {
    let file = File::create(path).expect("建样本文件");
    let mut zip = ZipWriter::new(file);
    for (name, content) in entries {
        zip.start_file(*name, SimpleFileOptions::default())
            .expect("建 zip 条目");
        zip.write_all(content.as_bytes()).expect("写 zip 条目");
    }
    zip.finish().expect("收尾 zip");
}

// ---------------------------------------------------------------------------
// 手工构造：最小多页 PDF（英文，分页门槛与窗口测试用）与损坏 PDF
// ---------------------------------------------------------------------------

/// 手写字节多页英文文本 PDF（Helvetica 标准字体零依赖）：页数传入以便
/// 生成超过页码窗口上限（20 页）的样本——「>10 页须显式页码」门槛与
/// 「单次窗口 ≤20 页」上限共用本样本。
fn gen_pages_pdf(path: &str, page_count: usize) {
    let mut objects: Vec<(usize, Vec<u8>)> = Vec::new();
    // 对象 1：Catalog；对象 2：Pages；字体对象恒为 3 + page_count。
    // 每页两个对象：页对象（3 + 2*i）、内容流对象（4 + 2*i）。
    let font_obj = 3 + 2 * page_count;
    for i in 0..page_count {
        let page_num = i + 1;
        let content = format!("BT /F1 24 Tf 72 720 Td (Page {page_num} of {page_count}) Tj ET\n");
        let page_obj = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R /Resources << /Font << /F1 {font_obj} 0 R >> >> >>",
            4 + 2 * i
        );
        objects.push((3 + 2 * i, page_obj.into_bytes()));
        objects.push((
            4 + 2 * i,
            format!(
                "<< /Length {} >>\nstream\n{}endstream",
                content.len(),
                content
            )
            .into_bytes(),
        ));
    }
    let kids = (0..page_count)
        .map(|i| format!("{} 0 R", 3 + 2 * i))
        .collect::<Vec<_>>()
        .join(" ");
    objects.push((
        1,
        "<< /Type /Catalog /Pages 2 0 R >>".to_string().into_bytes(),
    ));
    objects.push((
        2,
        format!("<< /Type /Pages /Kids [{kids}] /Count {page_count} >>").into_bytes(),
    ));
    objects.push((
        font_obj,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ));

    let mut pdf: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut offsets: Vec<(usize, usize)> = Vec::new();
    let max_obj = font_obj;
    for (num, body) in &objects {
        offsets.push((*num, pdf.len()));
        pdf.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref_pos = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", max_obj + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    let mut table = vec![0usize; max_obj + 1];
    for (num, off) in &offsets {
        table[*num] = *off;
    }
    for off in &table[1..] {
        pdf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            max_obj + 1,
            xref_pos
        )
        .as_bytes(),
    );
    std::fs::write(path, pdf).expect("写 PDF 样本");
}

/// 损坏 PDF：PDF 头后跟无结构的乱字节（畸形不可信输入样本——
/// 解析必须被边界兜住：报结构化错误，绝不能把 panic 漏出去）。
fn gen_corrupted_pdf(path: &str) {
    let mut bytes = b"%PDF-1.4\n".to_vec();
    bytes.extend_from_slice(b"\x00\x01\x02\x03 broken obj <<<< endobj\ntrailer\n%%EOF\n");
    bytes.extend_from_slice(&[0xFF, 0xFE, 0x00, 0x17, 0x42, 0x00]);
    let _ = Path::new(path);
    std::fs::write(path, bytes).expect("写损坏 PDF 样本");
}
