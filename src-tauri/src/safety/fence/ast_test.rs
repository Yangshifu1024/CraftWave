//! fence AST 写目标分析测试（22 号票）：绕过向量测试集、字面量掩码、
//! 四象限判定、fail-closed、git_classify 表驱动、PowerShell 面。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::super::{Fence, FenceOutcome, FenceVerdict};
use super::{
    Canonicalizer, FsCanonicalizer, GitClass, ParsedCommand, Shell, TargetPath, Workspace,
    WriteKind, WriteTarget, git_classify, judge_target, judge_targets, parse_command,
};
use crate::safety::tier::PermissionTier;

// ---- 解析与提取（纯函数面）----

/// 断言：命令解析成功且写目标恰好等于期望（kind + path + raw 逐项比对）。
fn assert_targets(shell: Shell, command: &str, expected: &[(WriteKind, TargetPath, &str)]) {
    let parsed = parse_command(shell, command);
    let ParsedCommand::Ok { targets, .. } = parsed else {
        panic!("{command:?} 应解析成功");
    };
    let got: Vec<(WriteKind, TargetPath, String)> = targets
        .into_iter()
        .map(|t| (t.kind, t.path, t.raw))
        .collect();
    let want: Vec<(WriteKind, TargetPath, String)> = expected
        .iter()
        .map(|(k, p, r)| (*k, p.clone(), r.to_string()))
        .collect();
    assert_eq!(got, want, "命令 {command:?} 的写目标提取不符");
}

/// 字面量目标的简写构造。
fn lit(s: &str) -> TargetPath {
    TargetPath::Literal(s.to_string())
}

#[test]
fn redirect_targets_are_extracted() {
    assert_targets(
        Shell::Posix,
        "echo hi > out.txt",
        &[(WriteKind::Redirect, lit("out.txt"), "out.txt")],
    );
    assert_targets(
        Shell::Posix,
        "echo more >> log.txt",
        &[(WriteKind::Redirect, lit("log.txt"), "log.txt")],
    );
    // fd 重定向（stderr 落文件）也是写目标。
    assert_targets(
        Shell::Posix,
        "ls 2>err.txt",
        &[(WriteKind::Redirect, lit("err.txt"), "err.txt")],
    );
    // 多重重定向逐一提取。
    assert_targets(
        Shell::Posix,
        "echo a > f1 > f2",
        &[
            (WriteKind::Redirect, lit("f1"), "f1"),
            (WriteKind::Redirect, lit("f2"), "f2"),
        ],
    );
}

#[test]
fn quoted_redirect_and_pipe_are_not_targets() {
    // 引号内的 `>`/`|` 是字符串内容，不是语法层面的重定向/管道。
    assert_targets(Shell::Posix, "grep \"a|b\" file", &[]);
    assert_targets(Shell::Posix, "echo \"hello > world\"", &[]);
    assert_targets(Shell::Posix, "echo 'x >> y'", &[]);
    // 管道右段无写命令 → 无写目标（cat x | grep y 只读）。
    assert_targets(Shell::Posix, "cat x | grep y", &[]);
}

#[test]
fn variable_in_write_path_hits_literal_mask() {
    // 绕过向量：变量拼接的写路径 → 字面量掩码命中 → Opaque（保守 Confirm）。
    assert_targets(
        Shell::Posix,
        "echo x > $HOME/evil.txt",
        &[(WriteKind::Redirect, TargetPath::Opaque, "$HOME/evil.txt")],
    );
    assert_targets(
        Shell::Posix,
        "echo x > $TARGET/file",
        &[(WriteKind::Redirect, TargetPath::Opaque, "$TARGET/file")],
    );
    // 命令替换同样掩码命中。
    assert_targets(
        Shell::Posix,
        "cp a.txt \"$(mktemp -d)/b.txt\"",
        &[(WriteKind::Copy, TargetPath::Opaque, "$(mktemp -d)/b.txt")],
    );
    // 普通参数里的变量不影响只读判定（无写目标）。
    assert_targets(Shell::Posix, "cat $FILE", &[]);
}

#[test]
fn heredoc_redirect_target_extracted() {
    // 绕过向量：heredoc 重定向的目标嵌在 heredoc_redirect 内，须提取。
    let cmd = "cat <<EOF > out.txt\nhello\nEOF";
    assert_targets(
        Shell::Posix,
        cmd,
        &[(WriteKind::Redirect, lit("out.txt"), "out.txt")],
    );
}

#[test]
fn write_command_table_extraction() {
    // tee（含 -a 追加）。
    assert_targets(
        Shell::Posix,
        "cat x | tee -a log.txt",
        &[(WriteKind::Tee, lit("log.txt"), "log.txt")],
    );
    // sed -i：跳过脚本参数，文件参数为目标。
    assert_targets(
        Shell::Posix,
        "sed -i 's/a/b/' src.rs",
        &[(WriteKind::SedInPlace, lit("src.rs"), "src.rs")],
    );
    // rm：全部位置参数为删除目标。
    assert_targets(
        Shell::Posix,
        "rm build/a.txt build/b.txt",
        &[
            (WriteKind::Delete, lit("build/a.txt"), "build/a.txt"),
            (WriteKind::Delete, lit("build/b.txt"), "build/b.txt"),
        ],
    );
    // mv：按删除类口径。
    assert_targets(
        Shell::Posix,
        "mv a.txt b.txt",
        &[
            (WriteKind::Move, lit("a.txt"), "a.txt"),
            (WriteKind::Move, lit("b.txt"), "b.txt"),
        ],
    );
    // chmod：跳过模式参数。
    assert_targets(
        Shell::Posix,
        "chmod 755 deploy.sh",
        &[(WriteKind::MetaChange, lit("deploy.sh"), "deploy.sh")],
    );
    // cp：目标 = 最后一个位置参数。
    assert_targets(
        Shell::Posix,
        "cp src.txt dst.txt",
        &[(WriteKind::Copy, lit("dst.txt"), "dst.txt")],
    );
    // install 同 cp。
    assert_targets(
        Shell::Posix,
        "install -m 644 a.conf /etc/a.conf",
        &[(WriteKind::Copy, lit("/etc/a.conf"), "/etc/a.conf")],
    );
    // dd of=。
    assert_targets(
        Shell::Posix,
        "dd if=x.iso of=/dev/sda",
        &[(WriteKind::RawWrite, lit("/dev/sda"), "/dev/sda")],
    );
    // mkfs 家族。
    assert_targets(
        Shell::Posix,
        "mkfs.ext4 /dev/sda1",
        &[(WriteKind::Format, lit("/dev/sda1"), "/dev/sda1")],
    );
    // 只读命令不产目标。
    assert_targets(Shell::Posix, "ls -la && rg TODO src/", &[]);
    // 变量赋值（FOO=bar）不是文件写。
    assert_targets(Shell::Posix, "FOO=bar echo hi", &[]);
}

#[test]
fn malformed_and_deeply_nested_fail_closed() {
    // 畸形命令（未闭合引号）→ 解析失败。
    assert_eq!(
        parse_command(Shell::Posix, "echo \"unterminated"),
        ParsedCommand::Failed
    );
    // 超深嵌套命令替换 → 超限按解析失败（防炸）。
    let depth = 80; // 超过 MAX_DEPTH=64
    let mut nested = "echo x".to_string();
    for _ in 0..depth {
        nested = format!("$({nested})");
    }
    assert_eq!(
        parse_command(Shell::Posix, &nested),
        ParsedCommand::Failed,
        "超深嵌套应判解析失败（fail-closed）"
    );
}

// ---- PowerShell 面 ----

#[test]
fn powershell_redirect_and_cmdlets() {
    assert_targets(
        Shell::PowerShell,
        "echo hi > out.txt",
        &[(WriteKind::Redirect, lit("out.txt"), "out.txt")],
    );
    // 流合并 2>&1 不是文件写。
    assert_targets(Shell::PowerShell, "ls 2>&1", &[]);
    // Set-Content 显式 -Path。
    assert_targets(
        Shell::PowerShell,
        "Set-Content -Path out.txt -Value 1",
        &[(WriteKind::Set, lit("out.txt"), "out.txt")],
    );
    // 位置参数形态（大小写不敏感）。
    assert_targets(
        Shell::PowerShell,
        "set-content out.txt",
        &[(WriteKind::Set, lit("out.txt"), "out.txt")],
    );
    // Remove-Item 全部位置参数为删除目标。
    assert_targets(
        Shell::PowerShell,
        "Remove-Item -Recurse -Force build",
        &[(WriteKind::Delete, lit("build"), "build")],
    );
    // Copy-Item 目标 = 最后位置参数。
    assert_targets(
        Shell::PowerShell,
        "Copy-Item a.txt b.txt",
        &[(WriteKind::Copy, lit("b.txt"), "b.txt")],
    );
    // Tee-Object。
    assert_targets(
        Shell::PowerShell,
        "Get-Content x | Tee-Object -Append log.txt",
        &[(WriteKind::Set, lit("log.txt"), "log.txt")],
    );
    // 变量目标 → 掩码命中。
    assert_targets(
        Shell::PowerShell,
        "$a = \"x\"; Set-Content -Path $a -Value 1",
        &[(WriteKind::Set, TargetPath::Opaque, "$a")],
    );
    // PowerShell 只读管道无写目标。
    assert_targets(Shell::PowerShell, "Get-Content x | Measure-Object", &[]);
}

// ---- git_classify（26 号票接缝，表驱动）----

#[test]
fn git_classify_table() {
    let readonly = [
        vec!["git", "status"],
        vec!["git", "diff", "HEAD~1"],
        vec!["git", "log", "--oneline"],
        vec!["git", "show", "abc123"],
        vec!["git", "rev-parse", "HEAD"],
        vec!["git", "ls-files"],
    ];
    for argv in &readonly {
        assert_eq!(git_classify(argv), GitClass::ReadOnly, "{argv:?} 应归只读");
    }
    let write = [
        vec!["git", "push", "origin", "main"],
        vec!["git", "commit", "-m", "x"],
        vec!["git", "merge", "main"],
        vec!["git", "rebase", "main"],
        vec!["git", "add", "."],
        vec!["git", "checkout", "-b", "dev"],
    ];
    for argv in &write {
        assert_eq!(git_classify(argv), GitClass::Write, "{argv:?} 应归写");
    }
    let destructive = [
        vec!["git", "push", "--force", "origin", "main"],
        vec!["git", "push", "-f", "origin", "main"],
        vec!["git", "push", "--force-with-lease"],
    ];
    for argv in &destructive {
        assert_eq!(
            git_classify(argv),
            GitClass::Destructive,
            "{argv:?} 应归破坏性"
        );
    }
    // 非 git 输入（调用契约外）保守归写。
    assert_eq!(git_classify(&["ls"]), GitClass::Write);
}

// ---- 四象限判定 ----

/// mock 文件系统：把 `links` 里的前缀映射到目标（模拟 symlink 解析），
/// 未命中的路径原样返回（视为无 symlink）。
struct MockFs {
    links: HashMap<PathBuf, PathBuf>,
}

impl Canonicalizer for MockFs {
    fn canonical(&self, path: &Path) -> Option<PathBuf> {
        for (from, to) in &self.links {
            if let Ok(rest) = path.strip_prefix(from) {
                return Some(to.join(rest));
            }
        }
        Some(path.to_path_buf())
    }
}

/// 判定上下文：roots=[/proj]，base=/proj。
fn mock_ws(links: &[(&str, &str)]) -> (Vec<PathBuf>, PathBuf, MockFs) {
    let roots = vec![PathBuf::from("/proj")];
    let base = PathBuf::from("/proj");
    let fs = MockFs {
        links: links
            .iter()
            .map(|(f, t)| (PathBuf::from(f), PathBuf::from(t)))
            .collect(),
    };
    (roots, base, fs)
}

fn ws_of<'a>(roots: &'a [PathBuf], base: &'a Path, fs: &'a MockFs) -> Workspace<'a> {
    Workspace { roots, base, fs }
}

#[test]
fn four_quadrant_verdicts() {
    let (roots, base, fs) = mock_ws(&[("/proj/escape", "/outside")]);
    let ws = ws_of(&roots, &base, &fs);

    // 第一象限：词法在 roots 且 canonical 在 roots → Allow。
    let t = WriteTarget {
        kind: WriteKind::Redirect,
        path: lit("src/main.rs"),
        raw: "src/main.rs".into(),
    };
    assert_eq!(judge_target(&t, &ws).verdict, FenceVerdict::Allow);

    // 第二象限：词法在 roots 但 canonical 逃逸（symlink）→ Confirm。
    let t = WriteTarget {
        kind: WriteKind::Redirect,
        path: lit("escape/f.txt"),
        raw: "escape/f.txt".into(),
    };
    let v = judge_target(&t, &ws);
    assert_eq!(v.verdict, FenceVerdict::Confirm);
    assert!(v.reason.unwrap().contains("symlink 逃逸"));

    // 第三象限：词法不在 roots 但 canonical 落界内 → 保守 Confirm。
    let t = WriteTarget {
        kind: WriteKind::Redirect,
        path: lit("/outside/back"),
        raw: "/outside/back".into(),
    };
    let (roots2, base2, fs2) = mock_ws(&[("/outside/back", "/proj/inside")]);
    let v = judge_target(&t, &ws_of(&roots2, &base2, &fs2));
    assert_eq!(v.verdict, FenceVerdict::Confirm);

    // 第四象限：词法不在 roots → 普通写 Confirm。
    let t = WriteTarget {
        kind: WriteKind::Redirect,
        path: lit("/outside/x.txt"),
        raw: "/outside/x.txt".into(),
    };
    let v = judge_target(&t, &ws);
    assert_eq!(v.verdict, FenceVerdict::Confirm);
    assert!(v.reason.unwrap().contains("越出工作区"));

    // 第四象限删除类：rm/mv 越界 → 直接 Block。
    let t = WriteTarget {
        kind: WriteKind::Delete,
        path: lit("/outside/keep.txt"),
        raw: "/outside/keep.txt".into(),
    };
    assert_eq!(judge_target(&t, &ws).verdict, FenceVerdict::Block);
    let t = WriteTarget {
        kind: WriteKind::Move,
        path: lit("/outside/keep.txt"),
        raw: "/outside/keep.txt".into(),
    };
    assert_eq!(judge_target(&t, &ws).verdict, FenceVerdict::Block);
}

#[test]
fn literal_mask_and_unresolvable_confirm() {
    let (roots, base, fs) = mock_ws(&[]);
    let ws = ws_of(&roots, &base, &fs);
    // 字面量掩码：写路径含变量 → Confirm（防 $HOME/.. 逃逸）。
    let t = WriteTarget {
        kind: WriteKind::Redirect,
        path: TargetPath::Opaque,
        raw: "$HOME/evil.txt".into(),
    };
    let v = judge_target(&t, &ws);
    assert_eq!(v.verdict, FenceVerdict::Confirm);
    assert!(v.reason.unwrap().contains("未解析变量"));
    // 波浪号路径（shell 会展开 home）→ Opaque 同口径。
    let t = WriteTarget {
        kind: WriteKind::Redirect,
        path: TargetPath::Opaque,
        raw: "~/x.txt".into(),
    };
    assert_eq!(judge_target(&t, &ws).verdict, FenceVerdict::Confirm);
}

#[test]
fn combined_targets_take_strictest_verdict() {
    let (roots, base, fs) = mock_ws(&[]);
    let ws = ws_of(&roots, &base, &fs);
    // 一个 Allow + 一个 Block → 整条 Block。
    let targets = vec![
        WriteTarget {
            kind: WriteKind::Redirect,
            path: lit("inside.txt"),
            raw: "inside.txt".into(),
        },
        WriteTarget {
            kind: WriteKind::Delete,
            path: lit("/outside/x"),
            raw: "/outside/x".into(),
        },
    ];
    assert_eq!(judge_targets(&targets, &ws).verdict, FenceVerdict::Block);
    // 空清单 = 未发现写信号 → Allow。
    assert_eq!(judge_targets(&[], &ws).verdict, FenceVerdict::Allow);
}

// ---- 全链判定（Fence + 真实文件系统，symlink 测试限 Unix）----

#[cfg(unix)]
mod unix_fs_tests {
    use super::*;
    use crate::safety::fence::ast::Shell;

    /// 临时工作区：`root/` 为唯一 root，`root/link -> <outside>` 为逃逸链接。
    struct TempRoot {
        root: PathBuf,
        outside: PathBuf,
    }

    impl TempRoot {
        fn new(tag: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "cw-fence-ast-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            let root = base.join("proj");
            let outside = base.join("outside");
            std::fs::create_dir_all(&root).expect("建 root");
            std::fs::create_dir_all(&outside).expect("建 outside");
            std::os::unix::fs::symlink(&outside, root.join("link")).expect("建 symlink");
            Self { root, outside }
        }

        fn workspace(&self) -> (Vec<PathBuf>, PathBuf, FsCanonicalizer) {
            // roots/base 用 canonical 后路径（与生产 PathGuard 同口径：
            // macOS 的 /var 是 /private/var 的 symlink，词法与 canonical
            // 必须在同一表示域比较，否则会把自家 root 误判成逃逸）。
            let canonical = std::fs::canonicalize(&self.root).expect("root canonicalize");
            (vec![canonical.clone()], canonical, FsCanonicalizer)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(
                self.root
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| self.root.clone()),
            );
        }
    }

    /// 判定辅助：roots=[root]，base=root，auto_edit 档。
    fn judge_in(fence: &Fence, cmd: &str, ws: &Workspace) -> FenceOutcome {
        fence.judge_in_workspace(cmd, PermissionTier::AutoEdit, Shell::Posix, Some(ws))
    }

    #[test]
    fn bypass_vectors_reduce_conservatively() {
        // 绕过向量一：变量拼接写路径 → Confirm（字面量掩码）。
        let tmp = TempRoot::new("var");
        let (roots, base, fs) = tmp.workspace();
        let ws = Workspace {
            roots: &roots,
            base: &base,
            fs: &fs,
        };
        let fence = Fence::new();
        let out = judge_in(&fence, "echo x > $HOME/evil.txt", &ws);
        assert_eq!(out.verdict, FenceVerdict::Confirm);
        assert!(out.reason.unwrap().contains("未解析变量"));

        // 绕过向量二：symlink 穿越界外 → Confirm（symlink 逃逸）。
        let tmp = TempRoot::new("symlink");
        let (roots, base, fs) = tmp.workspace();
        let ws = Workspace {
            roots: &roots,
            base: &base,
            fs: &fs,
        };
        let out = judge_in(&fence, "echo x > link/evil.txt", &ws);
        assert_eq!(out.verdict, FenceVerdict::Confirm);
        assert!(out.reason.unwrap().contains("symlink 逃逸"));

        // 绕过向量三：heredoc 重定向 → 提取目标并判定（roots 内 → Allow）。
        let tmp = TempRoot::new("heredoc");
        let (roots, base, fs) = tmp.workspace();
        let ws = Workspace {
            roots: &roots,
            base: &base,
            fs: &fs,
        };
        let out = judge_in(&fence, "cat <<EOF > out.txt\nhello\nEOF", &ws);
        assert_eq!(
            out.verdict,
            FenceVerdict::Allow,
            "roots 内 heredoc 写应放行"
        );
    }

    #[test]
    fn escape_and_destructive_outside_roots() {
        let tmp = TempRoot::new("escape2");
        let (roots, base, fs) = tmp.workspace();
        let ws = Workspace {
            roots: &roots,
            base: &base,
            fs: &fs,
        };
        let fence = Fence::new();

        // 界外普通写 → Confirm。
        let out = judge_in(
            &fence,
            &format!("echo x > {}", tmp.outside.join("x.txt").display()),
            &ws,
        );
        assert_eq!(out.verdict, FenceVerdict::Confirm);

        // 界外删除 → Block（删除类直接硬拦）。
        let out = judge_in(
            &fence,
            &format!("rm {}", tmp.outside.join("keep.txt").display()),
            &ws,
        );
        assert_eq!(out.verdict, FenceVerdict::Block);

        // cat x | tee 界外 → 保守合并整条判定（tee 写目标在界外 → Confirm）。
        let out = judge_in(
            &fence,
            &format!(
                "cat secret.txt | tee {}",
                tmp.outside.join("leak.txt").display()
            ),
            &ws,
        );
        assert_eq!(out.verdict, FenceVerdict::Confirm);

        // sed -i 越界 → Confirm。
        let out = judge_in(
            &fence,
            &format!("sed -i 's/a/b/' {}/f.txt", tmp.outside.display()),
            &ws,
        );
        assert_eq!(out.verdict, FenceVerdict::Confirm);

        // tee -a 追加越界 → Confirm。
        let out = judge_in(
            &fence,
            &format!("echo x | tee -a {}/log.txt", tmp.outside.display()),
            &ws,
        );
        assert_eq!(out.verdict, FenceVerdict::Confirm);

        // roots 内普通写 → Allow（auto_edit 档直写语义由 §8.5 矩阵合成）。
        let out = judge_in(&fence, "echo done > build.log", &ws);
        assert_eq!(out.verdict, FenceVerdict::Allow);
    }

    #[test]
    fn parse_failure_is_fail_closed_not_open() {
        let tmp = TempRoot::new("failclosed");
        let (roots, base, fs) = tmp.workspace();
        let ws = Workspace {
            roots: &roots,
            base: &base,
            fs: &fs,
        };
        let fence = Fence::new();
        // 构造解析失败路径（未闭合引号）：行为为 Confirm，无 fail-open。
        let out = judge_in(&fence, "echo \"unterminated > out.txt", &ws);
        assert_eq!(out.verdict, FenceVerdict::Confirm);
        assert!(out.reason.unwrap().contains("解析失败"));
        // 超深嵌套同样 fail-closed。
        let mut nested = "echo x".to_string();
        for _ in 0..80 {
            nested = format!("$({nested})");
        }
        let out = judge_in(&fence, &nested, &ws);
        assert_eq!(out.verdict, FenceVerdict::Confirm);
    }

    #[test]
    fn dd_to_device_still_blocked_by_blacklist() {
        let tmp = TempRoot::new("dd");
        let (roots, base, fs) = tmp.workspace();
        let ws = Workspace {
            roots: &roots,
            base: &base,
            fs: &fs,
        };
        let fence = Fence::new();
        // dd of=/dev/… 黑名单先行 Block（AST 提取不到判界机会）。
        let out = judge_in(&fence, "dd if=x of=/dev/sda", &ws);
        assert_eq!(out.verdict, FenceVerdict::Block);
    }
}
