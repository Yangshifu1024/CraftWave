//! [`super::redact`] 的表驱动单元测试（32 号票测试口径）：
//! 嵌套对象/数组、key 大小写与结尾模式、Bearer 与 sk- 兜底、其余字段
//! 零破坏、图片 base64 摘要化（含 blob 路径补全与防漏兜底）。

use super::*;
use serde_json::json;

// ---- is_sensitive_key：精确名单 + 大小写 + 结尾模式 ----

#[test]
fn sensitive_key_exact_and_case() {
    for key in [
        "apiKey",
        "api_key",
        "API_KEY",
        "Authorization",
        "TOKEN",
        "secret",
    ] {
        assert!(is_sensitive_key(key), "精确名单应命中：{key}");
    }
    assert!(!is_sensitive_key("apikeyx"), "非整词命中不应判敏感");
    assert!(!is_sensitive_key("model"), "普通键不命中");
    assert!(!is_sensitive_key(""), "空键不命中");
}

#[test]
fn sensitive_key_suffix_patterns() {
    for key in [
        "tavilyApiKey",
        "access_token",
        "client_secret",
        "openai-api-key",
        "dbPassword",
    ] {
        assert!(is_sensitive_key(key), "结尾模式应命中：{key}");
    }
    // 结尾模式只看后缀：不以敏感片段结尾的普通键不误伤
    assert!(
        !is_sensitive_key("keyboard"),
        "keyboard 不以 _key/-key 结尾"
    );
    assert!(
        !is_sensitive_key("tokenCount"),
        "tokenCount 不以 token 结尾"
    );
}

// ---- redact_string：Bearer / sk- 头模式 ----

#[test]
fn redact_bearer_header_value() {
    let input = "Authorization: Bearer sk-abcdef1234567890abcdef 尾部";
    let out = redact_string(input).expect("应命中 Bearer 模式");
    assert!(out.contains("Bearer ***"), "令牌段应被替换：{out}");
    assert!(!out.contains("abcdef1234567890"), "明文不应残留：{out}");
    assert!(out.ends_with(" 尾部"), "尾部普通文本应保留：{out}");
    // 小写 bearer 也兜住（HTTP 头值大小写不敏感的防御面）
    assert!(redact_string("bearer sk-abcdefghijklmnop1234").is_some());
    // 令牌过短（<8）不命中：避免误伤普通词组
    assert!(redact_string("Bearer short").is_none());
}

#[test]
fn redact_sk_prefix_key() {
    let out = redact_string("key=sk-proj-abcdefghijklmnop123456").expect("应命中 sk- 模式");
    assert!(out.contains("sk-***"), "sk- 密钥应替换：{out}");
    assert!(!out.contains("abcdefghijklmnop"), "明文不应残留：{out}");
    // 过短的 sk- 串（<16 位）不命中
    assert!(redact_string("sk-short").is_none());
    assert!(redact_string("请用 task 工具").is_none(), "普通文本不命中");
}

// ---- redact_json：递归 / key 替换 / 其余零破坏 ----

#[test]
fn redact_json_nested_object_and_array() {
    let mut value = json!({
        "providers": [
            { "id": "p1", "apiKey": "sk-abcdefghijklmnop1234", "baseUrl": "https://api.example.com" },
            { "id": "p2", "config": { "access_token": "tok-abcdef123456", "note": "Bearer eyJhbGciOiJIUzI1" } }
        ],
        "count": 3,
        "enabled": true,
        "empty": null
    });
    redact_json(&mut value);
    assert_eq!(value["providers"][0]["apiKey"], "***");
    assert_eq!(value["providers"][1]["config"]["access_token"], "***");
    // 字符串兜底：Bearer 头段替换、其余保留
    let note = value["providers"][1]["config"]["note"].as_str().unwrap();
    assert!(note.starts_with("Bearer ***"), "Bearer 段应替换：{note}");
    // 其余字段零破坏
    assert_eq!(value["providers"][0]["id"], "p1");
    assert_eq!(value["providers"][0]["baseUrl"], "https://api.example.com");
    assert_eq!(value["count"], 3);
    assert_eq!(value["enabled"], true);
    assert!(value["empty"].is_null());
}

#[test]
fn redact_json_normal_values_untouched() {
    let mut value = json!({
        "message": "请帮我写一个 token 计数函数",
        "text": "sk 学习笔记",
        "url": "https://example.com/a/b"
    });
    redact_json(&mut value);
    assert_eq!(
        value["message"], "请帮我写一个 token 计数函数",
        "普通中文正文零破坏（键不含敏感、值无密钥头模式）"
    );
    assert_eq!(value["text"], "sk 学习笔记", "sk- 后非密钥形态不误伤");
    assert_eq!(value["url"], "https://example.com/a/b");
}

// ---- summarize_images：base64 图片段摘要化 ----

/// 生成一段以真实 PNG 魔数开头的 base64 文本（头 8 字节 PNG 签名 + 填充，
/// 共 600 字节 → base64 约 800 字符，超过 512 阈值）；`fill` 变换填充字节
/// 以生成不同内容的图片（不同哈希）。
fn png_base64_with(fill: u8) -> String {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    bytes.extend(std::iter::repeat_n(fill, 600 - 8));
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}

fn png_base64() -> String {
    png_base64_with(0xAB)
}

#[test]
fn summarize_images_standalone_value() {
    let body = png_base64();
    let mut value = json!({ "type": "base64", "value": body });
    summarize_images(&mut value, None);
    let out = value["value"].as_str().unwrap();
    assert!(!out.contains(&body[..64]), "本体 base64 不应残留");
    assert!(
        out.starts_with("[image png sha256:"),
        "摘要应含格式与哈希：{out}"
    );
    assert!(out.contains("已省略本体]"), "摘要应含省略标注：{out}");
    // 哈希可复算：摘要里的 sha256 应等于本体字节的 SHA-256（与 imgblob 同算法）
    let hash = out
        .split("sha256:")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .expect("摘要应含 sha256 段");
    let expected = Sha256::digest(
        base64::engine::general_purpose::STANDARD
            .decode(&body)
            .unwrap(),
    );
    let expected_hex = expected
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    assert_eq!(hash, expected_hex);
}

#[test]
fn summarize_images_inside_text_keeps_source_path() {
    // read 读图结果的 compact 文本形态：源路径与格式在 base64 段之外，
    // 只摘本体段——源路径保留即「工作区源文件路径」的实际记录。
    let body = png_base64();
    let text = format!(
        r#"{{"kind":"image","path":"docs/截图.png","mediaType":"image/png","base64":"{body}"}}"#
    );
    let mut value = json!({ "content": text });
    summarize_images(&mut value, None);
    let out = value["content"].as_str().unwrap();
    assert!(out.contains("docs/截图.png"), "工作区源路径应保留：{out}");
    assert!(out.contains("image/png"), "格式字段应保留：{out}");
    assert!(!out.contains(&body[..64]), "本体 base64 不应残留");
    assert!(out.contains("[image png"), "本体段应替换为摘要：{out}");
}

#[test]
fn summarize_images_appends_real_blob_path_only() {
    let dir = tempfile::tempdir().expect("临时目录");
    let body = png_base64();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&body)
        .unwrap();
    let hash = {
        let digest = Sha256::digest(&bytes);
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    // blob 文件实际存在 → 摘要补 path
    std::fs::write(dir.path().join(format!("{}.bin", &hash[..32])), &bytes)
        .expect("写入 blob 文件");
    let mut value = json!(body);
    summarize_images(&mut value, Some(dir.path()));
    let out = value.as_str().unwrap();
    let blob_file = dir.path().join(format!("{}.bin", &hash[..32]));
    assert!(
        out.contains(&format!("path={}", blob_file.display())),
        "实际存在的 blob 路径应补进摘要：{out}"
    );
    // blob 文件不存在（无来源路径）→ 不虚构路径
    let mut missing = json!(png_base64_with(0xCD));
    summarize_images(&mut missing, Some(dir.path()));
    assert!(
        !missing.as_str().unwrap().contains("path="),
        "无来源的图片不虚构路径：{}",
        missing.as_str().unwrap()
    );
}

#[test]
fn summarize_images_short_or_non_image_strings_untouched() {
    let mut value = json!({
        "hash": "aGVsbG8gd29ybGQgZnJvbSBiYXNlNjQgZW5jb2Rpbmc=",
        "longText": "x".repeat(2000),
        "chinese": "这段中文正文完全没有 base64 段，应当原样保留"
    });
    summarize_images(&mut value, None);
    assert_eq!(
        value["hash"], "aGVsbG8gd29ybGQgZnJvbSBiYXNlNjQgZW5jb2Rpbmc=",
        "短 base64（<512）不进判定"
    );
    assert_eq!(value["longText"], "x".repeat(2000), "非图片魔数长串零破坏");
    assert_eq!(
        value["chinese"],
        "这段中文正文完全没有 base64 段，应当原样保留"
    );
}

#[test]
fn summarize_images_unknown_format_fallback() {
    // 头部是图片魔数但整段解码失败（标准字母表尾部混入 URL 安全字符）：
    // 防漏兜底——仍替换为 unknown 摘要，本体不漏进日志。
    let mut body = png_base64();
    body.replace_range(body.len() - 2.., "__");
    let mut value = json!(body);
    summarize_images(&mut value, None);
    let out = value.as_str().unwrap();
    assert!(out.starts_with("[image"), "异常编码的图片段也应替换：{out}");
    assert!(!out.contains(&body[..64]), "本体不应残留");
}
