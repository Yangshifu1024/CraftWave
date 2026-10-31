//! web_fetch 工具测试（12 号票）：readability 正文提取快照（中/英文）+
//! 空文档拒绝 + 参数反序列化缺省面。
//!
//! 快照入库说明（票面「两份真实网页 HTML 快照（含中文正文页）」）：测试
//! 资产为按真实新闻页结构手工构造的仿真快照（`fixtures/news_article_*.html`，
//! 含导航/头部/正文四段/相关推荐/评论区/页脚/脚本等典型噪声块），不走外网
//! 抓取（map.md 站定偏好：测试一律本地 fixture，不连真实外网）。

use super::web_fetch::{WebFetchArgs, WebFetchFormat, extract_readability};

/// 中文正文页快照（fixtures 入库，include_str 保编译期绑定）。
const ZH_ARTICLE: &str = include_str!("fixtures/news_article_zh.html");
/// 英文正文页快照。
const EN_ARTICLE: &str = include_str!("fixtures/news_article_en.html");

// 断言：readability 提取矩阵——中文页标题（og:title/h1 双锚点）与正文四段
// 进 Markdown、噪声（导航/评论/页脚/脚本）不混入；英文页同语义；空文档与
// 非法 document_url 拒绝；参数缺省 format → readability、非法 format 拒绝。
#[test]
fn readability_extraction_and_args_matrix() {
    // —— 中文页 ——
    let (title, content, length) =
        extract_readability(ZH_ARTICLE, "https://news.example.cn/tech/1")
            .expect("中文页提取应成功");
    assert!(
        title.contains("国内量子计算研究团队发布新成果"),
        "标题不符：{title}"
    );
    for sentence in [
        "超导量子比特的相干时间",
        "毫秒量级",
        "量子调控技术",
        "量子化学模拟",
    ] {
        assert!(
            content.contains(sentence),
            "正文缺少「{sentence}」：\n{content}"
        );
    }
    assert!(length > 0, "正文字符数应大于 0");
    for noise in [
        "相关推荐",
        "网友评论",
        "京ICP备",
        "首页",
        "人工智能芯片市场",
    ] {
        assert!(
            !content.contains(noise),
            "正文不应包含噪声「{noise}」：\n{content}"
        );
    }

    // —— 英文页 ——
    let (title, content, _length) =
        extract_readability(EN_ARTICLE, "https://news.example.com/tech/rust-2")
            .expect("英文页提取应成功");
    assert!(
        title.contains("Rust 2.0 Roadmap Outlined by Core Team"),
        "标题不符：{title}"
    );
    for sentence in [
        "backwards compatibility",
        "quarterly progress reports",
        "long-term stability",
    ] {
        assert!(
            content.contains(sentence),
            "正文缺少「{sentence}」：\n{content}"
        );
    }
    for noise in ["Related", "Comments", "All rights reserved", "Newsletter"] {
        assert!(
            !content.contains(noise),
            "正文不应包含噪声「{noise}」：\n{content}"
        );
    }

    // —— 空文档 / 非法 document_url 拒绝 ——
    assert!(
        extract_readability("", "https://example.com/").is_err(),
        "空文档提取应失败"
    );
    assert!(
        extract_readability(ZH_ARTICLE, "not-a-url").is_err(),
        "非法 document_url 应失败"
    );

    // —— 参数反序列化：缺省 format → readability（serde default）、
    //    非法 format 拒绝（schema enum 与 serde 双向一致）——
    let args: WebFetchArgs = serde_json::from_str(r#"{"url":"https://example.com"}"#).unwrap();
    assert_eq!(args.format, None);
    assert_eq!(args.url, "https://example.com");
    let args: WebFetchArgs =
        serde_json::from_str(r#"{"url":"https://example.com","format":"raw"}"#).unwrap();
    assert_eq!(args.format, Some(WebFetchFormat::Raw));
    assert!(
        serde_json::from_str::<WebFetchArgs>(
            r#"{"url":"https://example.com","format":"markdown"}"#
        )
        .is_err()
    );
}
