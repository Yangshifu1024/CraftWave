//! 命令围栏 fence——纯判定逻辑。
//!
//! 1. 黑名单（正则集）命中即 Block，任何档位均拦截（灾难级命令）。
//! 2. plan 档只读白名单：每段命中方放行，否则 Confirm（段边界由 AST
//!    提供——v0.1 词法切段的引号误拦已消除，白名单语义不变）。
//! 3. 非 plan 档：AST 写目标分析（tree-sitter bash/PowerShell，见
//!    [`ast`] 子模块）——提取真正的写目标
//!    做四象限判定；写信号词法启发（v0.1 规则 3 的重定向/tee/sed 判定）
//!    已由 AST 接管，安装类命令（npm/pip/cargo 等会写工作区文件）保留
//!    词法信号防放松。
//! 4. fail-closed：规则集编译异常 / AST 解析失败或超限 → 恒 Confirm，
//!    绝不 fail-open（宁可误拦走人工确认，不可漏拦）。
//!
//! 判定输入两档：
//! - [`Fence::judge`]：无工作区上下文（无 roots）——有写目标即 Confirm
//!   （无法判界，保守方向）；
//! - [`Fence::judge_in_workspace`]：带 roots 与相对路径基准——对写目标做
//!   四象限判定（Allow/Confirm/Block + 人话原因）。

use regex::Regex;

use super::tier::PermissionTier;

pub mod ast;

/// 围栏判定结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FenceVerdict {
    /// 围栏不拦（最终能否执行仍由能力矩阵合成）。
    Allow,
    /// 需用户审批。
    Confirm,
    /// 硬拦（E_FENCE_BLOCKED，任何档位不豁免）。
    Block,
}

/// 判定结论 + 人话原因（审批挂起载荷的 fenceReason 展示给用户；
/// 原因随 AST 判定更具体，如「重定向目标越出工作区（symlink 逃逸）」）。
#[derive(Debug, Clone, PartialEq)]
pub struct FenceOutcome {
    pub verdict: FenceVerdict,
    /// 命中原因（None = Allow 或纯权限矩阵审批）。
    pub reason: Option<String>,
}

impl FenceOutcome {
    fn allow() -> Self {
        Self {
            verdict: FenceVerdict::Allow,
            reason: None,
        }
    }

    fn confirm(reason: impl Into<String>) -> Self {
        Self {
            verdict: FenceVerdict::Confirm,
            reason: Some(reason.into()),
        }
    }

    fn block(reason: impl Into<String>) -> Self {
        Self {
            verdict: FenceVerdict::Block,
            reason: Some(reason.into()),
        }
    }
}

/// 黑名单正则集（规则 1）。在整条命令文本上匹配；
/// 覆盖面较常见清单更保守（如 `su`、`--force-with-lease` 一并拦截）。
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

/// 安装类信号（语义已收窄：v0.1 规则 3 的重定向/tee/sed -i 判定
/// 已由 AST 写目标分析接管，本集仅保留「会修改工作区文件」的安装类命令
/// 词法信号防放松——npm install 写 node_modules、cargo build 写 target 等，
/// 这些命令无 shell 级重定向语法，AST 写目标清单覆盖不到）。
const WRITE_SIGNALS: &[&str] = &[
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
    /// 编译黑名单与安装类信号两套正则（编译失败时对应集合为 None → 判定恒 Confirm）。
    pub fn new() -> Self {
        Self {
            blacklist: compile_all(BLACKLIST),
            write_signals: compile_all(WRITE_SIGNALS),
        }
    }

    /// 对一条命令文本做围栏判定（规则 1→2→3，无工作区上下文：
    /// 发现写目标即 Confirm，无法做四象限判界）。
    pub fn judge(&self, command: &str, tier: PermissionTier) -> FenceVerdict {
        self.judge_in_workspace(command, tier, ast::Shell::host_default(), None)
            .verdict
    }

    /// 带工作区上下文的完整判定：对 AST 提取的写目标做四象限判定，
    /// 结论附人话原因（审批挂起载荷 fenceReason 用）。
    ///
    /// - `shell`：命令的执行 shell（与 command 工具执行形态同源）；
    /// - `ws`：None = 无 roots（退化语义：有写目标即 Confirm）。
    pub fn judge_in_workspace(
        &self,
        command: &str,
        tier: PermissionTier,
        shell: ast::Shell,
        ws: Option<&ast::Workspace>,
    ) -> FenceOutcome {
        // 规则 4 fail-closed：规则集未成功编译 → 恒 Confirm。
        let (Some(blacklist), Some(signals)) = (&self.blacklist, &self.write_signals) else {
            return FenceOutcome::confirm("围栏规则集未就绪，按保守策略需确认");
        };
        // 规则 1：黑名单命中即 Block（任何档位，灾难级命令）。
        if blacklist.iter().any(|re| re.is_match(command)) {
            return FenceOutcome::block(format!("灾难级命令，任何档位不豁免：{command}"));
        }
        // AST 解析（解析失败/超限 → fail-closed Confirm）。
        let ast::ParsedCommand::Ok { argvs, targets } = ast::parse_command(shell, command) else {
            return FenceOutcome::confirm("命令解析失败，按保守策略需确认（不可漏拦）");
        };
        match tier {
            // 规则 2：plan 档只读白名单（语义不变：只放白名单内且无写目标
            // 的纯只读命令；段边界由 AST 提供，引号内 `|` 不再误切）。
            PermissionTier::Plan => {
                if !argvs.is_empty()
                    && argvs.iter().all(|argv| is_readonly_whitelisted(argv))
                    && targets.is_empty()
                {
                    FenceOutcome::allow()
                } else {
                    FenceOutcome::confirm("命令不在 plan 档只读白名单内")
                }
            }
            // 规则 3：AST 写目标分析（confirm_each/auto_edit/full_access
            // 同判；结论与能力矩阵由调用方合成——Confirm 表示须经审批）。
            _ => {
                if !targets.is_empty() {
                    return match ws {
                        Some(ws) => {
                            let v = ast::judge_targets(&targets, ws);
                            FenceOutcome {
                                verdict: v.verdict,
                                reason: v.reason,
                            }
                        }
                        // 无工作区上下文：无法判界，全部按 Confirm（保守）。
                        None => FenceOutcome::confirm("命令含写目标，需确认"),
                    };
                }
                if signals.iter().any(|re| re.is_match(command)) {
                    return FenceOutcome::confirm("安装类命令会修改工作区文件，需确认");
                }
                FenceOutcome::allow()
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

/// plan 档只读白名单（规则 2）：ls/cat/head/tail/wc/find/rg/grep、
/// git 只读子命令、node/python --version。输入为 AST 提取的命令段 argv。
/// 命令名不区分大小写并取 basename（PowerShell cmdlet 面与 `/bin/ls`
/// 形态统一归一；白名单语义不变）。
fn is_readonly_whitelisted(argv: &[String]) -> bool {
    let Some(head) = argv.first() else {
        return false;
    };
    let cmd = ast::basename(head).to_ascii_lowercase();
    match cmd.as_str() {
        "ls" | "cat" | "head" | "tail" | "wc" | "find" | "rg" | "grep" => true,
        "git" => argv
            .get(1)
            .is_some_and(|sub| matches!(sub.as_str(), "status" | "diff" | "log" | "show")),
        "node" | "python" | "python3" => {
            argv.len() == 2 && matches!(argv[1].as_str(), "--version" | "-V")
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "fence_test.rs"]
mod fence_test;
