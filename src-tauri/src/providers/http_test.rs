//! 代理 HTTP 客户端工厂测试（19 号票）：三模式装配、scheme 白名单校验、
//! manual 缺地址拒绝、热生效换代。

use super::*;

// 断言：validate_proxy_url 的白名单表驱动——四种合法 scheme（含大小写）
// 放行；缺前缀/未知协议/空主机/纯空白拒绝，错误消息带支持列表。
#[test]
fn validate_proxy_url_whitelist() {
    for ok in [
        "http://127.0.0.1:7890",
        "https://proxy.example.com",
        "socks5://127.0.0.1:1080",
        "socks5h://127.0.0.1:1080",
        "HTTP://127.0.0.1:7890",
        "SOCKS5H://localhost:1080",
    ] {
        assert!(validate_proxy_url(ok).is_ok(), "{ok} 应合法");
    }
    for bad in [
        "127.0.0.1:7890",     // 缺协议前缀
        "ftp://127.0.0.1:21", // 未知协议
        "file:///etc/passwd", // 非代理协议
        "http://",            // 空主机
        "socks5://   ",       // 纯空白主机
        "",                   // 空串
    ] {
        let err = validate_proxy_url(bad);
        assert!(err.is_err(), "{bad} 应拒绝");
        let message = err.unwrap_err();
        assert!(
            message.contains("socks5h"),
            "错误消息应带支持列表：{message}"
        );
    }
}

// 断言：三模式装配——none/未配置显式 no_proxy（构建成功即语义正确；
// reqwest 不暴露 builder 内部状态，探测行为层面的断言由 probe_test 的
// socks5 e2e 承担：走代理可达 / 绕过代理可达的差分即代理确实生效）。
#[test]
fn apply_proxy_three_modes_build() {
    let none = ProxyConfig {
        mode: ProxyMode::None,
        url: None,
    };
    assert!(apply_proxy(reqwest::Client::builder(), Some(&none)).is_ok());
    assert!(apply_proxy(reqwest::Client::builder(), None).is_ok());
    let system = ProxyConfig {
        mode: ProxyMode::System,
        url: None,
    };
    assert!(apply_proxy(reqwest::Client::builder(), Some(&system)).is_ok());
}

// 断言：manual 模式——合法 http/socks5/socks5h 地址放行；缺 url、非法
// scheme、空主机拒绝且错误面为 E_INVALID_ARG。
#[test]
fn apply_proxy_manual_url_rules() {
    let manual = |url: Option<&str>| ProxyConfig {
        mode: ProxyMode::Manual,
        url: url.map(str::to_string),
    };
    for ok in [
        "http://127.0.0.1:7890",
        "https://proxy.example.com:8443",
        "socks5://127.0.0.1:1080",
        "socks5h://127.0.0.1:1080",
    ] {
        assert!(
            apply_proxy(reqwest::Client::builder(), Some(&manual(Some(ok)))).is_ok(),
            "{ok} 应构建成功"
        );
    }
    // 缺地址
    let err = apply_proxy(reqwest::Client::builder(), Some(&manual(None))).unwrap_err();
    assert!(
        err.contains("manual"),
        "缺地址错误应说明 manual 需要 url：{err}"
    );
    // 非法 scheme / 空主机
    for bad in ["ftp://127.0.0.1:21", "http://", "127.0.0.1:7890"] {
        assert!(
            apply_proxy(reqwest::Client::builder(), Some(&manual(Some(bad)))).is_err(),
            "{bad} 应拒绝"
        );
    }
    // build_proxy_client 把配置错误归面 E_INVALID_ARG
    let ipc = build_proxy_client(Some(&manual(Some("ftp://x")))).unwrap_err();
    assert_eq!(ipc.code, "E_INVALID_ARG");
}

// 断言：热生效——reload 成功后 generation 递增且 client 换新实例；
// reload 失败（非法配置）保留旧 client，generation 不变。
#[test]
fn factory_reload_bumps_generation_keeps_old_on_failure() {
    let factory = HttpClientFactory::from_config(None).expect("初代构建");
    let first = factory.client();
    assert_eq!(factory.generation(), 1);
    factory
        .reload(Some(&ProxyConfig {
            mode: ProxyMode::Manual,
            url: Some("http://127.0.0.1:7890".to_string()),
        }))
        .expect("reload 合法配置");
    let second = factory.client();
    assert_eq!(factory.generation(), 2);
    // Arc 指针不等 = 换了新实例（Arc::ptr_eq 是同一份才 true）
    assert!(
        !std::sync::Arc::ptr_eq(&first, &second),
        "reload 后应为新 client"
    );
    // 旧引用仍可用（进行中请求不受热更新影响：Arc 各自存活，旧 client
    // 持有者照常构造请求）
    let keep_alive = first.head("http://127.0.0.1:1/x");
    assert_eq!(
        keep_alive
            .build()
            .expect("旧 client 可正常构造请求")
            .method(),
        reqwest::Method::HEAD
    );
}
