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

/// 篡改向量（数据侧文件禁区）：写 config.json（含 apiKey 明文）必须命中。
#[test]
fn data_side_config_json_hit() {
    let (data, _project, guard) = setup();
    assert_hit(&guard, &data.path().join("config.json"));
    // 相对路径经基准展开后同样命中（cwd 在数据目录内的形态）。
    assert_hit(&guard, &data.path().join("./config.json"));
}

/// 篡改向量（数据侧目录禁区）：projects/sessions/histories/stats/quota/
/// trust/logs/cache 与 ui-state.json、全局 mcp.json 逐项命中。
#[test]
fn data_side_zones_all_hit() {
    let (data, _project, guard) = setup();
    for d in super::DATA_SIDE_DIRS {
        assert_hit(&guard, &data.path().join(d));
        // 目录内深层文件同样命中
        assert_hit(&guard, &data.path().join(d).join("nested/file.json"));
    }
    for f in super::DATA_SIDE_FILES {
        assert_hit(&guard, &data.path().join(f));
    }
}

/// 篡改向量（项目侧）：`<home>/.craftwave/` 全部（project.json、项目
/// mcp.json）命中。
#[test]
fn project_side_craftwave_hit() {
    let (_data, project, guard) = setup();
    assert_hit(&guard, &project.path().join(".craftwave"));
    assert_hit(&guard, &project.path().join(".craftwave/project.json"));
    assert_hit(&guard, &project.path().join(".craftwave/mcp.json"));
}

/// 文件名面：任意层级的 `.segmeta.json`（段元数据/索引/锁面）命中，
/// 同目录其他文件不受影响。
#[test]
fn segmeta_filename_hit_anywhere() {
    let (_data, project, guard) = setup();
    // 项目内（工作区内）的 .segmeta.json 也拦——文件名面不分层级
    assert_hit(&guard, &project.path().join(".segmeta.json"));
    assert_hit(
        &guard,
        &project.path().join("deep/nested/dir/.segmeta.json"),
    );
    assert_pass(&guard, &project.path().join("segmeta.json"));
    assert_pass(&guard, &project.path().join("normal.txt"));
}

/// workspace 例外：数据根下 `workspace/` 是代理工作区，不属禁区。
#[test]
fn data_workspace_exception() {
    let (data, _project, guard) = setup();
    assert_pass(&guard, &data.path().join("workspace"));
    assert_pass(&guard, &data.path().join("workspace/out.txt"));
    // 清单外的数据根路径（skills/、backups/ 等）不在本票禁区——不扩大
    assert_pass(&guard, &data.path().join("skills"));
}

/// symlink 指入禁区：词法路径在工作区内、canonical 落点在禁区 → 命中。
#[test]
fn symlink_into_forbidden_zone_hit() {
    let (data, project, guard) = setup();
    // canonical 判定要真实落点存在（canonicalize 解析 symlink 后才可比对）
    fs::write(data.path().join("config.json"), "{}").expect("写 config.json");
    fs::create_dir(data.path().join("sessions")).expect("建 sessions");
    // 工作区内的 symlink → 数据侧 config.json
    let link = project.path().join("out.json");
    unix_symlink(data.path().join("config.json"), &link);
    assert_hit(&guard, &link);
    // 工作区内的目录 symlink → sessions/
    let dir_link = project.path().join("sessions_view");
    unix_symlink(data.path().join("sessions"), &dir_link);
    assert_hit(&guard, &dir_link.join("x.json"));
}

/// symlink 指向非禁区（工作区外普通路径）不命中——禁区判定不越权扩大。
#[test]
fn symlink_outside_zones_pass() {
    let (data, project, guard) = setup();
    let plain = data.path().join("plain.txt");
    fs::write(&plain, "x").expect("写普通文件");
    let link = project.path().join("link.txt");
    unix_symlink(plain, &link);
    assert_pass(&guard, &link);
}

/// 段元数据 symlink 形态：词法文件名是 `.segmeta.json` 即拦
///（文件名面在 canonical 之前判定，链接指向处不影响）。
#[test]
fn segmeta_via_symlink_hit() {
    let (_data, project, guard) = setup();
    let target = project.path().join("harmless.json");
    fs::write(&target, "{}").expect("写无害文件");
    let link = project.path().join(".segmeta.json");
    unix_symlink(target, &link);
    assert_hit(&guard, &link);
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
