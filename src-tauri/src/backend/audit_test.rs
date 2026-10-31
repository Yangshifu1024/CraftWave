//! 灰区命令审计纯逻辑面单元测试（36 号票，§8.7）：模型输出解析、提示词
//! 骨架构建、证据收集（授权目录边界 / 缺失标注 / 预算截断）。

use std::path::PathBuf;

use super::audit::{
    AuditDecision, AuditPromptInput, EvidenceCollector, MAX_EVIDENCE_FILES, MAX_FILE_BYTES,
    MAX_TOTAL_BYTES, bad_output_reason, build_audit_prompt, parse_verdict,
};

// ---- parse_verdict：严格 JSON 三态 + 前后缀容忍 + 坏输出拒绝 ----

#[test]
fn parse_verdict_three_decisions() {
    for (text, want_decision) in [
        (
            r#"{"decision":"allow","reason":"常规构建"}"#,
            AuditDecision::Allow,
        ),
        (
            r#"{"decision":"block","reason":"外传数据"}"#,
            AuditDecision::Block,
        ),
        (
            r#"{"decision":"ask","reason":"证据不足"}"#,
            AuditDecision::Ask,
        ),
    ] {
        let verdict = parse_verdict(text).unwrap_or_else(|| panic!("应解析：{text}"));
        assert_eq!(verdict.decision, want_decision);
    }
}

#[test]
fn parse_verdict_tolerates_surrounding_text() {
    let verdict =
        parse_verdict("好的，结论如下：\n{\"decision\":\"ask\",\"reason\":\"范围不明\"}\n以上。")
            .expect("前后缀文本应被容忍");
    assert_eq!(verdict.decision, AuditDecision::Ask);
    assert_eq!(verdict.reason, "范围不明");
}

#[test]
fn parse_verdict_missing_reason_normalized_empty() {
    let verdict = parse_verdict(r#"{"decision":"allow"}"#).expect("reason 缺失归一空串");
    assert_eq!(verdict.decision, AuditDecision::Allow);
    assert_eq!(verdict.reason, "");
}

#[test]
fn parse_verdict_rejects_bad_outputs() {
    for text in [
        "",
        "不是 JSON",
        "{\"reason\":\"缺 decision\"}",
        "{\"decision\":\"maybe\"}",
        "allow",
        "{broken",
        "{\"decision\":\"allow\",",
    ] {
        assert!(parse_verdict(text).is_none(), "应拒绝：{text}");
    }
}

#[test]
fn bad_output_reason_truncates_sample() {
    let long = "x".repeat(500);
    let reason = bad_output_reason(&long);
    assert!(reason.starts_with("审计输出无效："));
    // 采样截断：不超过采样上限（多字节安全按字符计）。
    assert!(reason.chars().count() < 500);
}

// ---- build_audit_prompt：骨架完整性 + 截断 + 空值兜底 ----

#[test]
fn prompt_contains_all_sections() {
    let prompt = build_audit_prompt(&AuditPromptInput {
        command: "touch /tmp/x",
        cwd: "/proj",
        evidence: "[授权目录] /proj",
        user_goal: Some("帮我创建文件"),
        scope: Some("已批准计划范围：/proj/out"),
    });
    assert!(prompt.contains("## 待审命令\ntouch /tmp/x"));
    assert!(prompt.contains("## 工作目录\n/proj"));
    assert!(prompt.contains("## 证据（应用在授权目录内只读收集）\n[授权目录] /proj"));
    assert!(prompt.contains("帮我创建文件"));
    assert!(prompt.contains("已批准计划范围：/proj/out"));
    // 判定要求关键句：诱导文字不作指令 + 证据不足必须 ask + 只输出 JSON。
    assert!(prompt.contains("待审查数据"));
    assert!(prompt.contains("证据不足时必须选 ask"));
    assert!(prompt.contains("{\"decision\":\"allow\""));
}

#[test]
fn prompt_empty_goal_and_scope_fallback() {
    let prompt = build_audit_prompt(&AuditPromptInput {
        command: "ls",
        cwd: "/proj",
        evidence: "",
        user_goal: None,
        scope: None,
    });
    assert!(prompt.contains("## 用户目标（本次任务的用户输入）\n（无）"));
    assert!(prompt.contains("（无已批准计划范围）"));
    // 空证据块兜底为「证据不完整」标注。
    assert!(prompt.contains("证据不完整"));
}

#[test]
fn prompt_truncates_long_user_goal() {
    let long = "目标".repeat(2000);
    let prompt = build_audit_prompt(&AuditPromptInput {
        command: "ls",
        cwd: "/proj",
        evidence: "e",
        user_goal: Some(&long),
        scope: None,
    });
    let marker = "## 批准范围";
    let goal_section = prompt
        .split("## 用户目标（本次任务的用户输入）")
        .nth(1)
        .and_then(|s| s.split(marker).next())
        .unwrap_or_default();
    // 截断至 2000 字符（加标注前缀），不整段注入。
    assert!(goal_section.chars().count() < 2100, "目标段应被截断");
}

// ---- EvidenceCollector：授权边界 / 缺失标注 / 命令定义 / 预算 ----

/// 造收集器工作区：tempdir 项目根 + workspace 根（canonical 路径）。
fn fixture_roots(project: &std::path::Path, workspace: &std::path::Path) -> Vec<PathBuf> {
    vec![
        crate::safety::safe_join::canonicalize_best_effort(project).expect("项目根归一"),
        crate::safety::safe_join::canonicalize_best_effort(workspace).expect("workspace 归一"),
    ]
}

#[test]
fn evidence_reads_file_within_roots_and_marks_outside() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().join("proj");
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(project.join("data.txt"), "机密内容标记ABC").unwrap();

    let roots = fixture_roots(&project, &workspace);
    let collector = EvidenceCollector {
        roots: &roots,
        base: &project,
    };
    // 项目内文件读取；授权外路径只标注不读；不存在如实标注。
    let text = collector.collect("cat data.txt /etc/hostname missing.txt > /dev/null");
    assert!(text.contains("[授权目录]"), "应列出授权目录：{text}");
    assert!(text.contains("机密内容标记ABC"), "项目内文件应读出：{text}");
    assert!(text.contains("（授权目录外，未读取）"), "越界不读：{text}");
    assert!(text.contains("（不存在，未读取）"), "缺失标注：{text}");
}

#[test]
fn evidence_collects_scripts_and_makefile() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().join("proj");
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        project.join("package.json"),
        r#"{"name":"x","scripts":{"build":"tsc -p ."}}"#,
    )
    .unwrap();
    std::fs::write(project.join("Makefile"), "build:\n\tcargo build\n").unwrap();

    let roots = fixture_roots(&project, &workspace);
    let collector = EvidenceCollector {
        roots: &roots,
        base: &project,
    };
    let text = collector.collect("npm run build");
    assert!(
        text.contains("package.json scripts"),
        "scripts 段应进证据：{text}"
    );
    assert!(text.contains("tsc -p ."), "scripts 内容应读出：{text}");
    assert!(text.contains("Makefile 头部"), "Makefile 应进证据：{text}");
    assert!(text.contains("cargo build"));
}

#[test]
fn evidence_marks_parse_failure_and_opaque_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().join("proj");
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let roots = fixture_roots(&project, &workspace);
    let collector = EvidenceCollector {
        roots: &roots,
        base: &project,
    };
    // 解析失败（未闭合引号）→ 证据块标注。
    let failed = collector.collect("echo \"unclosed");
    assert!(failed.contains("命令解析失败"), "解析失败应标注：{failed}");
    // Opaque 写目标（$VAR）→ 无法读取标注。
    let opaque = collector.collect("echo hi > \"$OUT/file.txt\"");
    assert!(opaque.contains("含未解析变量"), "Opaque 应标注：{opaque}");
}

#[test]
fn evidence_budget_truncates_files_and_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().join("proj");
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    // 造 10 个项目内文件 + 1 个超预算大文件（> 总 16KB）。
    for i in 0..10 {
        std::fs::write(project.join(format!("f{i}.txt")), format!("内容{i}")).unwrap();
    }
    let big = "长".repeat(MAX_TOTAL_BYTES + 100);
    std::fs::write(project.join("big.txt"), &big).unwrap();

    let roots = fixture_roots(&project, &workspace);
    let collector = EvidenceCollector {
        roots: &roots,
        base: &project,
    };
    let args: Vec<String> = (0..10)
        .map(|i| format!("f{i}.txt"))
        .chain(std::iter::once("big.txt".to_owned()))
        .collect();
    let command = format!("cat {}", args.join(" "));
    let text = collector.collect(&command);
    // 文件数预算：最多 MAX_EVIDENCE_FILES 个文件读出 + 预算耗尽标注。
    let budget_note = text.contains("证据预算耗尽");
    let read_marker = text.matches("：\n内容").count() + text.matches("：\n长").count();
    assert!(
        read_marker <= MAX_EVIDENCE_FILES,
        "读出文件数不应超过预算：{read_marker}"
    );
    assert!(budget_note, "预算耗尽应有标注：{text}");
    // 单文件截断：大文件读出不超过单文件上限（字符数受限）。
    if text.contains("big.txt") {
        let big_section = text.split("big.txt").nth(1).unwrap_or_default();
        assert!(
            big_section.chars().take(MAX_FILE_BYTES + 10).count() > 0,
            "大文件应有片段"
        );
    }
}

#[test]
fn evidence_no_paths_note() {
    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().join("proj");
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    let roots = fixture_roots(&project, &workspace);
    let collector = EvidenceCollector {
        roots: &roots,
        base: &project,
    };
    let text = collector.collect("echo hello");
    assert!(text.contains("（命令未涉及可识别的文件路径）"), "{text}");
}
