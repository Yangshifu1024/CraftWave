//! render_html 工具测试：50,000 字符上限、data 形态、PreviewRegistry 的
//! 幂等注册/容量淘汰/token 校验、预览 CSP 的禁外联断言（沙箱防线面）。

use serde_json::json;

use super::{
    MAX_HTML_CHARS, PREVIEW_CSP, RenderHtmlArgs, is_valid_token, run, validate_html_length,
};
use crate::util::test_support::TestWorkspace;

/// 测试辅助：执行工具（不依赖工作区内容，仅用其 ctx 形状）。
fn render(html: &str) -> crate::tools::ToolOutcome {
    let ws = TestWorkspace::new();
    run(
        &ws.ctx(),
        &RenderHtmlArgs {
            html: html.to_string(),
        },
    )
}

// 断言：合法 HTML 返回 data={ html, charCount }，字符数按 UTF-8 字符计
// （中文字符按 1 计，不按字节）。
#[test]
fn returns_data_shape_with_char_count() {
    let html = "<!DOCTYPE html><html><body>你好</body></html>";
    let outcome = render(html);
    assert!(outcome.ok, "{outcome:?}");
    let data = outcome.data.expect("data");
    assert_eq!(data["charCount"], json!(43));
    assert_eq!(data["html"], json!(html));
    assert!(outcome.ui_summary.contains("43 字符"));
}

// 断言：超过 50,000 字符（按字符计）拒绝，E_INVALID_ARG 带上限与实际数。
#[test]
fn rejects_html_over_char_limit() {
    let ok_html = "a".repeat(MAX_HTML_CHARS);
    assert!(render(&ok_html).ok, "恰好 50,000 字符应放行");

    let over = format!("{ok_html}!");
    let outcome = render(&over);
    assert!(!outcome.ok, "50,001 字符应拒绝");
    let err = outcome.error.expect("error");
    assert_eq!(err.code, "E_INVALID_ARG");
    assert!(
        err.message.contains("50,000") || err.message.contains("50000"),
        "{err:?}"
    );
    assert!(
        err.message.contains("50,001") || err.message.contains("50001"),
        "{err:?}"
    );

    // 中文字符按 1 字符计：50,001 个汉字应拒（字节数远超但字符数刚超）。
    let outcome = render(&"汉".repeat(MAX_HTML_CHARS + 1));
    assert!(!outcome.ok);
    let outcome = render(&"汉".repeat(MAX_HTML_CHARS));
    assert!(outcome.ok, "恰好 50,000 个汉字（150KB 字节）应放行");
}

// 断言：validate_html_length 供 preview_register 命令复用的入口（字符数
// 校验与工具同款口径）。
#[test]
fn register_entry_validates_length() {
    assert!(validate_html_length(&"x".repeat(MAX_HTML_CHARS)).is_ok());
    assert!(validate_html_length(&"x".repeat(MAX_HTML_CHARS + 1)).is_err());
}

// 断言：注册表幂等（同内容同 token）、不同内容不同 token、lookup 往返。
#[test]
fn registry_is_content_addressed() {
    let registry = super::PreviewRegistry::new();
    let t1 = registry.register("<p>one</p>");
    let t1_again = registry.register("<p>one</p>");
    let t2 = registry.register("<p>two</p>");
    assert_eq!(t1, t1_again, "同内容幂等");
    assert_ne!(t1, t2);
    assert_eq!(registry.lookup(&t1).as_deref(), Some("<p>one</p>"));
    assert!(is_valid_token(&t1));
    assert!(registry.lookup("0123456789abcdef").is_none());
    assert!(!is_valid_token("SHORT"), "非 16 位 hex 应判非法");
    assert!(!is_valid_token("zzzzzzzzzzzzzzzz"));
}

// 断言：容量上限淘汰最旧（插入序），最新注册保留。
#[test]
fn registry_evicts_oldest_over_capacity() {
    let registry = super::PreviewRegistry::new();
    let first = registry.register("first");
    for i in 0..super::MAX_PREVIEW_ENTRIES {
        registry.register(&format!("entry-{i}"));
    }
    assert!(registry.lookup(&first).is_none(), "最旧应被淘汰");
    let last = registry.register("last");
    assert_eq!(registry.lookup(&last).as_deref(), Some("last"));
}

// 断言：预览页 CSP 禁一切网络面（防内网探测）、放内联脚本（自包含 HTML
// 的合理需要）、无 frame/form/object/base 面——与 iframe sandbox 双重隔离。
#[test]
fn preview_csp_blocks_all_network_surfaces() {
    // 禁外联：connect/img/media 的源清单里不得出现 http/https/通配。
    for directive in ["connect-src", "img-src", "media-src", "font-src"] {
        let part = PREVIEW_CSP
            .split(';')
            .map(str::trim)
            .find(|d| d.starts_with(directive))
            .unwrap_or_else(|| panic!("CSP 缺 {directive} 指令"));
        assert!(
            !part.contains("http") && !part.contains("*"),
            "{directive} 放行了网络源：{part}"
        );
    }
    // 自包含需要：内联脚本/样式放行。
    assert!(PREVIEW_CSP.contains("script-src 'unsafe-inline'"));
    assert!(PREVIEW_CSP.contains("style-src 'unsafe-inline'"));
    // 兜底与封闭面。
    assert!(PREVIEW_CSP.contains("default-src 'none'"));
    assert!(PREVIEW_CSP.contains("frame-src 'none'"));
    assert!(PREVIEW_CSP.contains("form-action 'none'"));
    assert!(PREVIEW_CSP.contains("object-src 'none'"));
    assert!(PREVIEW_CSP.contains("base-uri 'none'"));
}

// 断言：preview_url 按平台形态拼接（非 Windows = preview://localhost）。
#[test]
fn preview_url_shape() {
    if cfg!(any(windows, target_os = "android")) {
        assert!(super::preview_url("0123456789abcdef").starts_with("http://preview.localhost/"));
    } else {
        assert_eq!(
            super::preview_url("0123456789abcdef"),
            "preview://localhost/0123456789abcdef"
        );
    }
}
