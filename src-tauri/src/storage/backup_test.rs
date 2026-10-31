//! 备份守卫与恢复保护序列测试（建立、扩展）。
//! 同型用例在单个 #[test] 内顺序合并，断言消息带用例标识。

use super::{
    BACKUP_ITEMS, BACKUP_MANIFEST_FILE, RESTORE_GUIDE_FILE, RESTORE_PENDING_NAME,
    apply_restore_protection, ensure_upgrade_backup, restore_pending_path,
};
use crate::storage::config_store::ConfigStore;
use crate::storage::trust::{FileTrustStore, TrustDecision, TrustRecord, TrustScope, TrustStore};
use crate::util::atomic::write_atomic;

// 断言：ensure_upgrade_backup 全守卫——有数据时首次调用创建备份目录 + 升级
// 标记且权威数据完整可读；标记存在时幂等放行；全新安装直接写标记零负载；
// 全清单进备份且排除面（cache/logs/trust/workspace/ui-state 等）不进、
// manifest/恢复说明/恢复标记三件齐备；unix 下凭据文件权限复制不放宽（0600）。
#[test]
fn ensure_upgrade_backup_manifest_and_guards() {
    // 首次备份：权威数据都进备份，不嵌套备份，升级标记已写
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_atomic(&root.join("config.json"), br#"{"schemaVersion":2}"#).expect("write config");
    std::fs::create_dir_all(root.join("histories").join("sess-1")).expect("mkdir");
    std::fs::write(
        root.join("histories").join("sess-1").join("0001.jsonl"),
        "{\"kind\":\"session\"}\n",
    )
    .expect("write segment");
    std::fs::create_dir_all(root.join("sessions")).expect("mkdir sessions");
    std::fs::write(
        root.join("sessions").join("index.json"),
        b"{\"sessions\":[]}",
    )
    .expect("index");

    let backup = ensure_upgrade_backup(root, "compaction").expect("备份应成功");
    assert!(
        backup.starts_with(root.join("backups")),
        "备份应在 backups/ 下：{backup:?}"
    );
    assert_eq!(
        std::fs::read_to_string(backup.join("config.json")).expect("备份 config"),
        r#"{"schemaVersion":2}"#,
        "权威 config 进备份"
    );
    assert!(
        backup
            .join("histories")
            .join("sess-1")
            .join("0001.jsonl")
            .exists(),
        "会话日志进了备份"
    );
    assert!(
        backup.join("sessions").join("index.json").exists(),
        "索引进了备份"
    );
    assert!(!backup.join("backups").exists(), "备份不应嵌套备份");
    assert!(
        root.join("backups")
            .join(".upgrade-compaction.done")
            .exists(),
        "升级标记已写"
    );

    // 幂等：标记存在时不再产生新备份目录
    let count = |root: &std::path::Path| -> usize {
        std::fs::read_dir(root.join("backups"))
            .expect("backups 目录")
            .filter(|e| {
                e.as_ref()
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .starts_with("pre-")
            })
            .count()
    };
    assert_eq!(count(root), 1);
    let second = ensure_upgrade_backup(root, "compaction").expect("第二次应幂等放行");
    assert_eq!(count(root), 1, "不应产生第二份备份");
    assert_eq!(backup, second, "幂等路径一致");

    // manifest/恢复说明/恢复标记：另起全新根，造全清单与排除面
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_atomic(&root.join("config.json"), b"{\"schemaVersion\":2}").expect("config");
    write_atomic(&root.join("mcp.json"), br#"{"mcpServers":{}}"#).expect("mcp");
    write_atomic(
        &root.join("skills-state.json"),
        br#"{"version":1,"disabled":[]}"#,
    )
    .expect("skills");
    for item in ["histories", "sessions", "projects", "stats"] {
        std::fs::create_dir_all(root.join(item)).expect("mkdir");
        std::fs::write(root.join(item).join("data.txt"), "x").expect("write");
    }
    std::fs::create_dir_all(root.join("cache")).expect("cache");
    std::fs::write(root.join("cache").join("index.db"), b"db").expect("db");
    std::fs::create_dir_all(root.join("logs")).expect("logs");
    std::fs::write(root.join("logs").join("app.log"), b"log").expect("log");
    std::fs::create_dir_all(root.join("trust")).expect("trust");
    std::fs::write(root.join("trust").join("project.json"), b"[]").expect("trust");
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    std::fs::write(root.join("workspace").join("user.txt"), b"u").expect("ws");
    std::fs::write(root.join("ui-state.json"), b"{}").expect("ui-state");
    std::fs::write(root.join("window-state.json"), b"{}").expect("win-state");
    std::fs::write(root.join("quota.json"), b"[]").expect("quota");

    let backup = ensure_upgrade_backup(root, "compaction").expect("备份成功");
    let manifest_bytes = std::fs::read(backup.join(BACKUP_MANIFEST_FILE)).expect("manifest 存在");
    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest_bytes).expect("manifest JSON");
    assert_eq!(manifest["version"], 1, "manifest 版本 = 1");
    assert_eq!(manifest["reason"], "compaction");
    let items = manifest["items"].as_array().expect("items 数组").clone();
    assert_eq!(items.len(), BACKUP_ITEMS.len(), "全清单命中：{items:?}");
    for excluded in [
        "cache",
        "logs",
        "trust",
        "workspace",
        "ui-state.json",
        "window-state.json",
        "quota.json",
        "backups",
    ] {
        assert!(
            !backup.join(excluded).exists(),
            "排除面 {excluded} 不应进备份"
        );
    }
    let guide = std::fs::read_to_string(backup.join(RESTORE_GUIDE_FILE)).expect("恢复说明存在");
    assert!(guide.contains("退出 CraftWave"), "恢复说明含关闭应用步骤");
    assert!(
        guide.contains(".restore-pending.json"),
        "恢复说明含标记复制步骤"
    );
    assert!(guide.contains("备份时点"), "恢复说明含时点警告");
    assert!(
        backup.join(RESTORE_PENDING_NAME).exists(),
        "恢复标记随备份生成"
    );

    // 全新安装（无权威数据）：直接写标记放行，无空备份负载
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let backup = ensure_upgrade_backup(root, "compaction").expect("应放行");
    assert!(
        root.join("backups")
            .join(".upgrade-compaction.done")
            .exists(),
        "标记应存在"
    );
    let entries = std::fs::read_dir(&backup).expect("backup 目录").count();
    assert_eq!(entries, 0, "空安装无备份负载");

    // unix：凭据文件权限复制后不放宽（0600 保持）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let config = root.join("config.json");
        std::fs::write(&config, b"{\"apiKey\":\"sk-secret\"}").expect("config");
        let mut perms = std::fs::metadata(&config).expect("meta").permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&config, perms.clone()).expect("chmod 600");
        let backup = ensure_upgrade_backup(root, "compaction").expect("备份成功");
        let copied_mode = std::fs::metadata(backup.join("config.json"))
            .expect("备份 config")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            copied_mode,
            perms.mode() & 0o777,
            "备份的凭据文件权限应与原文件一致（0600 不放宽）"
        );
    }
}

// 断言：失败守卫与恢复保护序列——config.json 不可读时守卫失败、原数据未动、
// 标记未写（暂停升级语义，unix 以 000 权限模拟）；恢复保护全序列：无标记
// 零动作，有标记时信任清空、遗留 mcp-tools 删除、外部授权清空、审批白名单
// 清空、未暂停队列强制暂停（已暂停保持、损坏文件记账不静默）、标记删除且
// 幂等不重复触发。
#[test]
fn restore_protection_and_failure_guard() {
    // 失败守卫（unix）：不可读的权威数据让守卫失败，不写标记、原数据未动
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let config = root.join("config.json");
        std::fs::write(&config, b"{\"secret\":1}").expect("write config");
        let mut perms = std::fs::metadata(&config).expect("meta").permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&config, perms).expect("chmod 000");
        let result = ensure_upgrade_backup(root, "compaction");
        // 恢复权限（tempdir 清理需要）
        let mut perms = std::fs::metadata(&config).expect("meta").permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&config, perms).expect("chmod 644");
        assert!(result.is_err(), "不可读的权威数据应让守卫失败");
        assert_eq!(
            std::fs::read_to_string(&config).expect("原 config 未被改动"),
            "{\"secret\":1}",
            "原数据不受失败备份影响"
        );
        assert!(
            !root
                .join("backups")
                .join(".upgrade-compaction.done")
                .exists(),
            "失败不得写升级标记"
        );
    }

    // 恢复保护序列
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // ① 普通重启：无标记零动作
    let none = apply_restore_protection(root, None);
    assert!(!none.applied, "无标记不触发保护");
    assert!(none.actions.is_empty() && none.failures.is_empty());

    // 造恢复现场：
    // - 信任存储有 remember 记录 + 遗留 mcp-tools.json
    // - index.json 两个会话各带外部授权
    // - config.json 白名单两条
    // - 两个队列文件（一个未暂停、一个已暂停）+ 一个未知版本损坏文件
    let trust = FileTrustStore::new(root);
    trust
        .decide(TrustRecord {
            scope: TrustScope::Project,
            resource_name: "AGENTS.md".to_owned(),
            fingerprint: "fp-project".to_owned(),
            decided_at: 1,
            decision: TrustDecision::Remember.as_str().to_owned(),
        })
        .expect("写信任");
    trust
        .decide(TrustRecord {
            scope: TrustScope::Mcp,
            resource_name: String::new(),
            fingerprint: "fp-mcp".to_owned(),
            decided_at: 1,
            decision: TrustDecision::Remember.as_str().to_owned(),
        })
        .expect("写 MCP 信任");
    std::fs::create_dir_all(root.join("trust")).expect("trust 目录");
    std::fs::write(
        root.join("trust").join("mcp-tools.json"),
        br#"[{"fingerprint":"fp-tool"}]"#,
    )
    .expect("mcp-tools");

    let config_path = root.join("config.json");
    write_atomic(
        &config_path,
        br#"{"schemaVersion":2,"approval":{"commandAllowlist":["git status","cargo test"]}}"#,
    )
    .expect("config");
    let config = ConfigStore::new(config_path.clone());

    std::fs::create_dir_all(root.join("sessions")).expect("sessions");
    for (sid, paused) in [("s-a", false), ("s-b", true)] {
        write_atomic(
            &root.join("sessions").join(format!("{sid}.queue.json")),
            format!(
                r#"{{"version":1,"paused":{paused},"entries":[{{"id":"fu-1","text":"待发","createdAt":1,"runConfig":{{"tier":"confirmEach","maxTurns":24}}}}]}}"#
            )
            .as_bytes(),
        )
        .expect("队列文件");
    }
    // 损坏队列文件：保护序列按失败记账（不静默），不中断其他面
    std::fs::write(
        root.join("sessions").join("s-broken.queue.json"),
        b"{broken",
    )
    .expect("坏文件");
    let index = crate::storage::index_store::SessionIndex::new(root);
    for sid in ["s-a", "s-b"] {
        index
            .upsert(crate::storage::SessionMeta {
                id: sid.to_owned(),
                project_id: None,
                title: "会话".to_owned(),
                created_at: 1,
                updated_at: 1,
                revision: 1,
                message_count: 1,
                total_tokens: 0,
                authorized_external_dirs: vec!["/tmp/external".to_owned()],
            })
            .expect("写索引");
    }

    // 放置恢复标记（模拟用户按说明复制回）
    std::fs::write(restore_pending_path(root), b"{\"version\":1}").expect("标记");

    let report = apply_restore_protection(root, Some(&config));
    assert!(report.applied, "有标记应执行保护");
    // 信任双域全清空
    assert!(
        trust.entries().is_empty(),
        "恢复后信任存储应清空（重新确认）：{:?}",
        trust.entries()
    );
    assert!(
        !root.join("trust").join("mcp-tools.json").exists(),
        "MCP 工具级许可应删除"
    );
    // 外部授权清空
    for sid in ["s-a", "s-b"] {
        assert!(
            index
                .get(sid)
                .expect("会话仍在")
                .authorized_external_dirs
                .is_empty(),
            "{sid} 外部授权应清空"
        );
    }
    // 白名单清空（走 ConfigStore 唯一写入口，缓存热替换一致）
    assert!(
        config
            .load()
            .expect("读配置")
            .approval
            .command_allowlist
            .is_empty(),
        "审批白名单应清空（防恢复已撤销许可）"
    );
    // 队列：未暂停的 s-a 被强制暂停；已暂停的 s-b 保持；损坏文件记账不静默
    let queue_value = |sid: &str| -> serde_json::Value {
        serde_json::from_str(
            &std::fs::read_to_string(root.join("sessions").join(format!("{sid}.queue.json")))
                .expect("读队列"),
        )
        .expect("解析")
    };
    assert_eq!(queue_value("s-a")["paused"], true, "未暂停队列被强制暂停");
    assert_eq!(
        queue_value("s-a")["entries"]
            .as_array()
            .expect("条目")
            .len(),
        1,
        "暂停不丢待发消息"
    );
    assert_eq!(queue_value("s-b")["paused"], true, "已暂停保持");
    assert!(
        report
            .failures
            .iter()
            .any(|f| f.contains("s-broken.queue.json")),
        "损坏队列文件应进失败清单（不静默）：{:?}",
        report.failures
    );
    // 标记删除（防重复触发）+ 幂等：再跑一遍零动作
    assert!(!restore_pending_path(root).exists(), "保护完成后标记应删除");
    let again = apply_restore_protection(root, Some(&config));
    assert!(!again.applied, "标记已删不应重复触发");
}
