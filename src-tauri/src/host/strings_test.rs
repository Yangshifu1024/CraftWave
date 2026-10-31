//! host/strings.rs 单测（31 号票）：两语言表键集一致性（票面验收「两表 key
//! 集合一致性单测钉死」）+ 查词与通知正文组装冒烟 + 无 CJK 抽查（en 面）。

use super::*;

/// 两语言表键集合一致（双向 diff 为空）——票面验收钉死项。
#[test]
fn key_sets_match() {
    let zh_keys: std::collections::HashSet<_> = ZH.iter().map(|(k, _)| *k).collect();
    let en_keys: std::collections::HashSet<_> = EN.iter().map(|(k, _)| *k).collect();
    let missing_in_en: Vec<_> = zh_keys.difference(&en_keys).collect();
    let extra_in_en: Vec<_> = en_keys.difference(&zh_keys).collect();
    assert!(missing_in_en.is_empty(), "en 表缺键：{missing_in_en:?}");
    assert!(extra_in_en.is_empty(), "en 表多键：{extra_in_en:?}");
}

/// zh 表无空文案（基准包完整性）。
#[test]
fn zh_texts_non_empty() {
    for (key, text) in ZH {
        assert!(!text.is_empty(), "{key:?} 的 zh 文案为空");
    }
}

/// 查词冒烟：zh/en 各取一条；en 文案无 CJK（用户可见面英文化抽查）。
#[test]
fn lookup_and_no_cjk_in_en() {
    assert_eq!(t(Str::TrayShow, Language::ZhCn), "显示主窗口");
    assert_eq!(t(Str::TrayShow, Language::EnUs), "Show Main Window");
    for (_, text) in EN {
        assert!(
            !text.chars().any(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)),
            "en 表含 CJK：{text}"
        );
    }
}

/// 通知正文组装：done/error/cancelled 三动作词 + 耗时插值，双语各一例。
#[test]
fn run_ended_body_bilingual() {
    let zh = run_ended_body("done", 12.34, Language::ZhCn);
    assert_eq!(zh, "运行完成（耗时 12.3 秒）");
    let en = run_ended_body("error", 3.0, Language::EnUs);
    assert_eq!(en, "Run failed (took 3.0s)");
    let cancelled = run_ended_body("cancelled", 1.0, Language::ZhCn);
    assert_eq!(cancelled, "运行已取消（耗时 1.0 秒）");
}
