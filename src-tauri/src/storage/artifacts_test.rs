//! 产物登记测试（独立测试文件，28 号票）：登记合并（首写/重复计数）、
//! 损坏兜底、list 存在探测与排序、会话删除 GC 联动。同型用例在单个
//! #[test] 内顺序合并，断言消息带用例标识。

use super::artifacts::{ARTIFACTS_FORMAT_VERSION, artifacts_path, list, load, record};
use super::session_tree::SessionStore;

/// 临时数据根（每测试独立，互不串扰）。
fn root() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

// 断言：登记全语义——首写追加 runCount=1 且文件结构字段齐全；同路径再登记
// runCount+1、lastSeenAt 刷新不追加新条目，不同路径照常追加；同会话多 run
// 各自记 firstRunId；JSON 损坏按空清单处理且下一次登记覆盖重建（宁缺勿炸）。
#[test]
fn record_semantics_and_corrupt_fallback() {
    // 首写：新路径追加一条 runCount=1 的记录，字段齐全
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

    // 重复命中：runCount+1、lastSeenAt 刷新；不同路径照常追加（去重键 = 规范化路径）
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
    assert_eq!(hit.run_count, 2, "重复命中计数累加");
    assert_eq!(hit.first_run_id, "run-a", "首登信息保留");
    assert!(hit.last_seen_at >= hit.first_seen_at);

    // 同一会话多 run 登记：各自记 firstRunId（不同文件互不影响）
    record(dir.path(), "s1", "run-1", "/proj/one.txt").unwrap();
    record(dir.path(), "s1", "run-2", "/proj/two.txt").unwrap();
    let file = load(dir.path(), "s1");
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

    // 损坏兜底：按空清单处理，下一次登记覆盖重建
    let dir = root();
    std::fs::create_dir_all(dir.path().join("sessions")).unwrap();
    std::fs::write(artifacts_path(dir.path(), "s1"), b"not json at all").unwrap();
    assert!(load(dir.path(), "s1").artifacts.is_empty(), "损坏按空清单");
    record(dir.path(), "s1", "run-a", "/proj/a.txt").unwrap();
    assert_eq!(load(dir.path(), "s1").artifacts.len(), 1, "登记覆盖坏文件");
}

// 断言：list 与 SessionStore 直驱面——list 存在探测（文件在 true/删除后
// false）、排序存在优先其余按 lastSeenAt 倒序（灰显置底）；SessionStore
// 登记锁内串行计数正确；会话删除 GC 清掉 `<sid>.artifacts.json`。
#[test]
fn list_probe_sort_and_store_gc() {
    // 存在探测 + 排序：文件在 → true；删除后 → false（symlink 解析判定由
    // fs::metadata 天然承担，断链即不存在）
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
    assert!(live_row.exists, "文件在 → exists=true");
    assert!(!gone_row.exists, "删除后 → exists=false");

    // 排序：存在优先，其余按 lastSeenAt 倒序（灰显置底语义）
    let dir = root();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::fs::write(&a, "x").unwrap(); // b 不落盘（不存在）
    record(dir.path(), "s1", "r", a.to_string_lossy().as_ref()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    record(dir.path(), "s1", "r", b.to_string_lossy().as_ref()).unwrap();
    let rows = list(dir.path(), "s1");
    // 存在的 a 在前；不存在的 b 在后（虽然 b 更新）
    assert!(rows[0].path.ends_with("a.txt"), "存在的置前");
    assert!(rows[1].path.ends_with("b.txt"), "不存在的灰显置底");

    // SessionStore 直驱：锁内串行语义（同会话多次登记计数正确）+ 删除 GC
    let dir = root();
    let store = SessionStore::with_root(dir.path().to_path_buf());
    store
        .index()
        .upsert(crate::storage::SessionMeta {
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
    store
        .record_artifact("s-del", "run-1", "/p/one.txt")
        .unwrap();
    store
        .record_artifact("s-del", "run-2", "/p/one.txt")
        .unwrap();
    store
        .record_artifact("s-del", "run-2", "/p/two.txt")
        .unwrap();
    let rows = store.list_artifacts("s-del");
    assert_eq!(rows.len(), 2);
    let one = rows.iter().find(|r| r.path == "/p/one.txt").unwrap();
    assert_eq!(one.run_count, 2, "同会话多次登记计数正确");

    // GC：`<sid>.artifacts.json` 随 delete_session 一并清除（§7.5 toolres
    // 边车 GC 同款口径），删除后查询 → 空清单
    assert!(artifacts_path(dir.path(), "s-del").exists());
    store.delete_session("s-del").unwrap();
    assert!(
        !artifacts_path(dir.path(), "s-del").exists(),
        "随会话删除清除"
    );
    assert!(store.list_artifacts("s-del").is_empty());
}
