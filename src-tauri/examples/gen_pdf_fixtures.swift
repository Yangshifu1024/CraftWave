// 测试样本生成器（16 号票，macOS 专用一次性工具）：用 CoreGraphics /
// CoreText（系统文本引擎）导出真实 PDF 样本——纯手写字节无法覆盖中文字体
// 排版与加密字典，这两种「真实世界形态」必须由系统导出。
//
// 产物（入库到 tests/fixtures/docs/）：
//   sample_zh_12pages.pdf  12 页中文文本 PDF（>10 页显式页码门槛 + 中文提取回归）
//   sample_encrypted.pdf   口令加密 PDF（加密文件友好错误样本）
//
// 用法：swift gen_pdf_fixtures.swift（本目录内执行，自动写到 tests/fixtures/docs/）

import CoreGraphics
import CoreText
import Foundation

let outDir = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
    .appendingPathComponent("tests/fixtures/docs")
try? FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

/// 用 CoreText 在 PDF 页上绘制一行文本（坐标系原点左下）。
func drawLine(_ ctx: CGContext, text: String, x: CGFloat, y: CGFloat, size: CGFloat) {
    let font = CTFontCreateWithName("PingFang SC" as CFString, size, nil)
    let attrs: [CFString: Any] = [kCTFontAttributeName: font]
    guard let attrStr = CFAttributedStringCreate(nil, text as CFString, attrs as CFDictionary)
    else { return }
    let line = CTLineCreateWithAttributedString(attrStr)
    ctx.textPosition = CGPoint(x: x, y: y)
    CTLineDraw(line, ctx)
}

/// 生成一份 PDF：`pages` 页，每页两行中文（页码行 + 正文行）；`password`
/// 非空时导出为口令加密 PDF。
func makePdf(path: URL, pages: Int, password: String?) {
    let mediaBox = CGRect(x: 0, y: 0, width: 612, height: 792)
    // 口令加密：CoreGraphics 需要 User 与 Owner 口令同时设置才会真正写入
    // /Encrypt 字典（仅设 UserPassword 时静默导出明文）。
    var aux: [CFString: Any] = [:]
    if let password {
        aux[kCGPDFContextUserPassword] = password
        aux[kCGPDFContextOwnerPassword] = password
    }
    guard let ctx = withUnsafePointer(to: mediaBox, { box in
        CGContext(path as CFURL, mediaBox: box, aux as CFDictionary)
    })
    else { fatalError("创建 PDF 上下文失败：\(path)") }
    for page in 1 ... pages {
        ctx.beginPDFPage(nil)
        drawLine(ctx, text: "第 \(page) 页（共 \(pages) 页）", x: 72, y: 720, size: 24)
        drawLine(
            ctx,
            text: "中文文本提取回归样本：文档读取管线需要正确处理系统导出的中文 PDF 内容。",
            x: 72, y: 660, size: 16
        )
        ctx.endPDFPage()
    }
    ctx.closePDF()
    print("已生成：\(path.path)")
}

makePdf(path: outDir.appendingPathComponent("sample_zh_12pages.pdf"), pages: 12, password: nil)
makePdf(path: outDir.appendingPathComponent("sample_encrypted.pdf"), pages: 1, password: "craftwave-test")
