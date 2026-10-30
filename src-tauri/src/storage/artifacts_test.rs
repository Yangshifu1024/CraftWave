//! 产物登记测试（独立测试文件，28 号票）：登记合并（首写/重复计数）、
//! 损坏兜底、list 存在探测（临时目录造删）、会话删除 GC 联动。

use super::artifacts::{ARTIFACTS_FORMAT_VERSION, artifacts_path, list, load, record};
use super::session_tree::SessionStore;

/// 临时数据根（每测试独立，互不串扰）。
fn root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// 首写登记：新路径追加一条 runCount=1 的记录，文件结构字段齐全。
#[test]
fn record_first_write_appends_entry() {
    let dir = root();
    record(dir.path(), "s1", "run-a", "/proj/a.txt").unwrap();
    let file = load(dir.path(), "s1");
    assert_eq!(file.version, ARTIFACTS_FORMAT_VERSION);
    assert_eq!(file.artifacts.len(), 1);
    let entry = &file.artifacts[0];
    assert_eq!(entry.path, "/proj/a.txt");
    assert_eq!(entry.first_run_id, "run-a");
    assert_eq!(entry.run_count, 1);
    assert_eq!(entry.first_seen_at, entry.last_seen_at);
}

/// 重复命中：同路径再登记 runCount+1、lastSeenAt 刷新，不追加新条目；
/// 不同路径照常追加（去重键 = 规范化路径字符串）。
#[test]
fn record_repeat_increments_run_count() {
    let dir = root();
    record(dir.path(), "s1", "run-a", "/proj/a.txt").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    record(dir.path(), "s1", "run-b", "/proj/a.txt").unwrap();
    record(dir.path(), "s1", "run-b", "/proj/other.txt").unwrap();
    let file = load(dir.path(), "s1");
    assert_eq!(file.artifacts.len(), 2);
    let hit = file
        .artifacts
        .iter()
        .find(|a| a.path == "/proj/a.txt")
        .unwrap();
    assert_eq!(hit.run_count, 2);
    assert_eq!(hit.first_run_id, "run-a"); // 首登信息保留
    assert!(hit.last_seen_at >= hit.first_seen_at);
}

/// 同一会话多 run 登记：各自记 firstRunId（不同文件互不影响）。
#[test]
fn record_distinguishes_runs() {
    let dir = root();
    record(dir.path(), "s1", "run-1", "/proj/one.txt").unwrap();
    record(dir.path(), "s1", "run-2", "/proj/two.txt").unwrap();
    let file = load(dir.path(), "s1");
    assert_eq!(file.artifacts.len(), 2);
    let one = file
        .artifacts
        .iter()
        .find(|a| a.path == "/proj/one.txt")
        .unwrap();
    let two = file
        .artifacts
        .iter()
        .find(|a| a.path == "/proj/two.txt")
        .unwrap();
    assert_eq!(one.first_run_id, "run-1");
    assert_eq!(two.first_run_id, "run-2");
}

/// 损坏兜底：JSON 损坏按空清单处理，下一次登记覆盖重建（宁缺勿炸）。
#[test]
fn load_corrupted_file_falls_back_to_empty() {
    let dir = root();
    std::fs::create_dir_all(dir.path().join("sessions")).unwrap();
    std::fs::write(artifacts_path(dir.path(), "s1"), b"not json at all").unwrap();
    assert!(load(dir.path(), "s1").artifacts.is_empty());
    // 登记照常工作（覆盖坏文件）
    record(dir.path(), "s1", "run-a", "/proj/a.txt").unwrap();
    assert_eq!(load(dir.path(), "s1").artifacts.len(), 1);
}

/// list 存在探测：文件在 → exists=true；删除后 → exists=false（symlink
/// 解析后判定由 fs::metadata 天然承担——断链 symlink 记不存在）。
#[test]
fn list_probes_existence() {
    let dir = root();
    let live = dir.path().join("live.txt");
    std::fs::write(&live, "x").unwrap();
    let gone = dir.path().join("gone.txt");
    std::fs::write(&gone, "x").unwrap();
    record(dir.path(), "s1", "run-a", live.to_string_lossy().as_ref()).unwrap();
    record(dir.path(), "s1", "run-a", gone.to_string_lossy().as_ref()).unwrap();
    std::fs::remove_file(&gone).unwrap();

    let rows = list(dir.path(), "s1");
    assert_eq!(rows.len(), 2);
    let live_row = rows.iter().find(|r| r.path.ends_with("live.txt")).unwrap();
    let gone_row = rows.iter().find(|r| r.path.ends_with("gone.txt")).unwrap();
    assert!(live_row.exists);
    assert!(!gone_row.exists);
}

/// list 排序：存在优先，其余按 lastSeenAt 倒序（灰显置底语义）。
#[test]
fn list_sorts_existing_first_then_recent() {
    let dir = root();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "x").unwrap(); // b 不落盘（不存在）
    record(dir.path(), "s1", "r", a.to_string_lossy().as_ref()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    record(dir.path(), "s1", "r", b.to_string_lossy().as_ref()).unwrap();
    let rows = list(dir.path(), "s1");
    // 存在的 a 在前；不存在的 b 在后（虽然 b 更新）
    assert!(rows[0].path.ends_with("a.txt"));
    assert!(rows[1].path.ends_with("b.txt"));
}

/// 会话删除 GC：`<sid>.artifacts.json` 随 delete_session 一并清除
/// （§7.5 toolres 边车 GC 同款口径）。
#[test]
fn delete_session_removes_artifacts_file() {
    let dir = root();
    let store = SessionStore::with_root(dir.path().to_path_buf());
    store
        .index()
        .upsert(super::super::SessionMeta {
            id: "s-del".to_string(),
            project_id: None,
            title: "会话".to_string(),
            created_at: 0,
            updated_at: 0,
            revision: 0,
            message_count: 0,
            total_tokens: 0,
            authorized_external_dirs: Vec::new(),
        })
        .unwrap();
    let store_ref = &store;
    store_ref
        .record_artifact("s-del", "run-a", "/proj/a.txt")
        .unwrap();
    assert!(artifacts_path(dir.path(), "s-del").exists());
    store.delete_session("s-del").unwrap();
    assert!(!artifacts_path(dir.path(), "s-del").exists());
    // 删除后查询 → 空清单（文件已清）
    assert!(store.list_artifacts("s-del").is_empty());
}

/// SessionStore 登记/查询直驱：锁内串行语义（同会话多次登记计数正确）。
#[test]
fn store_record_and_list_roundtrip() {
    let dir = root();
    let store = SessionStore::with_root(dir.path().to_path_buf());
    store.record_artifact("s-x", "run-1", "/p/one.txt").unwrap();
    store.record_artifact("s-x", "run-2", "/p/one.txt").unwrap();
    store.record_artifact("s-x", "run-2", "/p/two.txt").unwrap();
    let rows = store.list_artifacts("s-x");
    assert_eq!(rows.len(), 2);
    let one = rows.iter().find(|r| r.path == "/p/one.txt").unwrap();
    assert_eq!(one.run_count, 2);
}

/// symlink 目标：登记记录 resolve 后的规范路径（执行器侧已完成解析）；
/// 本测试从存储面验证断链 symlink 的 exists=false 判定。
#[test]
fn list_broken_symlink_reports_missing() {
    let dir = root();
    let target = dir.path().join("target.txt");
    std::fs::write(&target, "x").unwrap();
    #[cfg(unix)]
    {
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        record(dir.path(), "s1", "r", link.to_string_lossy().as_ref()).unwrap();
        std::fs::remove_file(&target).unwrap(); // 断链
        let rows = list(dir.path(), "s1");
        assert!(!rows[0].exists);
    }
    #[cfg(not(unix))]
    {
        let _ = target;
    }
}
