//! 日志脱敏（对齐 rig `redact` 的本项目自有实现）。
//!
//! 本模块是日志落盘前的最后一道闸：任何进入会话日志或应用日志的
//! JSON 载荷，必须先经过 [`redact_json`]（密钥脱敏）与
//! [`summarize_images`]（图片本体摘要化），保证——
//!
//! 1. **密钥红线**：API Key 明文 / Authorization 头值 / Bearer 令牌
//!    / `sk-` 前缀密钥，在任何日志位置都不以明文出现；
//! 2. **图片红线**：日志任何位置不出现图片本体字节（base64 或二进制），
//!    图片只记实际可用路径、内容哈希、格式与大小。
//!
//! 设计对齐 rig `redact` 的复刻要求（「落盘前 JSON redact 脱敏」），
//! 但为**本项目自有实现**（rig 依赖内不复刻、不引第三方）：
//! - rig `redact::json` 只扫字符串值；本模块在此基础上增加
//!   **对象 key 名单替换**（敏感命名的键，其值无论形态一律替换 `"***"`）；
//! - 字符串兜底只做两类头模式（`Bearer <…>` / `sk-…`），
//!   不追求覆盖全部厂商密钥形态（名单硬编码、攻击面最小化）。

use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// 敏感值替换后的统一占位。
pub const REDACTED: &str = "***";

/// 敏感键名单的精确段（小写比较）。键名（小写化后）等于其中之一即视为敏感。
const SENSITIVE_KEYS_EXACT: &[&str] = &[
    "apikey",
    "api_key",
    "authorization",
    "token",
    "secret",
    "password",
    "passwd",
    "credential",
    "credentials",
];

/// 敏感键名单的结尾模式（小写比较）：键名（小写化后）以下列片段**结尾**
/// 即视为敏感（「含 key/token 结尾类子串」口径）。例如 `tavilyApiKey`
/// `access_token` `client_secret` 都被覆盖。
const SENSITIVE_KEY_SUFFIXES: &[&str] = &[
    "apikey",
    "api_key",
    "_key",
    "-key",
    "token",
    "secret",
    "password",
    "passwd",
    "credential",
];

/// 判定一个对象键名是否命中敏感名单（大小写不敏感 + 结尾模式）。
pub fn is_sensitive_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    if lowered.is_empty() {
        return false;
    }
    if SENSITIVE_KEYS_EXACT.contains(&lowered.as_str()) {
        return true;
    }
    SENSITIVE_KEY_SUFFIXES
        .iter()
        .any(|suffix| lowered.ends_with(suffix))
}

/// 判定一个字符串值是否携带「以该位置开始的密钥头模式」并返回
/// `(替换段结束位置, 替换文本)`。
///
/// 兜底两类：
/// - `Bearer <非空白 ≥8 字符>`：Authorization 头值的常见形态（如
///   `Bearer sk-…`、`Bearer eyJ…`）——令牌段替换为 `***`、`Bearer `
///   前缀保留（日志可读：能看出这是一条鉴权头，但看不出值）；
/// - `sk-<≥16 位 [A-Za-z0-9_-]>`：OpenAI 系密钥形态（`sk-proj-` 等前缀
///   也在其内）——替换为 `sk-***`。
///
/// 返回 `None` = 该位置不是密钥头（调用方继续后移扫描）。
fn secret_span_at(bytes: &[u8], at: usize) -> Option<(usize, &'static str)> {
    let rest = &bytes[at..];
    if rest.starts_with(b"Bearer ") || rest.starts_with(b"bearer ") {
        let token = &rest[7..];
        let len = token
            .iter()
            .position(|b| b.is_ascii_whitespace())
            .unwrap_or(token.len());
        if len >= 8 {
            return Some((7 + len, "Bearer ***"));
        }
    }
    if rest.starts_with(b"sk-") {
        let len = rest[3..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_' || **b == b'-')
            .count();
        if len >= 16 {
            return Some((3 + len, "sk-***"));
        }
    }
    None
}

/// 字符串值兜底脱敏：扫 `Bearer <…>` 与 `sk-…` 头模式，命中段替换占位。
/// 无命中的输入原样返回 `None`（调用方零拷贝保留原值）。
pub fn redact_string(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out: Option<String> = None;
    let mut copied = 0usize;
    let mut at = 0usize;
    while at < bytes.len() {
        if let Some((end, replacement)) = secret_span_at(bytes, at) {
            let out = out.get_or_insert_with(|| String::with_capacity(input.len()));
            out.push_str(&input[copied..at]);
            out.push_str(replacement);
            copied = at + end;
            at = copied;
        } else {
            at += 1;
        }
    }
    out.map(|mut out| {
        out.push_str(&input[copied..]);
        out
    })
}

/// JSON 载荷密钥脱敏（密钥红线入口）：递归遍历——
/// - 对象键命中敏感名单 → 值整体替换 `***`（无论原值形态；键名保留）；
/// - 字符串值 → [`redact_string`] 兜底（`Bearer`/`sk-` 头模式）；
/// - 数组逐元素递归；其余标量零破坏。
///
/// 敏感键的值**不做**字符串兜底（已被整体替换）；键名本身不脱敏
/// （对齐 rig `redact::json` 口径：键原样保留）。
pub fn redact_json(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, val) in fields.iter_mut() {
                if is_sensitive_key(key) {
                    *val = Value::String(REDACTED.to_owned());
                } else {
                    redact_json(val);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_json),
        Value::String(text) => {
            if let Some(redacted) = redact_string(text) {
                *text = redacted;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// 连续 base64 段的最小判定长度（字符）。图片本体 base64 至少数千字符；
/// 阈值取 512：低于它的普通短编码串（如哈希、token 样式）不进入解码判定，
/// 避免误伤普通文本。
const IMAGE_BASE64_MIN: usize = 512;

/// base64 字符集判定（标准字母表 + URL 安全字母表 + 填充）。
fn is_base64_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'-' || b == b'_' || b == b'='
}

/// 图片格式魔数（png/jpeg/gif/webp 四种白名单，与 storage::sniff_image_mime
/// 同一口径；此处独立实现——util 不依赖 storage，依赖方向 storage → util）。
fn sniff_image_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some("png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

/// 图片摘要占位（不含任何本体字节；`path` 段仅在实际可用时由调用方拼入，
/// 见 [`summarize_images`] 的路径说明）。
fn image_placeholder(format: &str, hash: &str, size: usize) -> String {
    format!("[image {format} sha256:{hash} size={size}B 已省略本体]")
}

/// 对一段候选 base64 文本尝试判定为图片本体：按 4 字符对齐解码头部
/// （64 字节足够全部魔数），命中四种图片魔数之一才算图片。
/// 返回 `Some((format, hash, size))`（hash = 解码后完整字节的 SHA-256
/// 小写 hex，与 imgblob 内容寻址同算法，可对上 blob 文件名）。
///
/// 防漏兜底：头部确认是图片但全量解码失败（混合字母表等异常形态）时，
/// 仍以 `format="unknown"` 返回摘要（哈希/大小按原始 base64 文本字节计，
/// 编码后口径）——脱敏方向宁可误替换、不可漏本体。
fn image_summary_of_base64(text: &str) -> Option<(&'static str, String, usize)> {
    // 头部对齐解码：base64 每 4 字符产 3 字节，取前 128 个字符（≥96 字节）
    // 足够判魔数；长度不足 4 倍数时截到对齐边界。
    let head_chars = text
        .as_bytes()
        .iter()
        .take(128)
        .copied()
        .collect::<Vec<_>>();
    let aligned = head_chars.len() - head_chars.len() % 4;
    if aligned == 0 {
        return None;
    }
    let head = String::from_utf8_lossy(&head_chars[..aligned]).into_owned();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(head.as_bytes())
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(head.as_bytes()))
        .ok()?;
    let format = sniff_image_format(&decoded)?;
    // 全量哈希/大小：按解码后字节（与 imgblob 内容寻址对得上）；混合
    // 字母表等解码失败场景按原始文本字节兜底（防漏，见函数头注释）。
    let trimmed = text.trim_end_matches('=');
    match base64::engine::general_purpose::STANDARD
        .decode(trimmed)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(trimmed))
    {
        Ok(bytes) => Some((format, sha256_hex(&bytes), bytes.len())),
        Err(_) => Some(("unknown", sha256_hex(text.as_bytes()), text.len())),
    }
}

/// 字节序列的 SHA-256 小写 hex。
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// 在一个字符串值中查找并替换全部图片本体 base64 段（中段替换，两侧普通
/// 文本保留——read 读图结果的 compact 文本形态是
/// `{"kind":"image","path":"<源路径>",…,"base64":"<本体>"}`，路径与格式
/// 字段已在文本里，只摘掉本体段即可满足「路径/哈希/格式/大小、无本体」）。
/// 未命中返回 `None`（零拷贝保留原值）。
fn replace_image_segments(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out: Option<String> = None;
    let mut copied = 0usize;
    let mut at = 0usize;
    while at < bytes.len() {
        // 找下一候选段起点：base64 起始字符（字母/数字/+/色-_）
        if !is_base64_char(bytes[at]) {
            at += 1;
            continue;
        }
        let start = at;
        let mut end = at;
        while end < bytes.len() && is_base64_char(bytes[end]) {
            end += 1;
        }
        let len = end - start;
        if len >= IMAGE_BASE64_MIN
            && let Some((format, hash, size)) = image_summary_of_base64(&input[start..end])
        {
            let out = out.get_or_insert_with(|| String::with_capacity(input.len()));
            out.push_str(&input[copied..start]);
            out.push_str(&image_placeholder(format, &hash, size));
            copied = end;
        }
        at = end.max(start + 1);
    }
    out.map(|mut out| {
        out.push_str(&input[copied..]);
        out
    })
}

/// 图片本体摘要化（图片红线落盘闸）：递归遍历 JSON 载荷——
///
/// - 每个字符串值按 [`replace_image_segments`] 扫描，命中的超长 base64
///   图片段替换为 `[image <格式> sha256:<哈希> size=<字节>B 已省略本体]`
///   摘要文本（哈希 = 本体字节 SHA-256，与 imgblob 内容寻址同算法）；
/// - `blob_dir` 给定时（宿主传入会话 imgblob 目录），若对应 blob 文件
///   **实际存在**，摘要文本追加 `path=<文件路径>`——「只有实际获得的原始
///   路径才记录，不为无来源路径的图片虚构路径」：附件/粘贴图以内容寻址
///   落在 imgblob，路径可由哈希实证；read 读图的工作区源路径已在工具
///   结果文本内（base64 段外的普通文本，不被替换）。
///
/// 与 [`redact_json`] 配套使用：先摘要图片、再密钥脱敏（摘要文本不含
/// 密钥样式，顺序无强依赖，双重兜底）。
pub fn summarize_images(value: &mut Value, blob_dir: Option<&std::path::Path>) {
    match value {
        Value::Object(fields) => {
            for (_, val) in fields.iter_mut() {
                summarize_images(val, blob_dir);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| summarize_images(v, blob_dir)),
        Value::String(text) => {
            // 独立承载数据的字符串值（如 rig Image 的 base64 字段）与
            // 夹在普通文本里的 base64 段（如 read 读图工具结果的 compact
            // 文本）都走同一中段扫描：整段即本体时输出为纯占位，两侧
            // 普通文本（源路径等）原样保留。
            if let Some(replaced) = replace_image_segments(text) {
                *text = replaced;
            }
            if let Some(blob_dir) = blob_dir {
                append_blob_path(text, blob_dir);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// 摘要替换后尝试补路径：文本含 `sha256:<hash>` 且对应 blob 文件实际存在
/// 时，在占位文本尾部追加 `path=<blob 文件>`。哈希取前 32 hex（与
/// imgblob 文件名同长），目录外不构造、文件不存在不构造。
fn append_blob_path(text: &mut String, blob_dir: &std::path::Path) {
    let Some(pos) = text.find("sha256:") else {
        return;
    };
    let after = &text[pos + 7..];
    let hash32: String = after
        .chars()
        .take_while(|c| c.is_ascii_hexdigit())
        .take(32)
        .collect();
    if hash32.len() < 32 {
        return;
    }
    let blob_path = blob_dir.join(format!("{hash32}.bin"));
    if blob_path.is_file() {
        text.push_str(&format!(" path={}", blob_path.display()));
    }
}

#[cfg(test)]
#[path = "redact_test.rs"]
mod redact_test;
