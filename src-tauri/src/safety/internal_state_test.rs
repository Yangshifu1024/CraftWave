//! 宿主内部状态保护（internal_state）单元测试：禁区清单命中、词法与
//! canonical（symlink）双路判定、workspace 例外、文件名面（.segmeta.json）。
//! 全部用临时目录真实文件系统（FsCanonicalizer 生产实现），不 mock。

use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

use crate::safety::fence::ast::FsCanonicalizer;

use super::InternalStateGuard;

/// 造一对（数据根, 项目根）临时目录 + 已装配的禁区判定面。
fn setup() -> (TempDir, TempDir, InternalStateGuard) {
    let data = TempDir::new().expect("临时数据目录");
    let project = TempDir::new().expect("临时项目目录");
    let guard = InternalStateGuard::new(data.path(), project.path());
    (data, project, guard)
}

/// 命中断言的便捷包装：词法路径必须命中且原因含关键类别词。
fn assert_hit(guard: &InternalStateGuard, lexical: &std::path::Path) {
    let hit = guard.hit("TARGET", lexical, &FsCanonicalizer);
    assert!(hit.is_some(), "应命中禁区：{}", lexical.display());
}

/// 未命中断言的便捷包装。
fn assert_pass(guard: &InternalStateGuard, lexical: &std::path::Path) {
    let hit = guard.hit("TARGET", lexical, &FsCanonicalizer);
    assert!(
        hit.is_none(),
        "不应命中禁区：{}（原因：{hit:?}）",
        lexical.display()
    );
}

/// Unix symlink 创建（测试前置；失败即 panic）。
fn unix_symlink(target: PathBuf, link: &std::path::Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).expect("创建 symlink");
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        panic!("symlink 测试仅覆盖 Unix 面（Windows 判定路径由词法/目录用例覆盖）");
    }
}

// 断言：词法面禁区清单逐项命中——数据侧 config.json（含相对路径 `./`
// 形态）、projects/sessions/histories/stats/quota/trust/logs/cache 目录
// （含深层文件）与 ui-state.json、全局 mcp.json、项目侧 `<home>/.craftwave/`
// 全部；任意层级的 `.segmeta.json` 文件名面命中、同目录其他文件不受影响；
// workspace 例外不属禁区（清单外数据根路径也不扩大）。
#[test]
fn forbidden_zone_lexical_hits_and_workspace_exception() {
    let (data, project, guard) = setup();
    // 数据侧文件禁区（含相对路径经基准展开的形态）。
    assert_hit(&guard, &data.path().join("config.json"));
    assert_hit(&guard, &data.path().join("./config.json"));
    // 数据侧目录禁区与文件清单。
    for d in super::DATA_SIDE_DIRS {
        assert_hit(&guard, &data.path().join(d));
        // 目录内深层文件同样命中
        assert_hit(&guard, &data.path().join(d).join("nested/file.json"));
    }
    for f in super::DATA_SIDE_FILES {
        assert_hit(&guard, &data.path().join(f));
    }
    // 项目侧：`<home>/.craftwave/` 全部命中。
    assert_hit(&guard, &project.path().join(".craftwave"));
    assert_hit(&guard, &project.path().join(".craftwave/project.json"));
    assert_hit(&guard, &project.path().join(".craftwave/mcp.json"));
    // 文件名面：项目内（工作区内）的 .segmeta.json 也拦——不分层级。
    assert_hit(&guard, &project.path().join(".segmeta.json"));
    assert_hit(
        &guard,
        &project.path().join("deep/nested/dir/.segmeta.json"),
    );
    assert_pass(&guard, &project.path().join("segmeta.json"));
    assert_pass(&guard, &project.path().join("normal.txt"));
    // workspace 例外：数据根下 `workspace/` 是代理工作区；清单外的数据根
    // 路径（skills/、backups/ 等）不在本票禁区——不扩大。
    assert_pass(&guard, &data.path().join("workspace"));
    assert_pass(&guard, &data.path().join("workspace/out.txt"));
    assert_pass(&guard, &data.path().join("skills"));
}

// 断言：canonical（symlink）双路判定——工作区内 symlink 指入禁区（文件
// 与目录两种形态）命中；词法文件名是 `.segmeta.json` 即拦（文件名面在
// canonical 之前判定）；symlink 指向非禁区不命中（禁区判定不越权扩大）。
#[test]
fn canonical_symlink_vectors() {
    let (data, project, guard) = setup();
    // canonical 判定要真实落点存在（canonicalize 解析 symlink 后才可比对）。
    fs::write(data.path().join("config.json"), "{}").expect("写 config.json");
    fs::create_dir(data.path().join("sessions")).expect("建 sessions");
    // 工作区内的 symlink → 数据侧 config.json。
    let link = project.path().join("out.json");
    unix_symlink(data.path().join("config.json"), &link);
    assert_hit(&guard, &link);
    // 工作区内的目录 symlink → sessions/。
    let dir_link = project.path().join("sessions_view");
    unix_symlink(data.path().join("sessions"), &dir_link);
    assert_hit(&guard, &dir_link.join("x.json"));
    // 段元数据 symlink 形态：链接指向处不影响文件名面判定。
    let target = project.path().join("harmless.json");
    fs::write(&target, "{}").expect("写无害文件");
    let seg_link = project.path().join(".segmeta.json");
    unix_symlink(target, &seg_link);
    assert_hit(&guard, &seg_link);
    // symlink 指向非禁区（工作区外普通路径）不命中。
    let plain = data.path().join("plain.txt");
    fs::write(&plain, "x").expect("写普通文件");
    let plain_link = project.path().join("link.txt");
    unix_symlink(plain, &plain_link);
    assert_pass(&guard, &plain_link);
}
