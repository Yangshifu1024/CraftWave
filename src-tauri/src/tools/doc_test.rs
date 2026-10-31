//! 文档读取管线测试（16 号票，§8.9 读侧）：三类格式端到端中文解析、
//! Markdown 表格结构化、docx 降级路径（#873/#933）、畸形/加密 PDF 的
//! catch_unwind 兜底、页码门槛与窗口上限、512KB 截断与续读指引、
//! 大小/路径守卫，以及 read 工具的扩展名分派。矩阵化合并为
//! docx / xlsx / pdf / 截断守卫与分派四个用例。
//!
//! 样本全部来自入库的 `tests/fixtures/docs/`（生成方式见
//! `examples/gen_doc_fixtures.rs` 与 `examples/gen_pdf_fixtures.swift`
//! 头注）；每个用例把样本复制进临时工作区再读——路径守卫要求目标在
//! roots 内，这也顺带验证了守卫放行路径。

use std::fs::File;
use std::io::Write as _;

use crate::tools::doc::{DocArgs, DocFormat, detect_format, run as run_doc};
use crate::tools::read::{ReadArgs, ReadKind, run as run_read};
use crate::util::test_support::TestWorkspace;

/// 入库样本的绝对路径（单元测试 cwd 为 crate 根，用 CARGO_MANIFEST_DIR 最稳）。
fn fixture_src(name: &str) -> String {
    format!("{}/tests/fixtures/docs/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// 把入库样本复制进临时工作区，返回工作区内相对路径。
fn copy_fixture(ws: &TestWorkspace, name: &str) -> String {
    let dest = ws.project_file(name);
    std::fs::copy(fixture_src(name), &dest).expect("复制样本进工作区");
    name.to_string()
}

/// 构造文档读取参数（无页码 / 无 sheet）。
fn doc_args(path: &str) -> DocArgs {
    DocArgs {
        path: path.to_string(),
        page: None,
        pages: None,
        sheet: None,
    }
}

/// 构造 read 参数（文本类型，无窗口）。
fn read_args(path: &str) -> ReadArgs {
    ReadArgs {
        path: path.to_string(),
        offset: None,
        limit: None,
        kind: None,
        page: None,
        pages: None,
        sheet: None,
    }
}

/// serde_json::value::RawValue 之外布尔断言的简写（保持用例可读）。
fn json_bool(v: bool) -> serde_json::Value {
    serde_json::json!(v)
}

// 断言：docx 解析矩阵（验收①②，#933/#873）——中文 writer 样本端到端
//（段落中文 + 表格结构化 Markdown + 计数）、真实世界导出样本集（macOS
// textutil/Cocoa 导出）逐一解析且中文不乱码、嵌套表格展平带标注、
// 非默认命名空间前缀触发降级路径（warnings 只进 detail 不进 data）、
// 损坏 zip 给结构化错误不 panic、kind:image 对文档扩展名误用拒绝。
#[test]
fn docx_parsing_matrix() {
    // 验收①：中文 docx 端到端解析。
    let ws = TestWorkspace::new();
    let path = copy_fixture(&ws, "sample_zh_writer.docx");
    let outcome = run_doc(&ws.ctx(), &doc_args(&path));
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["kind"], "docx");
    assert_eq!(data["truncated"], json_bool(false));
    let content = data["content"].as_str().expect("content");
    assert!(content.contains("项目周报"), "段落标题缺失：{content}");
    assert!(
        content.contains("本周完成文档读取管线的解析与测试"),
        "段落正文缺失"
    );
    // 表格结构化：表头行 + 分隔行 + 数据行。
    assert!(
        content.contains("| 项目 | 进度 | 负责人 |"),
        "表头行缺失：{content}"
    );
    assert!(
        content.contains("| --- | --- | --- |"),
        "Markdown 分隔行缺失"
    );
    assert!(content.contains("| 张三 |"), "表格数据缺失");
    assert_eq!(data["tableCount"], 1);
    assert_eq!(data["paragraphCount"], 4);
    assert!(outcome.ui_summary.contains("docx"));

    // 验收①（15 号票回归要求）：真实世界导出样本集逐一解析成功。
    let ws = TestWorkspace::new();
    for name in [
        "export_report.docx",
        "export_minutes.docx",
        "export_memo.docx",
    ] {
        let path = copy_fixture(&ws, name);
        let outcome = run_doc(&ws.ctx(), &doc_args(&path));
        assert!(outcome.ok, "{name} 解析失败：{outcome:?}");
        let content = outcome.data.expect("data")["content"]
            .as_str()
            .expect("content")
            .to_string();
        assert!(!content.trim().is_empty(), "{name} 内容为空");
        if name == "export_report.docx" {
            assert!(
                content.contains("季度评审报告"),
                "中文标题乱码或缺失：{content}"
            );
            assert!(
                content.contains("解析成功率"),
                "表格单元格文本缺失：{content}"
            );
        }
        if name == "export_minutes.docx" {
            assert!(
                content.contains("项目例会纪要"),
                "中文标题乱码或缺失：{content}"
            );
        }
    }

    // 验收①：嵌套表格展平为独立 Markdown 表并带标注行。
    let ws = TestWorkspace::new();
    let path = copy_fixture(&ws, "sample_nested_table.docx");
    let outcome = run_doc(&ws.ctx(), &doc_args(&path));
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    let content = data["content"].as_str().expect("content");
    assert!(
        content.contains("外层单元格一"),
        "外层单元格缺失：{content}"
    );
    assert!(
        content.contains("内层甲") && content.contains("内层丁"),
        "内层数据缺失"
    );
    assert!(
        content.contains("嵌套表格，展平展示"),
        "嵌套标注缺失：{content}"
    );
    assert_eq!(data["tableCount"], 2);

    // 验收②：#933 形态（非默认命名空间前缀）触发降级路径，warnings 仅进
    // detail 不进 data（§8.2）。
    let ws = TestWorkspace::new();
    let path = copy_fixture(&ws, "sample_ns_prefix.docx");
    let outcome = run_doc(&ws.ctx(), &doc_args(&path));
    assert!(outcome.ok, "#933 形态应经降级路径成功解析：{outcome:?}");
    let data = outcome.data.expect("data");
    let content = data["content"].as_str().expect("content");
    assert!(
        content.contains("命名空间前缀测试"),
        "降级文本缺失：{content}"
    );
    assert!(content.contains("| 甲 | 乙 |"), "降级表格缺失：{content}");
    assert!(content.contains("| 一 | 二 |"), "降级表格数据缺失");
    let detail = outcome.detail.expect("detail");
    let warnings = detail["warnings"].as_array().expect("warnings 数组");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains("降级解析")),
        "warnings 缺降级标注：{warnings:?}"
    );
    assert!(data.get("warnings").is_none(), "warnings 泄入 data");

    // 验收②（#873 形态）：解 zip 失败的损坏 docx 给结构化错误，不 panic。
    let ws = TestWorkspace::new();
    let path = ws.project_file("broken.docx");
    let mut f = File::create(&path).expect("建文件");
    f.write_all(b"PK\x03\x04 not-a-zip-stream \x00\x01\x02")
        .expect("写乱字节");
    drop(f);
    let outcome = run_doc(&ws.ctx(), &doc_args("broken.docx"));
    assert!(!outcome.ok, "损坏 docx 不应成功");
    let err = outcome.error.expect("error");
    assert_eq!(err.code, "E_INVALID_ARG");
    assert!(
        err.message.contains("docx 解析失败"),
        "错误话术缺失：{err:?}"
    );

    // kind:"image" 对文档扩展名是误用，明确拒绝（read 分派层）。
    let ws = TestWorkspace::new();
    let path = copy_fixture(&ws, "sample_zh_writer.docx");
    let mut args = read_args(&path);
    args.kind = Some(ReadKind::Image);
    let outcome = run_read(&ws.ctx(), &args);
    assert!(!outcome.ok);
    assert_eq!(outcome.error.expect("e").code, "E_INVALID_ARG");
}

// 断言：xlsx 读取矩阵——缺省读取输出首 sheet 结构化 Markdown（含中文与
// 公式缓存值）与全部 sheet 名清单 + 续读指引；sheet 参数选表输出该表全量
// 且不混入其他 sheet；不存在的 sheet 名拒绝并带可用清单。
#[test]
fn xlsx_sheet_matrix() {
    // 缺省读取：首 sheet 预览 + sheet 清单 + 公式缓存值。
    let ws = TestWorkspace::new();
    let path = copy_fixture(&ws, "sample_multisheet.xlsx");
    let outcome = run_doc(&ws.ctx(), &doc_args(&path));
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["kind"], "xlsx");
    let sheets = data["sheets"].as_array().expect("sheets");
    assert_eq!(sheets.len(), 3);
    assert_eq!(sheets[0], "销售数据");
    let content = data["content"].as_str().expect("content");
    assert!(
        content.contains("## 销售数据"),
        "首 sheet 标题缺失：{content}"
    );
    assert!(content.contains("| 产品 | 销售额 |"), "表头缺失：{content}");
    assert!(content.contains("| Keyboard 键盘 | 50 |"), "中文行缺失");
    // 公式单元格取缓存值 130（SUM(B2:B3) 的 <v>），而非公式文本。
    assert!(content.contains("| 130 |"), "公式缓存值缺失：{content}");
    assert!(!content.contains("SUM("), "不应输出公式文本：{content}");
    // 缺省预览：sheet 清单 + 续读指引（多 sheet 时 truncated=true）。
    assert!(content.contains("全部 sheet"), "sheet 清单缺失：{content}");
    assert!(content.contains("用 sheet 参数选表读全量"), "续读指引缺失");
    assert_eq!(data["truncated"], json_bool(true));
    assert!(data["resumeHint"].is_string(), "resumeHint 缺失");

    // sheet 参数选表输出该表全量。
    let args = DocArgs {
        sheet: Some("汇总".to_string()),
        ..doc_args(&path)
    };
    let outcome = run_doc(&ws.ctx(), &args);
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    let content = data["content"].as_str().expect("content");
    assert!(content.contains("## 汇总"));

    assert!(
        content.contains("| 达成率 | 0.92 |"),
        "小数单元格缺失：{content}"
    );
    assert!(
        !content.contains("产品"),
        "不应混入其他 sheet 内容：{content}"
    );
    assert_eq!(data["selectedSheets"][0], "汇总");

    // 不存在的 sheet 名 → E_INVALID_ARG 且带可用清单（模型据此纠正）。
    let args = DocArgs {
        sheet: Some("不存在的表".to_string()),
        ..doc_args(&path)
    };
    let outcome = run_doc(&ws.ctx(), &args);
    assert!(!outcome.ok);
    let err = outcome.error.expect("e");
    assert_eq!(err.code, "E_INVALID_ARG");
    assert!(
        err.message.contains("销售数据"),
        "错误应带可用 sheet 清单：{err:?}"
    );
}

// 断言：pdf 读取矩阵（验收①③④）——中文 PDF 按页提取（头部「提取可能
// 不完整」标注，#103 已知形态不锚定逐字还原）；pages 窗口正常工作；>10 页
// 无页码拒绝；窗口 >20 页拒绝；page/pages 互斥、页码 0 拒绝、页码超界返回
// 空窗口；畸形 PDF 走 catch_unwind 兜底；加密 PDF 在解析前识别并给明确话术。
#[test]
fn pdf_page_and_error_matrix() {
    // 验收④：>10 页的 PDF 无显式页码被拒（话术带总页数与用法）。
    let ws = TestWorkspace::new();
    let path = copy_fixture(&ws, "sample_zh_12pages.pdf");
    let outcome = run_doc(&ws.ctx(), &doc_args(&path));
    assert!(!outcome.ok, "12 页 PDF 无页码应被拒");
    let err = outcome.error.expect("e");
    assert_eq!(err.code, "E_INVALID_ARG");
    assert!(err.message.contains("12 页"), "错误应带总页数：{err:?}");
    assert!(err.message.contains("page"), "错误应带用法指引：{err:?}");

    // 验收①：中文 PDF 按页提取；输出头部带「提取可能不完整」标注。
    // 已知限制（#103 的实测形态）：CoreGraphics 导出的 CID 键控中文 PDF 无
    // 完整 ToUnicode 映射，部分字会映射为 CJK 部首变体（如「页」→「⻚」）。
    // 断言只锚定我们自己的输出结构（页标记/标注）与「提到了 CJK 内容」。
    let args = DocArgs {
        page: Some(2),
        ..doc_args(&path)
    };
    let outcome = run_doc(&ws.ctx(), &args);
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["pageCount"], 12);
    assert_eq!(data["pages"][0], 2);
    assert_eq!(data["pages"][1], 2);
    let content = data["content"].as_str().expect("content");
    assert!(
        content.starts_with("（PDF 线性文本提取"),
        "头部标注缺失：{content}"
    );
    assert!(content.contains("提取可能不完整"), "#103 标注缺失");
    assert!(content.contains("--- 第 2 页 ---"), "页标记缺失：{content}");
    assert!(content.contains("第 2"), "页码文本缺失：{content}");
    assert!(
        content
            .chars()
            .any(|c| ('\u{2E80}'..='\u{9FFF}').contains(&c)
                || ('\u{3400}'..='\u{4DBF}').contains(&c)),
        "应提取到 CJK 文本：{content}"
    );
    assert_eq!(data["truncated"], json_bool(false));

    // 验收④：pages 窗口正常工作（英文 25 页样本，pages=[3,5] 输出 3 页）。
    let ws = TestWorkspace::new();
    let path = copy_fixture(&ws, "sample_25pages.pdf");
    let args = DocArgs {
        pages: Some((3, 5)),
        ..doc_args(&path)
    };
    let outcome = run_doc(&ws.ctx(), &args);
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    let content = data["content"].as_str().expect("content");
    assert!(content.contains("Page 3 of 25"));
    assert!(content.contains("Page 5 of 25"));
    assert!(!content.contains("Page 6 of 25"), "窗口外内容不应出现");

    // 验收④：页码窗口 >20 页被拒。
    let args = DocArgs {
        pages: Some((1, 21)),
        ..doc_args(&path)
    };
    let outcome = run_doc(&ws.ctx(), &args);
    assert!(!outcome.ok);
    let err = outcome.error.expect("e");
    assert_eq!(err.code, "E_INVALID_ARG");
    assert!(err.message.contains("20"), "错误应带上限值：{err:?}");

    // page 与 pages 互斥；页码 0 拒绝；页码超界返回空窗口（与 read 文本一致）。
    let args = DocArgs {
        page: Some(1),
        pages: Some((1, 2)),
        ..doc_args(&path)
    };
    let outcome = run_doc(&ws.ctx(), &args);
    assert!(!outcome.ok, "page 与 pages 互斥");

    let args = DocArgs {
        page: Some(0),
        ..doc_args(&path)
    };
    let outcome = run_doc(&ws.ctx(), &args);
    assert!(!outcome.ok, "页码 0 应被拒");

    let args = DocArgs {
        pages: Some((99, 100)),
        ..doc_args(&path)
    };
    let outcome = run_doc(&ws.ctx(), &args);
    assert!(outcome.ok, "页码超界应返回空窗口而非报错：{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["content"], "");
    assert_eq!(data["truncated"], json_bool(false));

    // 验收③：畸形 PDF 不崩（catch_unwind 生效），返回友好结构化错误。
    let ws = TestWorkspace::new();
    let path = copy_fixture(&ws, "corrupted.pdf");
    let outcome = run_doc(&ws.ctx(), &doc_args(&path));
    assert!(!outcome.ok, "损坏 PDF 不应成功");
    let err = outcome.error.expect("e");
    assert_eq!(err.code, "E_INVALID_ARG");
    assert!(
        err.message.contains("损坏或加密"),
        "错误话术应说明可能损坏或加密：{err:?}"
    );

    // 验收③：加密 PDF 在解析前被识别（trailer /Encrypt），返回明确话术的
    // 结构化错误（口令不在工具参数面，弱加密解出的内容亦不可信）。
    let ws = TestWorkspace::new();
    let path = copy_fixture(&ws, "sample_encrypted.pdf");
    let outcome = run_doc(&ws.ctx(), &doc_args(&path));
    assert!(!outcome.ok, "加密 PDF 不应成功解析");
    let err = outcome.error.expect("e");
    assert_eq!(err.code, "E_INVALID_ARG");
    assert!(err.message.contains("加密"), "错误话术应说明加密：{err:?}");
}

// 断言：截断、守卫与分派矩阵（验收⑤）——超 512KB 的 docx 内容按整行边界
// 截断 + truncated + 续读指引（帕累托：截断而非整体为空）；50MiB 上限的
// 超大文件本体在读取前拒绝；detect_format 扩展名家族覆盖与大小写不敏感；
// read 工具按扩展名把文档分派进文档管线、普通文本不受影响。
#[test]
fn truncation_guards_and_dispatch_matrix() {
    // 现场构造大文档：每段约 1KB × 1200 段 ≈ 1.2MB > 512KB。
    let ws = TestWorkspace::new();
    let filler = "内容填充数据".repeat(170); // ≈ 1KB（中文 3 字节/字）
    let mut docx = docx_rs::Docx::new();
    for i in 0..1200 {
        docx = docx.add_paragraph(
            docx_rs::Paragraph::new()
                .add_run(docx_rs::Run::new().add_text(format!("第 {i} 段：{filler}"))),
        );
    }
    let file = File::create(ws.project_file("big.docx")).expect("建大文档");
    docx.build().pack(file).expect("打包大文档");

    let outcome = run_doc(&ws.ctx(), &doc_args("big.docx"));
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["truncated"], json_bool(true));
    let content = data["content"].as_str().expect("content");
    assert!(
        content.len() <= 512 * 1024,
        "内容应 ≤512KB（实际 {}）",
        content.len()
    );
    assert!(content.ends_with('\n'), "整行边界截断");
    assert!(data["resumeHint"].is_string(), "截断应有续读指引");
    // 帕累托检查：截断发生而不是整体为空。
    assert!(content.len() > 100 * 1024, "截断点应接近预算");

    // 超大文件本体在读取前拒绝（50MiB 上限；稀疏文件不占磁盘）。
    let ws = TestWorkspace::new();
    let path = ws.project_file("huge.docx");
    let f = File::create(&path).expect("建文件");
    f.set_len(50 * 1024 * 1024 + 1).expect("置为稀疏大文件");
    drop(f);
    let outcome = run_doc(&ws.ctx(), &doc_args("huge.docx"));
    assert!(!outcome.ok);
    let err = outcome.error.expect("e");
    assert_eq!(err.code, "E_INVALID_ARG");
    assert!(err.message.contains("50MiB"), "错误应带上限说明：{err:?}");

    // detect_format：扩展名家族覆盖与大小写不敏感。
    assert_eq!(detect_format("a.DOCX"), Some(DocFormat::Docx));
    assert_eq!(detect_format("a.xlsx"), Some(DocFormat::Xlsx));
    assert_eq!(detect_format("a.xlsm"), Some(DocFormat::Xlsx));
    assert_eq!(detect_format("a.xlsb"), Some(DocFormat::Xlsx));
    assert_eq!(detect_format("a.xls"), Some(DocFormat::Xlsx));
    assert_eq!(detect_format("a.ods"), Some(DocFormat::Xlsx));
    assert_eq!(detect_format("a.PDF"), Some(DocFormat::Pdf));
    assert_eq!(detect_format("a.txt"), None);
    assert_eq!(detect_format("a.png"), None);
    assert_eq!(detect_format("docx"), None, "无扩展名不算文档");

    // read 工具的扩展名分派：read(path=docx) 走文档管线，文本路径不受影响。
    let ws = TestWorkspace::new();
    let path = copy_fixture(&ws, "sample_zh_writer.docx");
    let outcome = run_read(&ws.ctx(), &read_args(&path));
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["kind"], "docx", "read 应把 docx 分派进文档管线");

    std::fs::write(ws.project_file("t.txt"), "hello").expect("写文本");
    let outcome = run_read(&ws.ctx(), &read_args("t.txt"));
    assert!(outcome.ok);
    assert_eq!(outcome.data.expect("data")["kind"], "text");
}
