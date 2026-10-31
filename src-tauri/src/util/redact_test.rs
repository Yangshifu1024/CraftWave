//! [`super::redact`] 的表驱动单元测试（32 号票测试口径）：
//! 敏感键判定、Bearer/sk- 字符串兜底、JSON 递归脱敏与零破坏、
//! 图片 base64 摘要化（含 blob 路径补全与防漏兜底）。

use super::*;
use serde_json::json;

// ---- is_sensitive_key：精确名单 + 大小写 + 结尾模式（表驱动），
// ---- 与 redact_string 的 Bearer / sk- 头模式合并为一个测试。 ----

#[test]
fn sensitive_key_and_string_patterns_tables() {
    for (key, expect, why) in [
        ("apiKey", true, "精确名单命中"),
        ("api_key", true, "精确名单下划线形态"),
        ("API_KEY", true, "大小写不敏感"),
        ("Authorization", true, "精确名单"),
        ("TOKEN", true, "精确名单大写"),
        ("secret", true, "精确名单"),
        ("tavilyApiKey", true, "结尾 key 模式"),
        ("access_token", true, "结尾 token 模式"),
        ("client_secret", true, "结尾 secret 模式"),
        ("openai-api-key", true, "结尾 -key 模式"),
        ("dbPassword", true, "结尾 password 模式"),
        ("apikeyx", false, "非整词命中不应判敏感"),
        ("model", false, "普通键不命中"),
        ("", false, "空键不命中"),
        ("keyboard", false, "keyboard 不以 _key/-key 结尾"),
        ("tokenCount", false, "tokenCount 不以 token 结尾"),
    ] {
        assert_eq!(is_sensitive_key(key), expect, "敏感键判定[{key}] {why}");
    }

    let bearer = redact_string("Authorization: Bearer sk-abcdef1234567890abcdef 尾部")
        .expect("应命中 Bearer 模式");
    assert!(bearer.contains("Bearer ***"), "令牌段应被替换：{bearer}");
    assert!(
        !bearer.contains("abcdef1234567890"),
        "明文不应残留：{bearer}"
    );
    assert!(bearer.ends_with(" 尾部"), "尾部普通文本应保留：{bearer}");

    let sk = redact_string("key=sk-proj-abcdefghijklmnop123456").expect("应命中 sk- 模式");
    assert!(sk.contains("sk-***"), "sk- 密钥应替换：{sk}");
    assert!(!sk.contains("abcdefghijklmnop"), "明文不应残留：{sk}");

    for (input, expect_hit, why) in [
        ("bearer sk-abcdefghijklmnop1234", true, "小写 bearer 也兜住"),
        ("Bearer short", false, "令牌过短（<8）不命中"),
        ("sk-short", false, "sk- 串过短（<16）不命中"),
        ("请用 task 工具", false, "普通文本不命中"),
    ] {
        assert_eq!(
            redact_string(input).is_some(),
            expect_hit,
            "redact_string 命中判定[{input}] {why}"
        );
    }
}

// ---- redact_json：递归 / key 替换 / 其余零破坏；
// ---- 与 summarize_images 的摘要化语义（替换形态 / blob 路径 / 零破坏 / 兜底）合并。 ----

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
fn redact_json_and_summarize_images_semantics() {
    let mut value = json!({
        "providers": [
            { "id": "p1", "apiKey": "sk-abcdefghijklmnop1234", "baseUrl": "https://api.example.com" },
            { "id": "p2", "config": { "access_token": "tok-abcdef123456", "note": "Bearer eyJhbGciOiJIUzI1" } }
        ],
        "message": "请帮我写一个 token 计数函数",
        "text": "sk 学习笔记",
        "count": 3,
        "enabled": true,
        "empty": null
    });
    redact_json(&mut value);
    assert_eq!(
        value["providers"][0]["apiKey"], "***",
        "嵌套对象敏感键应替换"
    );
    assert_eq!(
        value["providers"][1]["config"]["access_token"], "***",
        "更深嵌套对象敏感键应替换"
    );
    let note = value["providers"][1]["config"]["note"].as_str().unwrap();
    assert!(
        note.starts_with("Bearer ***"),
        "字符串值 Bearer 段应替换：{note}"
    );
    assert_eq!(
        value["message"], "请帮我写一个 token 计数函数",
        "普通中文正文零破坏（键不含敏感、值无密钥头模式）"
    );
    assert_eq!(value["text"], "sk 学习笔记", "sk- 后非密钥形态不误伤");
    assert_eq!(value["providers"][0]["id"], "p1", "兄弟字段零破坏");
    assert_eq!(
        value["providers"][0]["baseUrl"], "https://api.example.com",
        "兄弟字段零破坏"
    );
    assert_eq!(value["count"], 3, "数字零破坏");
    assert_eq!(value["enabled"], true, "布尔零破坏");
    assert!(value["empty"].is_null(), "null 零破坏");

    // summarize_images：base64 图片段摘要化。
    // 1) 独立 base64 值：替换为摘要，本体不残留，哈希可复算（与 imgblob 同算法）。
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
    assert_eq!(hash, expected_hex, "摘要哈希应等于本体字节的 SHA-256");

    // 2) read 读图结果的 compact 文本形态：只摘本体段，源路径与格式保留——
    //    源路径保留即「工作区源文件路径」的实际记录。
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

    // 3) blob 目录：实际存在的 blob 文件补 path；无来源不虚构路径。
    let dir = tempfile::tempdir().expect("临时目录");
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&body)
        .unwrap();
    let digest = Sha256::digest(&bytes);
    let hash = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
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
    let mut missing = json!(png_base64_with(0xCD));
    summarize_images(&mut missing, Some(dir.path()));
    assert!(
        !missing.as_str().unwrap().contains("path="),
        "无来源的图片不虚构路径：{}",
        missing.as_str().unwrap()
    );

    // 4) 零破坏负例：短 base64（<512）、非图片魔数长串、纯中文不进判定。
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
        value["chinese"], "这段中文正文完全没有 base64 段，应当原样保留",
        "纯中文正文零破坏"
    );

    // 5) 防漏兜底：头是图片魔数但整段解码失败（尾部混入 URL 安全字符），
    //    仍替换为摘要，本体不漏进日志。
    let mut broken = png_base64();
    broken.replace_range(broken.len() - 2.., "__");
    let mut value = json!(broken);
    summarize_images(&mut value, None);
    let out = value.as_str().unwrap();
    assert!(out.starts_with("[image"), "异常编码的图片段也应替换：{out}");
    assert!(!out.contains(&broken[..64]), "本体不应残留");
}
