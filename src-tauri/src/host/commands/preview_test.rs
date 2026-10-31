//! preview_register 命令测试：注册往返、幂等、不同内容不同 token、超限拒绝。

use super::{PreviewRegisterArgs, preview_register};

/// 断言注册语义全链：返回 token + URL、注册表按 token 可取回原文；同内容
/// 幂等（token 稳定）、不同内容不同 token；超过 50,000 字符拒绝
/// （E_INVALID_ARG，与 render_html 同款口径）。
#[test]
fn preview_register_semantics() {
    // 注册往返。
    let html = "<p>hello preview</p>";
    let result = preview_register(PreviewRegisterArgs {
        html: html.to_string(),
    })
    .expect("注册成功");
    assert!(craftwave_engine::tools::render_html::is_valid_token(&result.token));
    assert_eq!(
        result.url,
        craftwave_engine::tools::render_html::preview_url(&result.token)
    );
    assert_eq!(
        craftwave_engine::tools::render_html::preview_registry()
            .lookup(&result.token)
            .as_deref(),
        Some(html)
    );

    // 幂等与差异。
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
    assert_eq!(a.token, a2.token, "同内容 token 稳定");
    assert_ne!(a.token, b.token, "不同内容不同 token");

    // 超限拒绝。
    let err = preview_register(PreviewRegisterArgs {
        html: "x".repeat(craftwave_engine::tools::render_html::MAX_HTML_CHARS + 1),
    })
    .expect_err("应拒绝");
    assert_eq!(err.code, "E_INVALID_ARG");
}
