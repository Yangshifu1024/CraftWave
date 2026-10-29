//! 备份守卫测试（Q34/Q36，04 号票）。

use super::ensure_upgrade_backup;
use crate::util::atomic::write_atomic;

// 断言：有数据时首次调用创建备份目录 + 升级标记，权威数据完整可读。
#[test]
fn first_call_creates_backup_with_authoritative_data() {
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
    // 权威数据都进了备份
    assert_eq!(
        std::fs::read_to_string(backup.join("config.json")).expect("备份 config"),
        r#"{"schemaVersion":2}"#
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
    // 排除面：cache/logs 不复制（不存在则无事发生）；备份自身不含 backups
    assert!(!backup.join("backups").exists(), "备份不应嵌套备份");
    // 升级标记已写
    assert!(
        root.join("backups")
            .join(".upgrade-compaction.done")
            .exists()
    );
}

// 断言：标记存在时幂等放行，不再产生新备份目录。
#[test]
fn marker_present_is_idempotent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("config.json"), b"{}").expect("write config");

    let first = ensure_upgrade_backup(root, "compaction").expect("第一次备份");
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
    assert_eq!(first, second, "幂等路径一致");
}

// 断言：全新安装（无任何权威数据）直接写标记放行，不产生空备份负担。
#[test]
fn empty_install_marks_done_without_backup_payload() {
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
}

// 断言：config.json 不可读（权限拒绝）时守卫失败、原数据未动、标记未写——
// 暂停升级语义。（unix 下以 000 权限模拟；windows 权限模型不同，跳过。）
#[cfg(unix)]
#[test]
fn backup_failure_keeps_original_data_and_no_marker() {
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
