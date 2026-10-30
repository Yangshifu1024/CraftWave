//! 命令围栏 AST 写目标分析（FR-05-05 P1，§8.7【M2】AST 管线，22 号票）。
//!
//! 把围栏的写信号判定从「词法猜」升级为「语法看」：用 tree-sitter 解析命令
//! 文本，从语法树里提取**真正的写目标**（重定向 / tee / sed -i / rm、mv、
//! chmod、chown / cp、install / mkfs / dd of=），再做四象限判定。
//!
//! 相比词法启发（fence v0.1 规则 3）的改进：
//! - 引号里的 `>` / `|` 不再参与判定（v0.1 的已知误拦消除——字符串是
//!   `string` 节点的子结构，不会被误认成语法层面的重定向/管道）；
//! - 变量拼接的写路径不再漏放（`$HOME/x` 出现在写目标里 → 字面量掩码命中
//!   → 保守 Confirm，防变量逃逸）；
//! - 解析失败 / 超限 → **fail-closed 按 Confirm**（非 fail-open，宁可误拦
//!   走人工确认，不可漏拦）。
//!
//! 结构约定（22 号票）：
//! - 纯判定核心 + 可注入文件系统解析器（[`Canonicalizer`]）——沿 12 号票
//!   ssrf.rs 先例，测试可 mock symlink 行为；
//! - [`WriteTarget`] 提取产物（目标路径 + 类别）是 23 号票（内部状态保护）
//!   的复用接缝：接口在本票定形，23 号票消费目标列表做禁区命中判定；
//! - [`git_classify`] 是 26 号票（git 集成）的复用接缝：纯函数表驱动，
//!   策略开关消费此函数。
//!
//! 判定口径（§8.7 四象限，字段级定义见技术设计文档）：
//!
//! | 词法位置 | canonical 位置 | 结论 |
//! |---|---|---|
//! | roots 内 | roots 内 | Allow（最终仍由 §8.5 矩阵合成） |
//! | roots 内 | 逃逸（symlink） | Confirm |
//! | roots 外 | roots 内 | Confirm（保守，少见形态） |
//! | roots 外 | roots 外 | 删除类 Block；其余 Confirm |
//! | 无法 canonicalize | — | Confirm |

use std::path::{Component, Path, PathBuf};

use tree_sitter::Node;

use super::FenceVerdict;

/// 解析防炸上限：单个命令文本的 AST 节点数上限（超限按解析失败 fail-closed）。
const MAX_NODES: usize = 4096;
/// 解析防炸上限：AST 深度上限（超深嵌套的命令替换/子 shell 递归按失败处理）。
const MAX_DEPTH: usize = 64;

/// command 工具的 shell 方言（执行形态：Unix `sh -c` / Windows
/// `powershell -NoProfile -Command`，见 tools/command.rs 的 prepare）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// POSIX shell（Unix 侧 sh -c）。
    Posix,
    /// Windows PowerShell。
    PowerShell,
}

impl Shell {
    /// 当前宿主的执行 shell（与 command 工具的实际执行形态同源）。
    pub fn host_default() -> Self {
        if cfg!(windows) {
            Shell::PowerShell
        } else {
            Shell::Posix
        }
    }
}

/// 写目标类别（§8.7 写目标提取清单）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteKind {
    /// 重定向 `>` / `>>`（含 `2>`、heredoc 目标）。
    Redirect,
    /// `tee`（管道分叉写文件）。
    Tee,
    /// `sed -i`（原地改写文件）。
    SedInPlace,
    /// `rm`（删除）。
    Delete,
    /// `mv`（移动：源被删，按删除类口径）。
    Move,
    /// `chmod` / `chown`（权限/属主变更）。
    MetaChange,
    /// `cp` / `install`（复制写）。
    Copy,
    /// `dd of=`（镜像/裸设备写）。
    RawWrite,
    /// `mkfs`（格式化；黑名单正则先行拦截，此处兜底）。
    Format,
    /// PowerShell 写入类 cmdlet（Set-Content/Add-Content/Out-File/
    /// New-Item/Tee-Object/Export-* 等，bash 无对应形态）。
    Set,
}

impl WriteKind {
    /// 删除类（`rm`/`mv`）：词法越界时直接 Block（§8.7 四象限）。
    pub fn is_destructive(self) -> bool {
        matches!(self, WriteKind::Delete | WriteKind::Move)
    }

    /// 人话类别名（挂起原因文案用）。
    pub fn label(self) -> &'static str {
        match self {
            WriteKind::Redirect => "重定向",
            WriteKind::Tee => "tee",
            WriteKind::SedInPlace => "sed -i 原地编辑",
            WriteKind::Delete => "删除",
            WriteKind::Move => "移动",
            WriteKind::MetaChange => "权限变更",
            WriteKind::Copy => "复制写",
            WriteKind::RawWrite => "dd 镜像写",
            WriteKind::Format => "格式化",
            WriteKind::Set => "文件写入",
        }
    }
}

/// 写目标路径的解析结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetPath {
    /// 字面量路径（可做四象限判定）。
    Literal(String),
    /// 含变量/命令替换/波浪号展开——字面量掩码命中，保守 Confirm
    /// （防 `$HOME/..` 逃逸：值未知时不猜）。
    Opaque,
}

/// 提取出的一个写目标。
///
/// 23 号票接缝：内部状态保护消费本结构的目标路径列表做禁区命中判定
/// （接口在本票定形）。36 号票（命令审计）消费判定原因区分「必须人工确认」
/// 与「可交审计定性的灰区」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteTarget {
    /// 写类别。
    pub kind: WriteKind,
    /// 目标路径（Literal 可判定 / Opaque 含未解析变量）。
    pub path: TargetPath,
    /// 命令里的原文片段（挂起原因展示用，如 `"$TARGET/file"`）。
    pub raw: String,
}

/// AST 解析与提取结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCommand {
    /// 解析成功：命令段 argv 清单（plan 白名单判定用）+ 写目标清单
    /// （空 = 语法层面未发现写信号）。
    Ok {
        /// 各命令段的 argv（argv[0] 为命令名，字符串已剥引号）。
        argvs: Vec<Vec<String>>,
        /// 提取出的写目标。
        targets: Vec<WriteTarget>,
    },
    /// 解析失败（语法 ERROR/MISSING）或超限（节点数/深度上限）——
    /// 调用方 fail-closed 按 Confirm 处理。
    Failed,
}

/// 解析一条命令并提取写目标（纯内存，不触文件系统）。
pub fn parse_command(shell: Shell, command: &str) -> ParsedCommand {
    // 每次判定局部构造 Parser（Language 为静态量，线程安全；审批路径
    // 非热点，单命令解析亚毫秒级）。
    let mut parser = tree_sitter::Parser::new();
    let lang = match shell {
        Shell::Posix => {
            let lang: tree_sitter::Language = tree_sitter_bash::LANGUAGE.into();
            lang
        }
        Shell::PowerShell => {
            let lang: tree_sitter::Language = tree_sitter_powershell::LANGUAGE.into();
            lang
        }
    };
    if parser.set_language(&lang).is_err() {
        return ParsedCommand::Failed;
    }
    let Some(tree) = parser.parse(command, None) else {
        return ParsedCommand::Failed;
    };
    let root = tree.root_node();
    // 语法错误（ERROR/MISSING 节点）→ fail-closed。
    if root.has_error() {
        return ParsedCommand::Failed;
    }
    let mut collector = Collector {
        budget: MAX_NODES,
        overflow: false,
        argvs: Vec::new(),
        targets: Vec::new(),
    };
    walk(root, shell, command, 0, &mut collector);
    if collector.overflow {
        return ParsedCommand::Failed;
    }
    ParsedCommand::Ok {
        argvs: collector.argvs,
        targets: collector.targets,
    }
}

/// 遍历累积器：节点预算 + 提取产物。
struct Collector {
    /// 剩余节点预算（防炸；耗尽即标记超限）。
    budget: usize,
    /// 超限标记（节点数或深度任一超限）。
    overflow: bool,
    /// 各命令段的 argv。
    argvs: Vec<Vec<String>>,
    /// 写目标清单。
    targets: Vec<WriteTarget>,
}

/// 深度优先遍历 AST：收集命令段 argv 与写目标。
fn walk(node: Node, shell: Shell, src: &str, depth: usize, acc: &mut Collector) {
    acc.budget = acc.budget.saturating_sub(1);
    if acc.budget == 0 || depth > MAX_DEPTH {
        acc.overflow = true;
        return;
    }
    match node.kind() {
        "command" => {
            let argv = command_argv(node, src, shell);
            if !argv.is_empty() {
                acc.argvs
                    .push(argv.iter().map(|a| a.text.clone()).collect());
                if let Some(targets) = command_write_targets(node, src, shell) {
                    acc.targets.extend(targets);
                }
            }
        }
        // bash：文件重定向（含 heredoc/多重定向形态，全树扫描统一覆盖）。
        "file_redirect" => {
            if let Some(target) = bash_redirect_target(node, src) {
                acc.targets.push(target);
            }
        }
        // PowerShell：重定向节点（> >> 2> 等；流合并 2>&1 在内部跳过）。
        "redirection" => {
            if let Some(target) = ps_redirect_target(node, src) {
                acc.targets.push(target);
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, shell, src, depth + 1, acc);
        if acc.overflow {
            return;
        }
    }
}

/// 一个命令参数项：字面文本 + 是否含未解析展开（字面量掩码的判定依据）。
#[derive(Debug, Clone)]
struct ArgItem {
    text: String,
    /// 子树含变量/命令替换/算术展开（`$VAR`/`$(...)`/`${...}`/`` `...` ``）。
    opaque: bool,
}

impl ArgItem {
    fn flag(&self) -> bool {
        self.text.starts_with('-')
    }
}

/// 目标路径的统一收口：opaque 参数 → TargetPath::Opaque；
/// 字面量参数以 `~` 开头（shell 会展开 home）也按 Opaque（保守）。
fn target_of(item: &ArgItem) -> TargetPath {
    if item.opaque || item.text.starts_with('~') {
        TargetPath::Opaque
    } else {
        TargetPath::Literal(item.text.clone())
    }
}

// ---- argv 提取（plan 档白名单判定用）----

/// 提取一个 command 节点的 argv（argv[0] = 命令名，参数按序）。
fn command_argv(node: Node, src: &str, shell: Shell) -> Vec<ArgItem> {
    let mut argv: Vec<ArgItem> = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match (shell, child.kind()) {
            // 命令名：取其文本（bash 是 word 子节点，PowerShell 直接文本）。
            (_, "command_name") => {
                argv.push(ArgItem {
                    text: basename(&child_text(child, src)),
                    opaque: false,
                });
            }
            // bash：参数词/字符串/拼接/展开；跳过重定向与变量赋值。
            (Shell::Posix, "file_redirect" | "herestring_redirect" | "heredoc_redirect")
            | (Shell::Posix, "variable_assignment") => {}
            // PowerShell：选项与参数 token 收进 argv，重定向节点跳过
            // （写目标由全树扫描统一提取）。
            (Shell::PowerShell, "redirection") => {}
            (Shell::PowerShell, "command_elements") => {
                let mut inner = child.walk();
                for token in child.children(&mut inner) {
                    if let Some(item) = ps_token_item(token, src) {
                        argv.push(item);
                    }
                }
            }
            // 其余命名子节点按参数项收（word/string/raw_string/
            // concatenation/simple_expansion/number/array…）。
            (_, kind) => {
                if is_named_param(kind)
                    && let Some(item) = posix_param_item(child, src)
                {
                    argv.push(item);
                }
            }
        }
    }
    argv
}

/// bash 侧命名参数类型（进 argv 的）。
fn is_named_param(kind: &str) -> bool {
    matches!(
        kind,
        "word"
            | "string"
            | "raw_string"
            | "concatenation"
            | "simple_expansion"
            | "special_variable_name"
            | "number"
            | "command_substitution"
            | "expansion"
            | "arithmetic_expansion"
            | "ansi_c_string"
            | "translated_string"
            | "array"
    )
}

/// bash 参数项：文本（string 剥外围引号）+ 展开判定。
fn posix_param_item(node: Node, src: &str) -> Option<ArgItem> {
    let text = quoted_off(node, src)?;
    Some(ArgItem {
        text,
        opaque: has_expansion(node),
    })
}

/// PowerShell 参数项：command_parameter → 选项（-Path 等）；
/// generic_token / array_literal_expression → 参数值。
fn ps_token_item(node: Node, src: &str) -> Option<ArgItem> {
    match node.kind() {
        "command_parameter" => Some(ArgItem {
            text: child_text(node, src).trim().to_string(),
            opaque: false,
        }),
        "generic_token" | "array_literal_expression" => {
            let text = child_text(node, src).trim().to_string();
            let opaque = has_expansion(node) || text.contains('$');
            Some(ArgItem { text, opaque })
        }
        _ => None,
    }
}

/// 节点文本（空安全）。
fn child_text(node: Node, src: &str) -> String {
    src.get(node.byte_range()).unwrap_or("").to_string()
}

/// 剥外围一层成对引号（`"x"` / `'x'`；bash string/raw_string 节点文本）。
/// 空/过短原文原样返回。
fn quoted_off(node: Node, src: &str) -> Option<String> {
    let text = src.get(node.byte_range())?;
    let kind = node.kind();
    let stripped = if matches!(kind, "string" | "raw_string" | "ansi_c_string")
        && text.len() >= 2
        && (text.starts_with('"') || text.starts_with('\'') || text.starts_with("$'"))
    {
        // $'...' 剥 $' 与尾'；"..." 与 '...' 剥首尾各一字符。
        if let Some(body) = text.strip_prefix("$'") {
            body.strip_suffix('\'').unwrap_or(body)
        } else {
            &text[1..text.len() - 1]
        }
    } else {
        text
    };
    Some(stripped.to_string())
}

/// 命令名取 basename（`/bin/ls` → `ls`；`C:\x\ls.exe` → `ls.exe` 保留，
/// 白名单侧再统一小写化）。`pub(crate)`：fence.rs 的 plan 白名单归一复用。
pub(crate) fn basename(name: &str) -> String {
    let trimmed = name.trim();
    trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(trimmed)
        .to_string()
}

/// 子树是否含展开类节点（变量/命令替换/算术/反引号）——字面量掩码依据。
fn has_expansion(node: Node) -> bool {
    if matches!(
        node.kind(),
        "simple_expansion" | "expansion" | "command_substitution" | "arithmetic_expansion"
    ) {
        return true;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if has_expansion(child) {
            return true;
        }
    }
    false
}

// ---- 写目标提取 ----

/// 按 shell 分派：命中写命令表则提取写目标。
fn command_write_targets(node: Node, src: &str, shell: Shell) -> Option<Vec<WriteTarget>> {
    let argv = command_argv(node, src, shell);
    let head = argv.first()?;
    let name = head.text.to_ascii_lowercase();
    let args = &argv[1..];
    match shell {
        Shell::Posix => posix_command_targets(&name, args),
        Shell::PowerShell => ps_command_targets(&name, args),
    }
}

/// 按 `--` 分隔把参数分为选项与位置参数（`--` 之后全部视为位置参数）。
fn split_flags(args: &[ArgItem]) -> (Vec<&ArgItem>, Vec<&ArgItem>) {
    let mut flags = Vec::new();
    let mut positional = Vec::new();
    let mut after_dashdash = false;
    for a in args {
        if after_dashdash {
            positional.push(a);
        } else if a.text == "--" {
            after_dashdash = true;
        } else if a.flag() {
            flags.push(a);
        } else {
            positional.push(a);
        }
    }
    (flags, positional)
}

/// bash 写命令表（§8.7 清单：tee、sed -i、rm/mv/chmod/chown、cp/install、
/// mkfs、dd of=）。提取不到目标时返回 None（该命令不产写目标）。
fn posix_command_targets(name: &str, args: &[ArgItem]) -> Option<Vec<WriteTarget>> {
    let (flags, positional) = split_flags(args);
    let targets_for = |kind: WriteKind, items: Vec<&ArgItem>| -> Vec<WriteTarget> {
        items
            .iter()
            .map(|a| WriteTarget {
                kind,
                path: target_of(a),
                raw: a.text.clone(),
            })
            .collect()
    };
    match name {
        // tee：全部位置参数都是写目标（`-a` 追加等选项除外；无文件参数 =
        // 只输出 stdout，无文件写）。
        "tee" if !positional.is_empty() => Some(targets_for(WriteKind::Tee, positional)),
        // sed -i：第一个位置参数是编辑脚本，其余为被改写文件。
        "sed"
            if flags.iter().any(|f| {
                f.text == "--in-place"
                    || f.text.starts_with("--in-place=")
                    || f.text.starts_with("-i")
            }) =>
        {
            let files = positional.iter().skip(1).copied().collect::<Vec<_>>();
            (!files.is_empty()).then(|| targets_for(WriteKind::SedInPlace, files))
        }
        // rm：全部位置参数是删除目标。
        "rm" if !positional.is_empty() => Some(targets_for(WriteKind::Delete, positional)),
        // mv：源与目标都收（源被删，按删除类口径）。
        "mv" if !positional.is_empty() => Some(targets_for(WriteKind::Move, positional)),
        // chmod/chown：第一个位置参数是模式/属主，其余是受影响文件。
        "chmod" | "chown" if positional.len() >= 2 => {
            let files = positional.iter().skip(1).copied().collect::<Vec<_>>();
            Some(targets_for(WriteKind::MetaChange, files))
        }
        // cp/install：目标 = 最后一个位置参数（源是读）。
        "cp" | "install" if positional.len() >= 2 => Some(targets_for(
            WriteKind::Copy,
            vec![positional[positional.len() - 1]],
        )),
        // dd of=<file>：of= 参数的值是写目标（of=/dev/… 由黑名单先行 Block）。
        "dd" => {
            let of = args
                .iter()
                .find(|a| a.text.starts_with("of=") && a.text.len() > 3)?;
            let value = ArgItem {
                text: of.text[3..].to_string(),
                opaque: of.opaque,
            };
            Some(targets_for(WriteKind::RawWrite, vec![&value]))
        }
        // mkfs 家族（mkfs.ext4 等）：设备参数。
        n if (n == "mkfs" || n.starts_with("mkfs.")) && !positional.is_empty() => {
            Some(targets_for(WriteKind::Format, vec![positional[0]]))
        }
        _ => None,
    }
}

/// PowerShell 写命令表：cmdlet 与常用别名（大小写已在调用处归一为小写）。
/// 写类 cmdlet 命中但提取不到任何目标参数 → 产 Opaque（保守 Confirm，
/// 不允许「认得是写命令却放行」）。
fn ps_command_targets(name: &str, args: &[ArgItem]) -> Option<Vec<WriteTarget>> {
    use WriteKind as K;
    const DELETE_CMDS: &[&str] = &["remove-item", "ri", "rm", "del", "erase", "rd", "rmdir"];
    // 复制/移动：目标 = 最后一个位置参数（源是读；移动源被删按删除类）。
    const COPY_CMDS: &[&str] = &["copy-item", "cp", "cpi", "copy"];
    const MOVE_CMDS: &[&str] = &["move-item", "mi", "mv", "move", "rename-item", "ren"];
    // 单路径写：目标 = 第一个位置参数（或 -Path/-LiteralPath/-FilePath 显式值）。
    const WRITE_CMDS: &[&str] = &[
        "set-content",
        "add-content",
        "out-file",
        "new-item",
        "ni",
        "mkdir",
        "md",
        "tee-object",
        "export-csv",
        "export-clixml",
        "clear-content",
        "clear-item",
        "cli",
        "set-itemproperty",
        "set-item",
        "si",
        "set-acl",
    ];
    let (_, positional) = split_flags(args);
    let make = |kind: WriteKind, items: Vec<&ArgItem>| -> Vec<WriteTarget> {
        items
            .iter()
            .map(|a| WriteTarget {
                kind,
                path: target_of(a),
                raw: a.text.clone(),
            })
            .collect()
    };
    // -Path/-LiteralPath/-FilePath/-Append 的显式跟随值（单路径写类用）。
    const PATH_FLAGS: &[&str] = &["-path", "-literapath", "-filepath", "-append"];
    let explicit_path = args.iter().enumerate().find_map(|(i, a)| {
        let is_path_flag = PATH_FLAGS
            .iter()
            .any(|pf| a.text.to_ascii_lowercase() == *pf);
        // 选项的直接后继若是位置参数即为路径值。
        args.get(i + 1).filter(|next| is_path_flag && !next.flag())
    });
    if DELETE_CMDS.contains(&name) {
        if positional.is_empty() {
            // 删除命令无参数 = 无操作。
            return None;
        }
        return Some(make(K::Delete, positional));
    }
    if COPY_CMDS.contains(&name) {
        return Some(match positional.last() {
            Some(last) => make(K::Copy, vec![last]),
            // 复制命令无参数：看不懂 → 保守 Opaque。
            None => vec![opaque_target(K::Copy)],
        });
    }
    if MOVE_CMDS.contains(&name) {
        return Some(match positional.last() {
            Some(last) => make(K::Move, vec![last]),
            None => vec![opaque_target(K::Move)],
        });
    }
    if WRITE_CMDS.contains(&name) {
        // 显式 -Path 优先；否则第一个位置参数；都无 → Opaque。
        let target = explicit_path
            .or(positional.first().copied())
            .map(|a| make(K::Set, vec![a]))
            .unwrap_or_else(|| vec![opaque_target(K::Set)]);
        return Some(target);
    }
    None
}

/// 「认得是写命令但提取不到目标」的保守兜底目标。
fn opaque_target(kind: WriteKind) -> WriteTarget {
    WriteTarget {
        kind,
        path: TargetPath::Opaque,
        raw: "<未能解析目标参数>".to_string(),
    }
}

/// bash file_redirect 节点：`>`/`>>`/`&>` 等为写；`<`（输入）与 fd 复制
/// （`2>&1`，目标纯数字）不是文件写。
fn bash_redirect_target(node: Node, src: &str) -> Option<WriteTarget> {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    let mut fd_seen = false;
    let mut op: Option<String> = None;
    for child in &children {
        match child.kind() {
            "file_descriptor" => fd_seen = true,
            // 操作符是匿名节点：kind 即文本（">" ">>" "<" ">&" "&>" ">|"…）。
            ">" | ">>" | "<" | ">&" | "&>" | ">|" | "<&" => op = Some(child.kind().to_string()),
            _ => {}
        }
    }
    let op = op?;
    // 输入重定向不是文件写。
    if op == "<" || op == "<&" {
        return None;
    }
    // 目标 = 第一个非 fd/操作符的命名子节点。
    let target_node = children.into_iter().find(|c| {
        !matches!(
            c.kind(),
            "file_descriptor" | ">" | ">>" | "<" | ">&" | "&>" | ">|" | "<&"
        )
    })?;
    let text = child_text(target_node, src).trim().to_string();
    // fd 复制（2>&1 的目标 "1"）不是文件。
    if fd_seen && !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let item = ArgItem {
        text,
        opaque: has_expansion(target_node),
    };
    Some(WriteTarget {
        kind: WriteKind::Redirect,
        path: target_of(&item),
        raw: item.text,
    })
}

/// PowerShell redirection 节点：`>`/`>>` 为写；`2>&1`（流合并，操作符含 &）
/// 与输入 `<` 不是文件写。
fn ps_redirect_target(node: Node, src: &str) -> Option<WriteTarget> {
    let mut cursor = node.walk();
    let mut op: Option<String> = None;
    let mut file: Option<String> = None;
    let mut file_node: Option<Node> = None;
    for child in node.children(&mut cursor) {
        match child.kind() {
            "file_redirection_operator" => op = Some(child_text(child, src)),
            "redirected_file_name" => {
                file = Some(child_text(child, src).trim().to_string());
                file_node = Some(child);
            }
            _ => {}
        }
    }
    let op = op?;
    if op.contains('&') || !op.contains('>') {
        return None;
    }
    let text = file?;
    let opaque = file_node.is_some_and(has_expansion) || text.contains('$');
    let item = ArgItem { text, opaque };
    Some(WriteTarget {
        kind: WriteKind::Redirect,
        path: target_of(&item),
        raw: item.text,
    })
}

// ---- git 子命令分类（26 号票接缝）----

/// git 子命令类别（26 号票策略开关消费本结论）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitClass {
    /// 只读（status/diff/log/show 等明确只读面）。
    ReadOnly,
    /// 写（commit/merge/rebase/push 等；未登记子命令的保守缺省）。
    Write,
    /// 破坏性（push --force 等；黑名单正则另有硬拦，此处供策略标注）。
    Destructive,
}

/// git 子命令分类表（§8.7：status/diff/log/show 只读；push/commit/merge/
/// rebase 写；push --force 归破坏性——fence 黑名单正则先行 Block）。
///
/// 输入契约：`argv[0]` 为 `git`（basename 归一后）；非 git 输入保守按
/// [`GitClass::Write`]（调用方须先确认 head 是 git）。
/// 未登记的子命令保守按 Write（宁可多审，不放行未知面）。
pub fn git_classify(argv: &[&str]) -> GitClass {
    let head = argv.first().map(|s| basename(s)).unwrap_or_default();
    if head != "git" {
        return GitClass::Write;
    }
    let sub = argv
        .get(1)
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    // push --force / -f / --force-with-lease（含前缀）归破坏性。
    if sub == "push"
        && argv[2..]
            .iter()
            .any(|a| a.starts_with("--force") || *a == "-f")
    {
        return GitClass::Destructive;
    }
    match sub.as_str() {
        "status" | "diff" | "log" | "show" | "rev-parse" | "blame" | "describe" | "shortlog"
        | "ls-files" | "cat-file" | "ls-tree" | "reflog" => GitClass::ReadOnly,
        "" => GitClass::ReadOnly, // 裸 git = 帮助输出，只读。
        _ => GitClass::Write,
    }
}

// ---- 四象限判定 ----

/// 文件系统 canonical 解析器（注入面：生产 [`FsCanonicalizer`]，测试 mock）。
pub trait Canonicalizer {
    /// 尽力解析：整路径存在则 canonicalize（解析 symlink）；不存在时对
    /// 最长存在前缀 canonicalize + 剩余段词法归一（与 safe_join 同思路，
    /// 22 号票注：剩余段不存在故无 symlink）；连根都无法解析 → None。
    fn canonical(&self, path: &Path) -> Option<PathBuf>;
}

/// 生产实现（真实文件系统）。
pub struct FsCanonicalizer;

impl Canonicalizer for FsCanonicalizer {
    fn canonical(&self, path: &Path) -> Option<PathBuf> {
        if let Ok(c) = std::fs::canonicalize(path) {
            return Some(c);
        }
        let all: Vec<Component<'_>> = path.components().collect();
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
                    _ => return None,
                }
            }
            return Some(resolved);
        }
        None
    }
}

/// 工作区判定上下文（judge_in_workspace 的输入）。
pub struct Workspace<'a> {
    /// canonical 后的允许根集合（项目主目录 + workspace 子目录，§9.1）。
    pub roots: &'a [PathBuf],
    /// 相对路径解析基准（command 的 cwd 解析后的绝对路径；缺省项目主目录）。
    pub base: &'a Path,
    /// 文件系统解析器。
    pub fs: &'a dyn Canonicalizer,
}

/// 单个写目标的判定结论（verdict + 人话原因，挂起载荷 fenceReason 用）。
#[derive(Debug, Clone, PartialEq)]
pub struct TargetVerdict {
    pub verdict: FenceVerdict,
    pub reason: Option<String>,
}

/// `path` 是否落在 `roots` 内（组件级前缀比较）。
fn path_within(path: &Path, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .any(|root| path == root || path.starts_with(root))
}

/// 单个写目标的四象限判定（§8.7 字段级口径）。
pub fn judge_target(target: &WriteTarget, ws: &Workspace) -> TargetVerdict {
    // 字面量掩码：写路径含未解析变量 → 保守 Confirm（防 $HOME/.. 逃逸）。
    let TargetPath::Literal(literal) = &target.path else {
        return TargetVerdict {
            verdict: FenceVerdict::Confirm,
            reason: Some(format!(
                "{}目标含未解析变量，需确认：{}",
                target.kind.label(),
                target.raw
            )),
        };
    };
    // 词法绝对化：绝对路径直接用；相对路径基于 cwd 基准。
    let p = Path::new(literal);
    let lexical = if p.is_absolute() {
        p.to_path_buf()
    } else {
        ws.base.join(p)
    };
    let lex_in = path_within(&lexical, ws.roots);
    let Some(canonical) = ws.fs.canonical(&lexical) else {
        // 无法 canonicalize（目标不存在且父链异常）→ Confirm。
        return TargetVerdict {
            verdict: FenceVerdict::Confirm,
            reason: Some(format!(
                "{}目标路径无法解析，需确认：{}",
                target.kind.label(),
                target.raw
            )),
        };
    };
    let canon_in = path_within(&canonical, ws.roots);
    match (lex_in, canon_in) {
        // 词法在 roots 且 canonical 在 roots → Allow。
        (true, true) => TargetVerdict {
            verdict: FenceVerdict::Allow,
            reason: None,
        },
        // 词法在 roots 但 canonical 逃逸（symlink）→ Confirm。
        (true, false) => TargetVerdict {
            verdict: FenceVerdict::Confirm,
            reason: Some(format!(
                "{}目标越出工作区（symlink 逃逸）：{}",
                target.kind.label(),
                target.raw
            )),
        },
        // 词法不在 roots 但 canonical 落界内（少见形态，如界外 symlink
        // 指回项目）→ 保守 Confirm。
        (false, true) => TargetVerdict {
            verdict: FenceVerdict::Confirm,
            reason: Some(format!(
                "{}目标路径不在工作区（经链接落回），需确认：{}",
                target.kind.label(),
                target.raw
            )),
        },
        // 词法不在 roots：删除类直接 Block；其余 Confirm。
        (false, false) => {
            let reason = format!("{}目标越出工作区：{}", target.kind.label(), target.raw);
            TargetVerdict {
                verdict: if target.kind.is_destructive() {
                    FenceVerdict::Block
                } else {
                    FenceVerdict::Confirm
                },
                reason: Some(reason),
            }
        }
    }
}

/// 全部写目标的合成判定：取最严结论（Block > Confirm > Allow），
/// 原因取最严结论里的一条。空清单 = 未发现写信号 → Allow。
pub fn judge_targets(targets: &[WriteTarget], ws: &Workspace) -> TargetVerdict {
    let mut worst = TargetVerdict {
        verdict: FenceVerdict::Allow,
        reason: None,
    };
    for t in targets {
        let v = judge_target(t, ws);
        let stricter = matches!(
            (worst.verdict, v.verdict),
            (FenceVerdict::Allow, _) | (FenceVerdict::Confirm, FenceVerdict::Block)
        );
        if stricter {
            worst = v;
        }
    }
    worst
}

#[cfg(test)]
#[path = "ast_test.rs"]
mod ast_test;
