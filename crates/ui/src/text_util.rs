//! 文本工具函数（gpui-rewrite 阶段 2.C-3，rendering.md「文本工具函数」逐条）。
//!
//! 四个函数全部从 Wake 蓝本**行为照搬**（含边界情况，代码按本仓库风格重写）：
//!
//! - [`clip_display`]：按**显示单元宽度**截断（CJK 记 2、字素簇不可拆分、
//!   省略号宽度计入预算）——中文 UI 截断的完整答案；
//! - [`one_line`]：多行压成一行（空白折叠为单空格）再按字符数截断加 `…`；
//! - [`smart_time`]：epoch 毫秒 → 渐进式时间文案（刚刚 → N 分钟前 → 今天
//!   时刻 → 昨天 → 日期），越近越强调新鲜度、越远越强调日期锚点；
//! - [`tilde_path`]：绝对路径的用户目录折叠为 `~`（**边界必须落在分隔符上**，
//!   防止 HOME 的同名前缀兄弟目录被误折叠）。
//!
//! 依赖选型（锁内已有、零新增下载）：
//! - `unicode-width 0.2`：CJK 宽度与 emoji 序列宽度的权威实现——0.2 起
//!   「well-formed ZWJ 序列计 2、肤色修饰序列计 2」（其 crate 文档钉死的
//!   规则），这正是 clip_display 对 emoji 不劈半的宽度依据；gpui-base 同款
//!   依赖（文本 shaping 也用它算宽），锁面 0.2.3；
//! - `unicode-segmentation 1.13`：UAX #29 字素簇切分（组合符/ZWJ emoji 归为
//!   一个簇），截断以簇为单位即天然不劈半；gpui-base 同款依赖，锁面 1.13.3。

use std::path::PathBuf;
use std::sync::OnceLock;

use chrono::{DateTime, Datelike, Duration, Local, TimeZone};
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthStr as _;

use crate::i18n::t;

// ======================================================================
// clip_display：显示单元宽度截断（Wake format.rs 行为照搬）
// ======================================================================

/// 按**显示单元宽度**截断文本，确有截断时补一个省略号（`…`，宽 1 也计入
/// `max_width` 预算）。
///
/// 两条宽度纪律：
/// - **CJK 宽字符按 2 个单元计**（unicode-width 的 East Asian Width 判定，
///   不是按 code point 数量或字节数猜）；
/// - **字素簇是截断的最小单位**（`graphemes(true)`：组合符「e + U+0301」、
///   ZWJ emoji 家庭 emoji、肤色修饰 emoji 各算一个簇，永远不会被劈成
///   两半渲染出乱码）。
///
/// 预算极端情况：`max_width` 小于省略号自身宽度（1）时返回空串；预算刚好
/// 装下原文时原样返回（不加省略号）。
pub fn clip_display(s: &str, max_width: usize) -> String {
    // 原文在预算内：原样返回，不画蛇添足加省略号
    if s.width() <= max_width {
        return s.to_string();
    }
    // 省略号自身要占 1 个单元；预算连它都装不下就直接给空串
    let ellipsis = "…";
    let ellipsis_width = ellipsis.width();
    if max_width < ellipsis_width {
        return String::new();
    }
    // 内容预算 = 总预算 - 省略号宽度；逐字素簇装箱，放不下就停
    let content_width = max_width - ellipsis_width;
    let mut clipped = String::new();
    let mut used = 0;
    for grapheme in s.graphemes(true) {
        let width = grapheme.width();
        if used + width > content_width {
            break;
        }
        clipped.push_str(grapheme);
        used += width;
    }
    clipped.push_str(ellipsis);
    clipped
}

// ======================================================================
// one_line：压行 + 按字符数截断（wake-core text.rs 行为照搬）
// ======================================================================

/// 折成一行：空白（换行/制表/连续空格）压成单空格，超过 `max_chars` 个
/// **字符**截断并加 `…`。
///
/// 不是「取首行」——首行可能是空的（消息以 `\n` 开头时按首行取会得到空串），
/// 必须先整段压平。截断按字符数计（`char_indices().nth` 找第 N 个字符的字节
/// 边界），CJK 安全，不会把多字节字符切出非法 UTF-8。
pub fn one_line(s: &str, max_chars: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(max_chars) {
        // 第 max_chars 个字符存在 → 截到它前面的字节边界再加省略号
        Some((ix, _)) => format!("{}…", &flat[..ix]),
        // 整串不超过 max_chars：压平后原样返回
        None => flat,
    }
}

// ======================================================================
// smart_time：渐进式时间文案（Wake format.rs 行为照搬）
// ======================================================================

/// epoch 毫秒 → 会话时间的渐进式展示：
/// 刚刚（1 分钟内）→ N 分钟前（1 小时内）→ 今天时刻 → 昨天 → 日期
/// （今年不带年份，往年带）。`ts <= 0` 或越界值给空串。
pub fn smart_time(ts: i64) -> String {
    smart_time_from(ts, &Local::now())
}

/// [`smart_time`] 的可测本体：`now` 由调用方注入（生产传本机当前时间，
/// 测试传固定锚点），分档判定不依赖真实时钟。
pub(crate) fn smart_time_from(ts: i64, now: &DateTime<Local>) -> String {
    // 无效时间戳（未设置/被清零）给空串，不显示「刚刚」假新鲜
    if ts <= 0 {
        return String::new();
    }
    let Some(dt) = Local.timestamp_millis_opt(ts).single() else {
        return String::new();
    };
    const MIN: i64 = 60_000;
    let diff = now.timestamp_millis() - ts;
    if (0..MIN).contains(&diff) {
        t("Just now").to_string()
    } else if (MIN..60 * MIN).contains(&diff) {
        // 数字 + 标签两段拼（i18n 词表只存标签；中英语序恰好同构）
        format!("{} {}", diff / MIN, t("min ago"))
    } else if dt.date_naive() == now.date_naive() {
        // 今天：只剩时刻（12 小时制无前导零，如 3:42 PM）
        dt.format(t("%-I:%M %p")).to_string()
    } else if dt.date_naive() == now.date_naive() - Duration::days(1) {
        t("Yesterday").to_string()
    } else {
        calendar_date(&dt, now)
    }
}

/// 一天以前的日期档：今年的不带年份（"Oct 12"），往年的带（"Oct 12, 2025"）。
fn calendar_date(dt: &DateTime<Local>, now: &DateTime<Local>) -> String {
    let format = if dt.year() == now.year() {
        t("%b %-d")
    } else {
        t("%b %-d, %Y")
    };
    dt.format(format).to_string()
}

// ======================================================================
// tilde_path：HOME 折叠（Wake format.rs 行为照搬）
// ======================================================================

/// 进程内不变的 HOME，缓存住：路径折叠在渲染路径每帧都要调，不能每次走 env。
fn cached_home() -> Option<&'static str> {
    static HOME: OnceLock<Option<String>> = OnceLock::new();
    HOME.get_or_init(home_dir)
        .as_deref()
}

/// 本机用户目录：Unix 走 `HOME`，Windows 走 `USERPROFILE`（平台差异的窄接缝；
/// Wake 用 dirs crate，本仓库锁内虽已有 dirs 但为了一个 env 读取不值得引入
/// 直接依赖——两个变量缺一即返回 None，路径原样展示不折叠）。
fn home_dir() -> Option<String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .map(|h| h.to_string_lossy().to_string())
        .filter(|h| !h.is_empty())
}

/// 绝对路径 → `~/…` 形式（只折叠 HOME 前缀，**不折叠中段**）。
///
/// 折叠边界必须落在路径分隔符上：裸 `starts_with` 会把 HOME 的同名前缀
/// 兄弟目录也折叠掉——HOME=`/Users/al` 时 `/Users/al-data` 会变成 `~-data`，
/// 一个并不存在的 HOME 相对路径。分隔符判定走 `std::path::is_separator`：
/// Windows 上 `\` 与 `/` 都算（跨平台语义一致）。
pub fn tilde_path(p: &str) -> String {
    tilde_path_with(p, cached_home())
}

/// [`tilde_path`] 的可测本体：home 由调用方注入（生产传缓存的本机 HOME，
/// 测试传固定值），折叠判定不依赖真实环境变量。
pub(crate) fn tilde_path_with(p: &str, home: Option<&str>) -> String {
    match home {
        Some(home) => match p.strip_prefix(home) {
            // 前缀命中后，剩余部分要么为空（p 就是 HOME 本身），要么以
            // 分隔符开头（HOME 下的子路径）——两者才可折叠
            Some(rest) if rest.is_empty() || rest.starts_with(std::path::is_separator) => {
                format!("~{rest}")
            }
            _ => p.to_string(),
        },
        // 无 HOME（env 缺失）：原样返回，不猜
        None => p.to_string(),
    }
}
