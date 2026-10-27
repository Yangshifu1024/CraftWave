//! 路径边界 safe_join（FR-05-06，§9.1）。
//!
//! 判定链：拒绝空/控制字符/Windows 保留名 → 解析绝对路径（相对路径相对
//! 项目主目录）→ 规范化并解析符号链接 → canonical 前缀 ∈ roots。
//! `full_access` 档不解除本检查（由调用方纪律保证，本模块无法感知档位）。
//!
//! M1 roots = 项目主目录 + 数据目录（§9.1，无 extra_roots）。

use std::path::{Component, Path, PathBuf};

use crate::util::errors::ToolError;

/// canonical 后的根集合 + 项目主目录。构造时对 roots 做 canonicalize，
/// 之后 [`PathGuard::resolve`] 可反复调用。
#[derive(Debug, Clone)]
pub struct PathGuard {
    /// canonical 后的 roots（含项目主目录与数据目录）。
    roots: Vec<PathBuf>,
    /// canonical 后的项目主目录（相对路径的解析基准）。
    project_root: PathBuf,
}

// PathGuard 的构造与路径解析方法。
impl PathGuard {
    /// roots 必须真实存在（canonicalize 失败即参数错误）。
    pub fn new(roots: Vec<PathBuf>) -> Result<Self, ToolError> {
        if roots.is_empty() {
            return Err(ToolError::invalid_arg("roots 不能为空"));
        }
        let mut canonical = Vec::with_capacity(roots.len());
        for r in &roots {
            canonical.push(
                std::fs::canonicalize(r)
                    .map_err(|e| ToolError::invalid_arg(format!("root {:?} 无法解析：{e}", r)))?,
            );
        }
        // 首个 root 约定为项目主目录（§9.1：相对路径相对项目主目录）。
        let project_root = canonical[0].clone();
        Ok(Self {
            roots: canonical,
            project_root,
        })
    }

    /// 把用户给定的路径解析到 roots 内，返回可用于 IO 的绝对路径。
    /// 越界返回 E_PATH_OUTSIDE_ROOTS；空/控制字符/保留名返回 E_INVALID_ARG。
    pub fn resolve(&self, path: &str) -> Result<PathBuf, ToolError> {
        // 空路径与控制字符直接拒绝。
        if path.is_empty() {
            return Err(ToolError::invalid_arg("路径为空"));
        }
        if path.chars().any(char::is_control) {
            return Err(ToolError::invalid_arg(format!(
                "路径含控制字符或 NUL：{path:?}"
            )));
        }
        // Windows 保留名跨平台拒绝（CON/PRN/AUX/NUL/COM1-9/LPT1-9，含带扩展名形态）。
        for comp in Path::new(path).components() {
            if let Component::Normal(os) = comp
                && let Some(name) = os.to_str()
                && is_windows_reserved(name)
            {
                return Err(ToolError::invalid_arg(format!(
                    "路径含 Windows 保留名：{name}"
                )));
            }
        }

        // 解析为绝对路径：相对路径相对项目主目录。
        let p = Path::new(path);
        let absolute = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.project_root.join(p)
        };

        // canonicalize 语义：解析符号链接。文件尚不存在（write/edit 场景）
        // 时对最长存在前缀做 canonicalize，剩余段做词法归一——剩余段
        // 不存在故无 symlink，`..` 逃逸由最终前缀检查拦截。
        let resolved = canonicalize_best_effort(&absolute)?;

        // canonical 前缀 ∈ roots 的 canonical 集合（组件级前缀比较）。
        if self
            .roots
            .iter()
            .any(|root| &resolved == root || resolved.starts_with(root))
        {
            Ok(resolved)
        } else {
            Err(ToolError::path_outside_roots(format!(
                "路径越界（不在 roots 内）：{path}"
            )))
        }
    }
}

/// Windows 保留名（§9.1）。stem 判定：CON.txt 同样非法。
fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("");
    matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// 整条路径存在则 canonicalize；否则对最长存在前缀 canonicalize +
/// 剩余段词法归一（ParentDir 在已解析基上弹栈、CurDir 去除；不存在段
/// 无 symlink，`..` 逃逸由最终前缀检查拦截）。
fn canonicalize_best_effort(absolute: &Path) -> Result<PathBuf, ToolError> {
    if let Ok(c) = std::fs::canonicalize(absolute) {
        return Ok(c);
    }
    // 自底向上试最长存在前缀（split 从尾部向头部滑动）。
    let all: Vec<Component<'_>> = absolute.components().collect();
    for split in (1..=all.len()).rev() {
        let prefix: PathBuf = all[..split].iter().collect();
        let Ok(canonical_prefix) = std::fs::canonicalize(&prefix) else {
            continue;
        };
        let mut resolved = canonical_prefix;
        for comp in &all[split..] {
            match comp {
                Component::ParentDir => {
                    resolved.pop();
                }
                Component::Normal(s) => resolved.push(s),
                Component::CurDir => {}
                _ => {
                    return Err(ToolError::invalid_arg(format!(
                        "路径组件非法：{absolute:?}"
                    )));
                }
            }
        }
        return Ok(resolved);
    }
    Err(ToolError::invalid_arg(format!(
        "路径无法解析：{absolute:?}"
    )))
}

#[cfg(test)]
#[path = "safe_join_test.rs"]
mod safe_join_test;
