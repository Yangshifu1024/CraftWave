//! 技能系统单测（09 号票）：frontmatter 解析（含 Claude 格式真实样例）、
//! 三来源同名覆盖优先级、Q21 版本指纹、skills-state.json round-trip、
//! 渐进披露注入文本生成（断言不含正文）、skill 工具规格动态 description、
//! run 快照查询面（Q20）。
//!
//! 独立 _test.rs 文件（仓库纪律：源码文件内不写测试）。

use std::path::PathBuf;

use super::skills::{
    RegistryRoots, SkillEntry, SkillRegistry, SkillSnapshot, SkillSnapshotEntry, SkillSource,
    disclosure_text, merge_sources, parse_skill_md, skill_spec, valid_skill_name,
    version_fingerprint,
};

/// 造一个临时技能目录（frontmatter + 正文 + 可选随附文件），返回目录路径。
/// 随附文件自动建父目录（如 `scripts/run.sh`）。
fn make_skill(
    root: &std::path::Path,
    dir_name: &str,
    frontmatter: &str,
    body: &str,
    extra: &[(&str, &[u8])],
) -> PathBuf {
    let dir = root.join(dir_name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\n{frontmatter}\n---\n{body}"),
    )
    .unwrap();
    for (name, bytes) in extra {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }
    dir
}

/// 手工构造索引条目（测试简写）。
fn entry(name: &str, source: SkillSource, description: &str) -> SkillEntry {
    SkillEntry {
        name: name.to_owned(),
        source,
        description: description.to_owned(),
        dir: PathBuf::from("/tmp"),
        fingerprint: format!("fp-{description}"),
    }
}

/// Claude 格式真实样例：额外字段（license/allowed-tools/metadata）+ 长
/// description 含冒号与引号——额外字段忽略、description 原样解析。
const CLAUDE_STYLE_SKILL: &str = "\
name: pdf-processing
description: \"Reads, extracts and merges text from PDF files: covers extraction, tables, forms & OCR fallbacks\"
license: MIT
allowed-tools:
  - Bash(pdftotext:*)
  - Read
metadata:
  version: \"1.2\"
";

#[test]
fn parses_claude_style_frontmatter_ignoring_extra_fields() {
    let parsed = parse_skill_md(&format!(
        "---\n{CLAUDE_STYLE_SKILL}---\n# 正文标题\n\n正文内容。\n"
    ))
    .expect("Claude 格式样例应解析成功");
    assert_eq!(parsed.name, "pdf-processing");
    assert_eq!(
        parsed.description,
        "Reads, extracts and merges text from PDF files: covers extraction, tables, forms & OCR fallbacks"
    );
    assert_eq!(parsed.body, "# 正文标题\n\n正文内容。\n");
}

#[test]
fn parses_multiline_folded_description_and_strips_frontmatter() {
    let text =
        "---\nname: demo\ndescription: >\n  第一行折行\n  第二行折行\n---\n\n正文从空行后开始\n";
    let parsed = parse_skill_md(text).expect("折行 description 应解析成功");
    // YAML 折叠标量：换行折叠为单空格（保持「一句话简介」形态）。
    assert_eq!(parsed.description, "第一行折行 第二行折行");
    assert_eq!(parsed.body, "正文从空行后开始\n");
}

#[test]
fn rejects_malformed_frontmatter() {
    // 无 frontmatter
    assert!(parse_skill_md("# 直接正文").is_none());
    // 结束符缺失
    assert!(parse_skill_md("---\nname: demo\ndescription: x\n正文").is_none());
    // 缺 name
    assert!(parse_skill_md("---\ndescription: 有简介\n---\n正文").is_none());
    // 缺 description
    assert!(parse_skill_md("---\nname: demo\n---\n正文").is_none());
    // name 非法字符
    assert!(parse_skill_md("---\nname: \"含空格 名\"\ndescription: x\n---\n正文").is_none());
    // name 超长（65 字符）
    let long = "a".repeat(65);
    assert!(parse_skill_md(&format!("---\nname: {long}\ndescription: x\n---\n正文")).is_none());
    // description 纯空白
    assert!(parse_skill_md("---\nname: demo\ndescription: \"   \"\n---\n正文").is_none());
    // frontmatter 是数组而非映射
    assert!(parse_skill_md("---\n- a\n- b\n---\n正文").is_none());
}

#[test]
fn validates_skill_name_charset() {
    assert!(valid_skill_name("hello-craftwave"));
    assert!(valid_skill_name("skill_2"));
    assert!(valid_skill_name("A1_b-c"));
    assert!(!valid_skill_name(""));
    assert!(!valid_skill_name("含中文"));
    assert!(!valid_skill_name("has space"));
    assert!(!valid_skill_name(&"x".repeat(65)));
}

#[test]
fn fingerprint_covers_body_and_attached_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = make_skill(
        tmp.path(),
        "demo",
        "name: demo\ndescription: 简介",
        "正文 v1",
        &[("scripts/run.sh", b"echo hi")],
    );
    let fp1 = version_fingerprint("demo", &dir).unwrap();
    // 正文变化 → 指纹变化（Q21：SKILL.md 变化重核信任）
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: demo\ndescription: 简介\n---\n正文 v2",
    )
    .unwrap();
    let fp2 = version_fingerprint("demo", &dir).unwrap();
    assert_ne!(fp1, fp2, "SKILL.md 变化必须改变指纹");
    // 附属脚本变化 → 指纹变化（Q21：附属资源同在信任覆盖内）
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: demo\ndescription: 简介\n---\n正文 v1",
    )
    .unwrap();
    std::fs::write(dir.join("scripts/run.sh"), b"echo changed").unwrap();
    let fp3 = version_fingerprint("demo", &dir).unwrap();
    assert_ne!(fp1, fp3, "附属资源变化必须改变指纹");
    // 还原附属脚本 → 指纹回到 fp1（内容寻址：变回即同版本）
    std::fs::write(dir.join("scripts/run.sh"), b"echo hi").unwrap();
    // 名字参与指纹（同内容不同名 → 不同指纹）
    assert_ne!(fp1, version_fingerprint("demo2", &dir).unwrap());
    // 稳定：同内容重算一致
    assert_eq!(fp1, version_fingerprint("demo", &dir).unwrap());
}

#[test]
fn fingerprint_skips_symlinks_and_rejects_missing_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = make_skill(
        tmp.path(),
        "demo",
        "name: demo\ndescription: 简介",
        "正文",
        &[],
    );
    let fp1 = version_fingerprint("demo", &dir).unwrap();
    // symlink 不追随：加 symlink 文件/目录不改变指纹，也不产生越界
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/etc", dir.join("evil_dir")).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", dir.join("evil_file")).unwrap();
        assert_eq!(
            fp1,
            version_fingerprint("demo", &dir).unwrap(),
            "symlink 不参与指纹"
        );
    }
    assert!(version_fingerprint("demo", &tmp.path().join("missing")).is_none());
}

#[test]
fn merge_sources_prefers_project_over_global_over_builtin() {
    // 同名三连 → 项目胜；异名并存
    let merged = merge_sources(&[
        vec![
            entry("alpha", SkillSource::Builtin, "内置版"),
            entry("beta", SkillSource::Builtin, "内置独有"),
        ],
        vec![entry("alpha", SkillSource::Global, "全局版")],
        vec![entry("alpha", SkillSource::Project, "项目版")],
    ]);
    assert_eq!(merged.len(), 2, "同名覆盖后只余 alpha/beta");
    let alpha = merged.iter().find(|e| e.name == "alpha").unwrap();
    assert_eq!(alpha.source, SkillSource::Project);
    assert_eq!(alpha.description, "项目版");
    assert_eq!(
        merged
            .iter()
            .find(|e| e.name == "beta")
            .unwrap()
            .description,
        "内置独有"
    );
    // 只有两来源时同理：全局覆盖内置
    let merged2 = merge_sources(&[
        vec![entry("alpha", SkillSource::Builtin, "内置版")],
        vec![entry("alpha", SkillSource::Global, "全局版")],
    ]);
    assert_eq!(merged2[0].source, SkillSource::Global);
}

#[test]
fn registry_reload_toggle_and_disclosure() {
    let tmp = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    make_skill(
        builtin.path(),
        "hello",
        "name: hello\ndescription: 内置示例",
        "内置正文 SECRET_BUILTIN_BODY",
        &[],
    );
    make_skill(
        global.path(),
        "hello",
        "name: hello\ndescription: 全局覆盖版",
        "全局正文",
        &[],
    );
    make_skill(
        project.path(),
        "proj-only",
        "name: proj-only\ndescription: 项目独有技能",
        "项目正文",
        &[],
    );
    let registry = SkillRegistry::default();
    registry.configure(RegistryRoots {
        builtin: Some(builtin.path().to_path_buf()),
        global: Some(global.path().to_path_buf()),
        project: Some(project.path().to_path_buf()),
        state_file: Some(tmp.path().join("skills-state.json")),
    });
    // 覆盖：hello = 全局版（内置 < 全局）；proj-only 并存
    let list = registry.list();
    assert_eq!(list.len(), 2);
    let hello = list.iter().find(|e| e.name == "hello").unwrap();
    assert_eq!(hello.source, SkillSource::Global);
    assert_eq!(hello.description, "全局覆盖版");
    // 启停 round-trip：禁用 → 重扫后仍禁用（状态独立于索引）
    assert!(registry.set_enabled("hello", false));
    assert!(registry.is_disabled("hello"));
    registry.reload();
    assert!(registry.is_disabled("hello"), "重扫不丢禁用态");
    let enabled = registry.enabled_entries();
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].name, "proj-only");
    // 重新启用
    assert!(registry.set_enabled("hello", true));
    assert!(!registry.is_disabled("hello"));
    // 不存在的名字 → false（命令层转 E_NOT_FOUND）
    assert!(!registry.set_enabled("missing", false));
    // 状态文件 round-trip（重启语义）：文件形状 = §7.7 schema
    let state_bytes = std::fs::read(tmp.path().join("skills-state.json")).unwrap();
    let state: super::skills::SkillsState = serde_json::from_slice(&state_bytes).unwrap();
    assert_eq!(state.version, 1);
    assert!(state.disabled.is_empty());

    // 渐进披露：仅 name + description，断言不含正文（FR-09-02）。
    // 注入面基于 run 快照条目（Q20：description 与正文同版本）。
    registry.set_enabled("hello", false);
    let snapshot_entries: Vec<super::skills::SkillSnapshotEntry> = registry
        .enabled_entries()
        .into_iter()
        .map(|e| super::skills::SkillSnapshotEntry {
            name: e.name,
            description: e.description,
            body: "项目正文 SECRET_BUILTIN_BODY".to_owned(),
            fingerprint: e.fingerprint,
            untrusted: false,
        })
        .collect();
    let disclosure = disclosure_text(&snapshot_entries).expect("有启用技能应产生注入文本");
    assert!(disclosure.contains("proj-only：项目独有技能"));
    assert!(!disclosure.contains("项目正文"), "注入文本不得包含正文");
    assert!(!disclosure.contains("SECRET_BUILTIN_BODY"));
    // 空清单零注入
    assert!(disclosure_text(&[]).is_none());
}

#[test]
fn skill_spec_description_lists_enabled_names() {
    let tmp = tempfile::tempdir().unwrap();
    make_skill(
        tmp.path(),
        "alpha",
        "name: alpha\ndescription: 技能甲",
        "甲正文",
        &[],
    );
    make_skill(
        tmp.path(),
        "beta",
        "name: beta\ndescription: 技能乙",
        "乙正文",
        &[],
    );
    let registry = SkillRegistry::default();
    registry.configure(RegistryRoots {
        builtin: None,
        global: Some(tmp.path().to_path_buf()),
        project: None,
        state_file: Some(tmp.path().join("state.json")),
    });
    SkillRegistry::install_from(&registry);
    let spec = skill_spec();
    assert_eq!(spec.name, "skill");
    assert!(spec.description.contains("alpha") && spec.description.contains("beta"));
    // 禁用其一后 description 不再列它（toggle 后下一 run 生效的动态面）
    registry.set_enabled("beta", false);
    SkillRegistry::install_from(&registry);
    let spec2 = skill_spec();
    assert!(spec2.description.contains("alpha"));
    assert!(!spec2.description.contains("beta"));
    // 空 registry：兜底文案（工具仍暴露）
    SkillRegistry::install_from(&SkillRegistry::default());
    assert!(skill_spec().description.contains("没有可用"));
}

#[test]
fn snapshot_fixed_within_run() {
    // Q20：快照持有正文全文，查询面（find/body_text/is_rejected）只读快照。
    let snapshot = SkillSnapshot {
        entries: vec![SkillSnapshotEntry {
            name: "demo".to_owned(),
            description: "简介".to_owned(),
            body: "正文全文 BODY".to_owned(),
            fingerprint: "fp".to_owned(),
            untrusted: false,
        }],
        rejected: vec!["denied".to_owned()],
    };
    let entry = snapshot.find("demo").expect("快照命中");
    assert_eq!(entry.body, "正文全文 BODY");
    let text = snapshot.body_text("demo").unwrap();
    assert!(text.starts_with("以下是技能 demo 的正文："));
    assert!(text.contains("正文全文 BODY"));
    // 未信任：is_rejected 命中且 find 不命中（调用按 E_PROJECT_UNTRUSTED 拒）
    assert!(snapshot.is_rejected("denied"));
    assert!(snapshot.find("denied").is_none());
    assert!(!snapshot.is_rejected("demo"));
    assert!(snapshot.find("missing").is_none());
    assert!(snapshot.body_text("missing").is_none());
}
