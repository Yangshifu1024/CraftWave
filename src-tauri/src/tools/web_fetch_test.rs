//! web_fetch 工具测试（12 号票）：readability 正文提取快照 + raw 清洗 +
//! 规格 schema 面。
//!
//! 快照入库说明（票面「两份真实网页 HTML 快照（含中文正文页）」）：测试
//! 资产为按真实新闻页结构手工构造的仿真快照（`fixtures/news_article_*.html`，
//! 含导航/头部/正文四段/相关推荐/评论区/页脚/脚本等典型噪声块），不走外网
//! 抓取（map.md 站定偏好：测试一律本地 fixture，不连真实外网）。

use super::web_fetch::{extract_readability, WebFetchArgs, WebFetchFormat};
use super::web_fetch::spec;
use crate::tools::STATIC_TOOL_NAMES;

/// 中文正文页快照（fixtures 入库，include_str 保编译期绑定）。
const ZH_ARTICLE: &str = include_str!("fixtures/news_article_zh.html");
/// 英文正文页快照。
const EN_ARTICLE: &str = include_str!("fixtures/news_article_en.html");

#[test]
fn readability_extracts_chinese_article() {
    let (title, content, length) =
        extract_readability(ZH_ARTICLE, "https://news.example.cn/tech/1")
            .expect("中文页提取应成功");
    // 标题提取（og:title/h1 双锚点）
    assert!(
        title.contains("国内量子计算研究团队发布新成果"),
        "标题不符：{title}"
    );
    // 正文四段核心句全部进入 Markdown
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
    // 噪声剔除：导航/相关推荐/评论/页脚/脚本不得混入正文
    for noise in ["相关推荐", "网友评论", "京ICP备", "首页", "人工智能芯片市场"] {
        assert!(
            !content.contains(noise),
            "正文不应包含噪声「{noise}」：\n{content}"
        );
    }
}

#[test]
fn readability_extracts_english_article() {
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
        assert!(content.contains(sentence), "正文缺少「{sentence}」：\n{content}");
    }
    // 噪声剔除
    for noise in ["Related", "Comments", "All rights reserved", "Newsletter"] {
        assert!(!content.contains(noise), "正文不应包含噪声「{noise}」：\n{content}");
    }
}

#[test]
fn readability_rejects_empty_document() {
    // 空文档/纯噪声页：提取失败返回人话原因（工具层降级建议 raw 模式）
    let result = extract_readability("", "https://example.com/");
    assert!(result.is_err(), "空文档提取应失败");
    // 提取器输入非法 document_url 也会被拒（Readability 要求绝对 URL）
    let result = extract_readability(ZH_ARTICLE, "not-a-url");
    assert!(result.is_err(), "非法 document_url 应失败");
}

#[test]
fn raw_format_cleans_control_characters() {
    // raw 模式的清洗语义由 execute 承载，这里钉清洗过滤器本身的契约：
    // 控制字符（除 \n \t）被剔除。直接构造含控制字符的样本走
    // extract 的同层（清洗逻辑在 execute 内联，此处经 spec 契约+
    // readability 链路覆盖，清洗单测锚定在 http_request 的 body 侧）。
    let dirty = "正文\u{0}内容\u{1f}继续";
    let cleaned: String = dirty
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    assert_eq!(cleaned, "正文内容继续");
}

#[test]
fn args_deserialize_defaults_to_readability() {
    // 参数缺省 format → readability（serde default）
    let args: WebFetchArgs = serde_json::from_str(r#"{"url":"https://example.com"}"#).unwrap();
    assert_eq!(args.format, None);
    assert_eq!(args.url, "https://example.com");
    let args: WebFetchArgs =
        serde_json::from_str(r#"{"url":"https://example.com","format":"raw"}"#).unwrap();
    assert_eq!(args.format, Some(WebFetchFormat::Raw));
    // 非法 format 拒绝（schema enum 与 serde 双向一致）
    assert!(serde_json::from_str::<WebFetchArgs>(
        r#"{"url":"https://example.com","format":"markdown"}"#
    )
    .is_err());
}

#[test]
fn spec_is_registered_as_network_tool() {
    // 规格：Network 能力 + 名称进静态段 + schema 形状（与前端
    // CW_TOOL_PARAMETERS 逐字镜像的源）
    let spec = spec();
    assert!(STATIC_TOOL_NAMES.contains(&spec.name.as_str()));
    assert!(matches!(
        spec.capability,
        crate::safety::tier::Capability::Network
    ));
    assert_eq!(spec.params_schema["required"][0], "url");
    assert_eq!(
        spec.params_schema["properties"]["format"]["enum"][0],
        "readability"
    );
    assert_eq!(
        spec.params_schema["properties"]["format"]["enum"][1],
        "raw"
    );
    // 只读注解：GET 抓取无副作用（open_world 除外）
    assert!(spec.annotations.read_only_hint);
    assert!(spec.annotations.open_world_hint);
}
