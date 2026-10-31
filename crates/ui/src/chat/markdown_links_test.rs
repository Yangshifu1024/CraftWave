//! 链接分流测试（gpui-rewrite 2.C-3，Wake `markdown_links.rs` 用例对照迁移）。
//!
//! 每个用例对应 resolve_link 判序的一条分支（判序表见 markdown_links.rs
//! 模块注释）；临时目录造真实文件——判定会 `exists()` 探测。

use std::path::PathBuf;

use url::Url;

use super::markdown_links::{LinkTarget, resolve_link};

/// 造 `项目根/internal/…` 形态的文件树，返回 (临时根, 项目根, 文件)。
fn project_file(name: &str) -> (tempfile::TempDir, String, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("OpenSurge for Mac");
    let file = project.join("internal/controlapi").join(name);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "package controlapi\n").unwrap();
    let project_text = project.to_str().unwrap().to_owned();
    (temp, project_text, file)
}

// ======================================================================
// 分支 7+8：本地文件 / 目录 / 编码链接 / 行号后缀
// ======================================================================

/// 本地文件按字面、编码、file: URI、相对/`~` 路径都能落到同一个文件。
#[test]
fn local_files_directories_and_encoded_links() {
    let (temp, project, file) = project_file("mihomo_recovery.go");
    let file_text = file.to_str().unwrap();
    let file_url = Url::from_file_path(&file).unwrap();
    for href in [
        file_text.to_owned(),
        file_text.replace(' ', "%20"),
        file_url.to_string(),
        format!("file:{}", file_url.path()),
        format!("FILE://localhost{}", file_url.path()),
        // 相对路径按项目根展开（./ 前缀与 ../ 折返两种写法）
        "internal/controlapi/mihomo_recovery.go".into(),
        "./internal/controlapi/mihomo_recovery.go".into(),
        "../OpenSurge for Mac/internal/controlapi/mihomo_recovery.go".into(),
        "~/OpenSurge for Mac/internal/controlapi/mihomo_recovery.go".into(),
    ] {
        let target = resolve_link(&href, false, &project, || Some(temp.path().into()));
        let LinkTarget::File(path) = target else {
            panic!("{href:?}: {target:?}");
        };
        assert_eq!(path.canonicalize().unwrap(), file.canonicalize().unwrap());
    }
    // 项目根本身也是合法目标（目录）
    assert_eq!(
        resolve_link(&project, false, "", || None),
        LinkTarget::File(PathBuf::from(&project))
    );
}

/// `:N` 行号后缀：完整路径不存在时去掉行号再找（三种 href 拼法 × 两种文件名）。
#[test]
fn line_number_links_resolve_to_existing_files() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("Wake with spaces");
    std::fs::create_dir_all(project.join("src")).unwrap();
    for (name, line) in [("markdown_links.rs", 11), ("workbench.rs", 7955)] {
        let file = project.join("src").join(name);
        std::fs::write(&file, "").unwrap();
        for href in [
            format!("{}:{line}", file.display()),
            format!("{}:{line}", file.to_str().unwrap().replace(' ', "%20")),
            format!("{}:{line}", Url::from_file_path(&file).unwrap()),
            format!("src/{name}:{line}"),
            format!("./src/{name}:{line}"),
            format!("~/Wake with spaces/src/{name}:{line}"),
        ] {
            let target = resolve_link(&href, false, project.to_str().unwrap(), || {
                Some(temp.path().into())
            });
            let LinkTarget::File(path) = target else {
                panic!("{href:?}: {target:?}");
            };
            assert_eq!(path.canonicalize().unwrap(), file.canonicalize().unwrap());
        }
    }
}

/// 裸文件名 `main.rs:11` 不当 scheme：按项目根展开；字面同名文件存在时
/// 字面优先；文件缺失或项目根缺席 → Unavailable（不当 URL）。
#[test]
fn bare_filename_line_links_use_the_session_project() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().to_str().unwrap();
    let file = temp.path().join("main.rs");
    std::fs::write(&file, "").unwrap();
    assert_eq!(
        resolve_link("main.rs:11", false, project, || None),
        LinkTarget::File(file)
    );
    // 字面文件名里真含 ":11" 后缀的仍字面优先
    let literal = temp.path().join("main.rs:11");
    std::fs::write(&literal, "").unwrap();
    assert_eq!(
        resolve_link("main.rs:11", false, project, || None),
        LinkTarget::File(literal)
    );
    for (href, project) in [("missing.rs:11", project), ("main.rs:11", "")] {
        assert_eq!(
            resolve_link(href, false, project, || None),
            LinkTarget::Unavailable
        );
    }
}

/// 行号后缀的边界：字面名优先于解码名；伪行号（0/带符号/带字/fragment/
/// 已编码冒号）都不算行号；目录不是文件（:N 找到的必须是 file）。
#[test]
fn line_numbers_preserve_literal_names_and_require_a_file() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("source file.rs");
    let literal = temp.path().join("source file.rs:11");
    // 即使去掉行号后两种拼法都存在，完整的解码文件名也优先
    for path in [&file, &literal, &temp.path().join("source%20file.rs")] {
        std::fs::write(path, "").unwrap();
    }
    for href in [
        literal.to_str().unwrap().to_owned(),
        literal.to_str().unwrap().replace(' ', "%20"),
        Url::from_file_path(&literal).unwrap().to_string(),
    ] {
        assert_eq!(
            resolve_link(&href, false, "", || None),
            LinkTarget::File(literal.clone())
        );
    }
    for suffix in [":0", ":+11", ":-11", ":line11", "%3A12", ":11#fragment"] {
        for base in [
            file.to_str().unwrap().to_owned(),
            Url::from_file_path(&file).unwrap().to_string(),
        ] {
            let href = format!("{base}{suffix}");
            assert_eq!(
                resolve_link(&href, false, "", || None),
                LinkTarget::Unavailable,
                "{href}"
            );
        }
    }
    // 去掉行号后是目录不是文件：不算命中
    assert_eq!(
        resolve_link(&format!("{}:11", temp.path().display()), false, "", || None),
        LinkTarget::Unavailable
    );
}

/// 文件名不被二次解释、也不二次解码：字面名含 % # ? 冒号的照常命中，
/// 双重编码（%2520）只解一次不解两次。
#[test]
fn filenames_are_not_reinterpreted_or_decoded_twice() {
    let temp = tempfile::tempdir().unwrap();
    for name in [
        "中文 #1%.md",
        "literal%20space.md",
        "literal space.md",
        "name:42",
        "name#L42",
    ] {
        let path = temp.path().join(name);
        std::fs::write(&path, "").unwrap();
        for href in [
            path.to_str().unwrap().to_owned(),
            Url::from_file_path(&path).unwrap().to_string(),
        ] {
            assert_eq!(
                resolve_link(&href, false, "", || None),
                LinkTarget::File(path.clone())
            );
        }
    }
    std::fs::write(temp.path().join("once decoded.md"), "").unwrap();
    assert_eq!(
        resolve_link(
            "once%2520decoded.md",
            false,
            temp.path().to_str().unwrap(),
            || None
        ),
        LinkTarget::Unavailable
    );
}

// ======================================================================
// 分支 5：远程会话一律 Remote（不碰本机、不碰 HOME）
// ======================================================================

#[test]
fn remote_paths_never_resolve_against_this_machine() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("exists.md");
    std::fs::write(&file, "").unwrap();
    let with_line = format!("{}:11", file.display());
    for href in [
        file.to_str().unwrap(),
        &with_line,
        "~/exists.md",
        "~/exists.md:11",
        "exists.md",
        "exists.md:11",
        "./exists.md:11",
        "file:///tmp/exists.md",
        "file:///tmp/exists.md:11",
        "C:/work/a.rs",
    ] {
        assert_eq!(
            resolve_link(href, true, temp.path().to_str().unwrap(), || panic!(
                "远程路径不得请求本地 HOME"
            )),
            LinkTarget::Remote
        );
    }
}

// ======================================================================
// 分支 6 + 兜底：无效路径永不变成外部 URL
// ======================================================================

#[test]
fn invalid_or_unavailable_paths_never_become_external_urls() {
    let temp = tempfile::tempdir().unwrap();
    for href in [
        "/wake-link-test-missing/a.rs",
        "/wake-link-test-missing/a.rs:11",
        "missing.rs",
        // file: 严格解析的反例族
        "file:relative.rs",
        "file://Users/example/a.rs",
        "file://server/share/a.rs",
        "file://server/share/a.rs:11",
        "file:///C:/work/a.rs",
        // Windows 形态与 UNC
        "C:/work/a.rs",
        r"C:\work\a.rs",
        r"\\server\share\a.rs",
        "//server/share/a.rs",
        // query / fragment / 控制字符
        "file:///tmp/a.rs?x=1",
        "file:///tmp/a.rs#L42",
        "file:///tmp/a%00.rs",
        "~someone/a.rs",
    ] {
        assert_eq!(
            resolve_link(href, false, temp.path().to_str().unwrap(), || None),
            LinkTarget::Unavailable,
            "{href}"
        );
    }
    // 即使 cwd 里真有 Cargo.toml，也不能在会话基准缺席时用 cwd
    assert_eq!(
        resolve_link("Cargo.toml", false, "", || None),
        LinkTarget::Unavailable
    );
    assert_eq!(
        resolve_link("Cargo.toml", false, ".", || None),
        LinkTarget::Unavailable
    );
}

// ======================================================================
// 分支 1-4：外部 scheme 与锚点保持本义
// ======================================================================

#[test]
fn external_schemes_and_anchors_keep_their_own_meaning() {
    for remote in [false, true] {
        for href in [
            "https://example.com/a%20b?q=1#x",
            "http://example.com",
            "mailto:a@example.com",
            "vscode://file/tmp/a.rs",
            "vscode://file/tmp/a.rs:11",
            "https://example.com/a.rs:11",
            "custom+app:value",
            "custom+app:11",
            "org.example://open/main.rs:11",
            "x://example.com",
        ] {
            assert_eq!(
                resolve_link(href, remote, "", || panic!("URL 不得请求本地 HOME")),
                LinkTarget::External
            );
        }
        // 空链接与页内锚点：忽略（远程与否无关）
        for href in ["", "#section"] {
            assert_eq!(
                resolve_link(href, remote, "", || None),
                LinkTarget::Ignore
            );
        }
    }
}
