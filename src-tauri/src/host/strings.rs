//! Rust 侧用户可见文案表（31 号票，FR-13-09 + FR-14-04，§10.8）。
//!
//! 范围（票面定案的两类）：托盘菜单（`host/shell.rs` install_tray）与系统
//! 通知正文（`host/notify.rs` NotifyGate）。按 `config.ui.language` 查表——
//! 事件/通知触发时读最新配置缓存（ConfigStore 共享缓存，天然「下次触发即
//! 新语言」）；托盘菜单在 save_config 钩子处重建一次（shell.rs
//! [`apply_tray_language`]）。
//!
//! 内部诊断/日志串（非用户可见）不进本表——与前端 console 诊断串同边界。

use crate::providers::types::Language;

/// 文案键（托盘菜单 + 通知正文）。两语言表必须同键集——单测
/// [`tests::key_sets_match`] 钉死（票面验收）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Str {
    /// 托盘菜单：显示主窗口。
    TrayShow,
    /// 托盘菜单：退出 CraftWave。
    TrayQuit,
    /// 通知正文：run 完成（{secs} = 耗时秒数，一位小数）。
    NotifyRunDone,
    /// 通知正文：run 出错。
    NotifyRunError,
    /// 通知正文：run 已取消。
    NotifyRunCancelled,
    /// 通知正文：人工介入等待。
    NotifyAskOpened,
    /// 通知耗时后缀（（耗时 {secs} 秒））。
    NotifyElapsed,
}

/// zh-CN 表（基准包，从已落地 UI 抽词）。
const ZH: &[(Str, &str)] = &[
    (Str::TrayShow, "显示主窗口"),
    (Str::TrayQuit, "退出 CraftWave"),
    (Str::NotifyRunDone, "运行完成"),
    (Str::NotifyRunError, "运行出错"),
    (Str::NotifyRunCancelled, "运行已取消"),
    (Str::NotifyAskOpened, "等待你的回应（询问/审批）"),
    (Str::NotifyElapsed, "（耗时 {secs} 秒）"),
];

/// en-US 表（全量翻译；键集合与 ZH 一致——单测钉死）。
const EN: &[(Str, &str)] = &[
    (Str::TrayShow, "Show Main Window"),
    (Str::TrayQuit, "Quit CraftWave"),
    (Str::NotifyRunDone, "Run completed"),
    (Str::NotifyRunError, "Run failed"),
    (Str::NotifyRunCancelled, "Run cancelled"),
    (
        Str::NotifyAskOpened,
        "Waiting for your response (question/approval)",
    ),
    (Str::NotifyElapsed, " (took {secs}s)"),
];

/// 查词：按语言与键取文案（`{secs}` 占位由调用方替换——耗时是运行数据）。
/// 未知键按编译期枚举不可能发生；表缺失时回退 zh 基准（防御性，键集单测
/// 保证不触发）。
pub fn t(key: Str, lang: Language) -> &'static str {
    let table = match lang {
        Language::ZhCn => ZH,
        Language::EnUs => EN,
    };
    table
        .iter()
        .find(|(k, _)| *k == key)
        .or_else(|| ZH.iter().find(|(k, _)| *k == key))
        .map(|(_, text)| *text)
        .unwrap_or("")
}

/// 通知正文组装：动作词 + 耗时后缀（`{secs}` 占位替换为真实秒数）。
pub fn run_ended_body(reason: &str, duration_secs: f64, lang: Language) -> String {
    let verb = match reason {
        "done" => t(Str::NotifyRunDone, lang),
        "error" => t(Str::NotifyRunError, lang),
        _ => t(Str::NotifyRunCancelled, lang),
    };
    let elapsed = t(Str::NotifyElapsed, lang).replace("{secs}", &format!("{duration_secs:.1}"));
    format!("{verb}{elapsed}")
}

#[cfg(test)]
#[path = "strings_test.rs"]
mod strings_test;
