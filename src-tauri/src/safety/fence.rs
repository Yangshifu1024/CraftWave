//! 命令围栏 fence v0.1（FR-05-05 P0 部分，§8.7）——纯判定逻辑。
//!
//! 1. 黑名单（正则集）命中即 Block，任何档位均拦截（§8.5 灾难级）。
//! 2. plan 档只读白名单：每段命中方放行，否则 Confirm。
//! 3. 非 plan 档写信号启发：重定向/tee/sed -i/包管理安装等 → Confirm。
//! 4. fail-closed：规则集编译异常（不应发生）时恒 Confirm，绝不 fail-open。
//!    AST 管线（tree-sitter）为 P1，本实现是词法级 v0.1。
//!
//! 词法局限（保守方向，不构成放行风险）：引号内的 `|`、`>` 也参与判定，
//! 可能产生误拦（Confirm），绝不误放。

use regex::Regex;

use super::tier::PermissionTier;

/// 围栏判定结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FenceVerdict {
    /// 围栏不拦（最终能否执行仍由 §8.5 矩阵合成）。
    Allow,
    /// 需用户审批。
    Confirm,
    /// 硬拦（E_FENCE_BLOCKED，任何档位不豁免）。
    Block,
}

/// 黑名单正则集（§8.7 规则 1）。在整条命令文本上匹配。
/// 较文档字面更保守的扩展见 11 号票 Answer。
const BLACKLIST: &[&str] = &[
    // rm 递归删除（-rf/-fr/-Rf 等短旗标组合与 --recursive）。
    r"\brm\s+-[a-zA-Z]*[rR][a-zA-Z]*\b",
    r"\brm\b[^|;&]*--recursive\b",
    // 提权。
    r"\bsudo\b",
    r"\bsu\b",
    // 关机/重启同族。
    r"\b(?:shutdown|reboot|halt|poweroff)\b",
    // 格式化/裸设备写入。
    r"\bmkfs",
    r"\bdd\b[^|;&]*\bof=/dev/",
    r">\s*/dev/(?:sd|hd|nvme|disk)",
    r"(?i)\bformat\s+[a-z]:",
    // 下载即执行。
    r"\b(?:curl|wget)\b[^|;&]*\|\s*(?:sudo\s+)?(?:ba|z|da|k)?sh\b",
    // 强推。
    r"\bgit\s+push\b[^|;&]*(?:--force\b|-f\b)",
    // 全盘放开权限。
    r"\bchmod\b[^|;&]*777\s+/",
    // Windows 递归删除。
    r"(?i)\brd\s+/s\b",
];

/// 写信号启发（§8.7 规则 3，非 plan 档）。重定向 `>` 单独子串判定。
const WRITE_SIGNALS: &[&str] = &[
    r"\btee\b",
    r"\bsed\b[^|;&]*\s-i\b",
    r"\bnpm\s+(?:i\b|install\b|ci\b)",
    r"\bpnpm\s+(?:i\b|install\b|add\b)",
    r"\byarn\s+(?:add\b|install\b)",
    r"\bpip3?\s+install\b",
    r"\bmake\b",
    r"\bcargo\s+(?:build|install)\b",
];

/// 围栏判定器。构造一次、判定多次；规则集编译失败时整体退化为恒 Confirm。
pub struct Fence {
    blacklist: Option<Vec<Regex>>,
    write_signals: Option<Vec<Regex>>,
}

// Fence 的构造与判定方法。
impl Fence {
    /// 编译黑名单与写信号两套正则（编译失败时对应集合为 None → 判定恒 Confirm）。
    pub fn new() -> Self {
        Self {
            blacklist: compile_all(BLACKLIST),
            write_signals: compile_all(WRITE_SIGNALS),
        }
    }

    /// 对一条命令文本做围栏判定（§8.7 规则 1→2→3）。
    pub fn judge(&self, command: &str, tier: PermissionTier) -> FenceVerdict {
        // 规则 4 fail-closed：规则集未成功编译 → 恒 Confirm。
        let (Some(blacklist), Some(signals)) = (&self.blacklist, &self.write_signals) else {
            return FenceVerdict::Confirm;
        };
        // 规则 1：黑名单命中即 Block（任何档位，§8.5 灾难级）。
        if blacklist.iter().any(|re| re.is_match(command)) {
            return FenceVerdict::Block;
        }
        let segments = split_segments(command);
        match tier {
            // 规则 2：plan 档只读白名单，每段都须命中。
            PermissionTier::Plan => {
                if segments.iter().all(|s| is_readonly_whitelisted(s)) {
                    FenceVerdict::Allow
                } else {
                    FenceVerdict::Confirm
                }
            }
            // 规则 3：写信号启发（confirm_each/auto_edit/full_access 同判；
            // 结论与 §8.5 矩阵由调用方合成——Confirm 表示须经审批）。
            _ => {
                if segments.iter().any(|s| has_write_signal(s, signals)) {
                    FenceVerdict::Confirm
                } else {
                    FenceVerdict::Allow
                }
            }
        }
    }
}

// Default 与 new 等价（正则集在 new 里编译）。
impl Default for Fence {
    fn default() -> Self {
        Self::new()
    }
}

/// 批量编译正则：任意一条编译失败则整体返回 None（触发上层 fail-closed）。
fn compile_all(patterns: &[&str]) -> Option<Vec<Regex>> {
    patterns
        .iter()
        .map(|p| Regex::new(p).ok())
        .collect::<Option<Vec<_>>>()
}

/// 词法分段：`&&`、`||`、`&`、`;`、`|`、换行处切开。不解析引号（保守）。
fn split_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ';' | '\n' | '\r' => {
                segments.push(std::mem::take(&mut current));
            }
            '|' | '&' => {
                segments.push(std::mem::take(&mut current));
                // 连字符（&&/||）消费掉第二个；单字符本身就是分隔。
                if chars.peek() == Some(&c) {
                    chars.next();
                }
            }
            _ => current.push(c),
        }
    }
    segments.push(current);
    segments.retain(|s| !s.trim().is_empty());
    segments
}

/// plan 档只读白名单（§8.7 规则 2）：ls/cat/head/tail/wc/find/rg/grep、
/// git 只读子命令、node/python --version。
fn is_readonly_whitelisted(segment: &str) -> bool {
    let argv: Vec<&str> = segment.split_whitespace().collect();
    let Some(head) = argv.first() else {
        return false;
    };
    let cmd = head.rsplit('/').next().unwrap_or(head);
    match cmd {
        "ls" | "cat" | "head" | "tail" | "wc" | "find" | "rg" | "grep" => true,
        "git" => argv
            .get(1)
            .is_some_and(|sub| matches!(*sub, "status" | "diff" | "log" | "show")),
        "node" | "python" | "python3" => argv.len() == 2 && matches!(argv[1], "--version" | "-V"),
        _ => false,
    }
}

/// 写信号判定：重定向（含 >>、2>）或启发正则命中。
fn has_write_signal(segment: &str, signals: &[Regex]) -> bool {
    segment.contains('>') || signals.iter().any(|re| re.is_match(segment))
}

#[cfg(test)]
#[path = "fence_test.rs"]
mod fence_test;
