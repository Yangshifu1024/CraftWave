//! 文本工具函数测试（gpui-rewrite 2.C-3，rendering.md「文本工具函数」验收）。
//!
//! 断言矩阵对照 Wake 蓝本单测（`crates/wake/src/format.rs`、
//! `crates/wake-core/src/text.rs`），并补「正好卡预算」的边界档。

use chrono::{DateTime, Duration, Local, TimeZone};

use crate::text_util::{clip_display, one_line, smart_time_from, tilde_path_with};

/// 固定测试锚点（本地时区某个无歧义时刻，Wake 同款 helper）。
fn local_time(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
    Local
        .with_ymd_and_hms(year, month, day, hour, minute, 0)
        .single()
        .expect("测试日期在本地时区应当无歧义")
}

// ======================================================================
// clip_display：显示单元宽度截断
// ======================================================================

/// CJK 按 2 单元、拉丁与空格按 1 单元混合计数（Wake 同款三断言）。
#[test]
fn clip_display_counts_cjk_and_latin_widths() {
    // "Wake " = 5 + "会" = 2 + "话" = 2 → 总宽 9，恰好卡预算：原样返回
    assert_eq!(clip_display("Wake 会话", 9), "Wake 会话");
    // 总宽 13 > 9：内容预算 8 装 "Wake 会"（5+2=7），"话"放不下
    assert_eq!(clip_display("Wake 会话标题", 9), "Wake 会…");
    // 全 CJK 宽 12 > 7：内容预算 6 装 "超长会"（2×3）
    assert_eq!(clip_display("超长会话标题", 7), "超长会…");
}

/// 组合字符是一个字素簇（e + U+0301 宽 1、不可拆），极小预算边界（Wake 同款）。
#[test]
fn clip_display_respects_zero_width_marks_and_tiny_limits() {
    // 内容预算 3："e\u{301}"(1) + "c"(1) + "l"(1)，"a" 放不下——组合符不劈半
    let combined = "e\u{301}clair";
    assert_eq!(clip_display(combined, 4), "e\u{301}cl…");
    // 预算 1：内容预算 0，任何非空文本都放不下，只剩省略号
    assert_eq!(clip_display("abc", 1), "…");
    // 预算 0：连省略号（宽 1）都装不下 → 空串
    assert_eq!(clip_display("abc", 0), "");
}

/// ZWJ emoji 与肤色修饰 emoji 各是一个字素簇，宽度按 unicode-width 0.2 的
/// 「well-formed 序列计 2」规则，永不劈半（Wake 同款断言）。
#[test]
fn clip_display_never_splits_emoji_grapheme_clusters() {
    // 家庭 emoji（4 人 ZWJ 序列）整体宽 2：预算 3 时簇内放进（2 ≤ 内容预算 2），
    // "x" 放不下 → 整簇保留 + 省略号
    let family = "👩‍👩‍👧‍👦";
    assert_eq!(
        clip_display(&format!("{family}xy"), 3),
        format!("{family}…")
    );
    // 肤色修饰握手（emoji + 肤色修饰符）整体宽 2，同理
    let toned = "👍🏽";
    assert_eq!(clip_display(&format!("{toned}xy"), 3), format!("{toned}…"));
}

/// 正好卡预算的边界档：总宽恰好等于预算 → 原样返回不加省略号；
/// 超出一个单元也要截（预算减省略号后装箱）。
#[test]
fn clip_display_exact_budget_boundary_stays_unclipped() {
    // 全拉丁 8 字符，预算 8：恰好装下，不加省略号
    assert_eq!(clip_display("abcdefgh", 8), "abcdefgh");
    // 9 > 8：内容预算 7 → 7 字符 + 省略号
    assert_eq!(clip_display("abcdefghi", 8), "abcdefg…");
    // CJK 恰好卡预算：4 个汉字宽 8，预算 8 → 原样
    assert_eq!(clip_display("四个汉字", 8), "四个汉字");
    // 空串在任何预算下原样返回（宽 0 ≤ 预算）
    assert_eq!(clip_display("", 4), "");
}

// ======================================================================
// one_line：压行 + 按字符截断
// ======================================================================

/// 空白压成单空格 + 按字符数截断（Wake 同款断言）。
#[test]
fn one_line_flattens_and_clips_by_chars() {
    assert_eq!(one_line("  a\n\n b\tc ", 10), "a b c");
    assert_eq!(one_line("二维码扫描登录", 3), "二维码…");
    assert_eq!(one_line("abc", 3), "abc", "恰好等长不截");
}

/// 首行为空的文本也能取到内容（不是「取首行」——首行可能空）。
#[test]
fn one_line_skips_leading_blank_lines() {
    assert_eq!(one_line("\n\n真正的第一行内容", 7), "真正的第一行内…");
}

// ======================================================================
// smart_time：渐进式时间档（锚点注入，Wake 同款六档断言）
// ======================================================================

#[test]
fn smart_time_uses_progressive_precision() {
    let now = local_time(2026, 8, 26, 18, 0);
    // 一分钟内 → 刚刚
    assert_eq!(
        smart_time_from((now - Duration::seconds(30)).timestamp_millis(), &now),
        "刚刚"
    );
    // 一小时内 → N 分钟前
    assert_eq!(
        smart_time_from((now - Duration::minutes(23)).timestamp_millis(), &now),
        "23 分钟前"
    );
    // 今天更早 → 时刻
    assert_eq!(
        smart_time_from(local_time(2026, 8, 26, 15, 42).timestamp_millis(), &now),
        "3:42 PM"
    );
    // 昨天 → 标签
    assert_eq!(
        smart_time_from(local_time(2026, 8, 25, 12, 0).timestamp_millis(), &now),
        "昨天"
    );
    // 今年更早 → 月日
    assert_eq!(
        smart_time_from(local_time(2026, 8, 21, 12, 0).timestamp_millis(), &now),
        "Aug 21"
    );
    // 往年 → 月日带年份
    assert_eq!(
        smart_time_from(local_time(2025, 8, 21, 12, 0).timestamp_millis(), &now),
        "Aug 21, 2025"
    );
}

/// 无效时间戳（≤0）给空串，不显示「刚刚」假新鲜。
#[test]
fn smart_time_gives_empty_for_invalid_timestamps() {
    let now = local_time(2026, 8, 26, 18, 0);
    assert_eq!(smart_time_from(0, &now), "");
    assert_eq!(smart_time_from(-42, &now), "");
}

// ======================================================================
// tilde_path：HOME 折叠（分隔符边界）
// ======================================================================

#[test]
fn tilde_path_folds_only_on_separator_boundaries() {
    let home = Some("/Users/al");
    // HOME 本身 → ~
    assert_eq!(tilde_path_with("/Users/al", home), "~");
    // HOME 下的子路径 → ~/子路径
    assert_eq!(tilde_path_with("/Users/al/Code/x.rs", home), "~/Code/x.rs");
    // 同名前缀兄弟目录：不是 HOME 下的东西，不折叠（裸 starts_with 的坑）
    assert_eq!(tilde_path_with("/Users/al-data", home), "/Users/al-data");
    // 字符串前缀命中但无分隔符（/Users/alicerice）同样不折叠
    assert_eq!(
        tilde_path_with("/Users/alicerice/x", home),
        "/Users/alicerice/x"
    );
    // HOME 之外的路径原样
    assert_eq!(tilde_path_with("/tmp/build.log", home), "/tmp/build.log");
    // 无 HOME（env 缺失）原样返回，不猜
    assert_eq!(tilde_path_with("/Users/al/x", None), "/Users/al/x");
}

// ======================================================================
// greeting：时段问候分档（B1 空状态，阶段 3-A）
// ======================================================================

/// 时段分档：12 点前早上 / 18 点前下午 / 其后晚上（分档与 smart_time
/// 同源——都是本机 Local 时钟，可测本体注入小时数）。
#[test]
fn greeting_buckets_by_hour() {
    use crate::text_util::greeting_from;
    assert_eq!(greeting_from(0), "Good morning");
    assert_eq!(greeting_from(11), "Good morning");
    assert_eq!(greeting_from(12), "Good afternoon");
    assert_eq!(greeting_from(17), "Good afternoon");
    assert_eq!(greeting_from(18), "Good evening");
    assert_eq!(greeting_from(23), "Good evening");
}
