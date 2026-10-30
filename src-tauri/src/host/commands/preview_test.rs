//! preview_register 命令测试：注册往返、幂等、超限拒绝、URL 形态。

use super::{PreviewRegisterArgs, preview_register};

// 断言：注册返回 token + URL，注册表按 token 可取回原文。
#[test]
fn registers_and_returns_url() {
    let html = "<p>hello preview</p>";
    let result = preview_register(PreviewRegisterArgs {
        html: html.to_string(),
    })
    .expect("注册成功");
    assert!(crate::tools::render_html::is_valid_token(&result.token));
    assert_eq!(
        result.url,
        crate::tools::render_html::preview_url(&result.token)
    );
    assert_eq!(
        crate::tools::render_html::preview_registry()
            .lookup(&result.token)
            .as_deref(),
        Some(html)
    );
}

// 断言：同内容幂等（token 稳定），不同内容不同 token。
#[test]
fn registration_is_idempotent() {
    let a = preview_register(PreviewRegisterArgs {
        html: "<p>a</p>".to_string(),
    })
    .expect("a");
    let a2 = preview_register(PreviewRegisterArgs {
        html: "<p>a</p>".to_string(),
    })
    .expect("a2");
    let b = preview_register(PreviewRegisterArgs {
        html: "<p>b</p>".to_string(),
    })
    .expect("b");
    assert_eq!(a.token, a2.token);
    assert_ne!(a.token, b.token);
}

// 断言：超过 50,000 字符拒绝（E_INVALID_ARG），与 render_html 同款口径。
#[test]
fn rejects_over_limit() {
    let err = preview_register(PreviewRegisterArgs {
        html: "x".repeat(crate::tools::render_html::MAX_HTML_CHARS + 1),
    })
    .expect_err("应拒绝");
    assert_eq!(err.code, "E_INVALID_ARG");
}
