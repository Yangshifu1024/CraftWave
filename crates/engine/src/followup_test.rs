//! followUp 队列管理器的单元测试：入队/出队/移除/清空/回队/暂停态、
//! queue.json 原子写与重启（重开管理器）恢复往返、损坏档容错。

use super::*;

/// 临时数据根（测试结束自动清理）。
fn temp_root() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// 造一条待发消息的简写（固定 tier/maxTurns，模型覆盖可选）。
fn entry(text: &str, model_id: Option<&str>) -> FollowUpEntry {
    FollowUpEntry {
        id: format!("fu-{}", text.len()),
        text: text.to_owned(),
        created_at: 1_760_000_000_000,
        run_config: QueuedRunConfig {
            model_id: model_id.map(str::to_owned),
            thinking: None,
            tier: crate::safety::tier::PermissionTier::AutoEdit,
            max_turns: 24,
        },
        attachments: Vec::new(),
    }
}

/// 断言内存队列操作语义：先进先出出队、按 id 移除（未命中 None）、
/// 清空按排队序交还并解除暂停且删除落盘文件、出队后回队仍为队头
/// （交付失败重试的挂回语义）；以及 queue.json 原子写与重启（重开管理器）
/// 恢复往返——字段级格式完整落盘（paused 与 entries 随行），暂停态
/// 跨重启保留。
#[test]
fn queue_operations_requeue_and_pause_across_restart() {
    let dir = temp_root();
    let mgr = FollowUpManager::new(dir.path().to_path_buf());
    let sid = "sess-q";

    // 入队与快照。
    assert_eq!(mgr.snapshot(sid).size, 0, "空会话快照为空");
    assert_eq!(mgr.push(sid, entry("第一条", None)).size, 1);
    assert_eq!(mgr.push(sid, entry("第二条", None)).size, 2);
    assert_eq!(mgr.snapshot(sid).size, 2);

    // 队头出队 = 先进先出；按 id 移除；未命中返回 None。
    let head = mgr.pop_head(sid).expect("队头应存在");
    assert_eq!(head.text, "第一条");
    assert_eq!(mgr.snapshot(sid).size, 1);
    let removed = mgr.remove(sid, &entry("第二条", None).id).expect("应命中");
    assert_eq!(removed.text, "第二条");
    assert_eq!(mgr.snapshot(sid).size, 0);
    assert!(mgr.remove(sid, "missing").is_none(), "未命中返回 None");

    // 出队后回队（交付失败重试）：回队后仍为队头。
    mgr.push(sid, entry("队头", None));
    mgr.push(sid, entry("队二", None));
    let head = mgr.pop_head(sid).expect("出队");
    assert_eq!(mgr.snapshot(sid).size, 1);
    mgr.push_front(sid, head);
    assert_eq!(
        mgr.peek_head(sid).expect("回队").text,
        "队头",
        "回队后仍为队头"
    );
    assert_eq!(mgr.snapshot(sid).size, 2);

    // 清空：按排队序交还、解除暂停、删除落盘文件（空队列无信息量，
    // 重启惰性加载即空态）。
    let sid = "sess-c";
    mgr.push(sid, entry("甲", None));
    mgr.push(sid, entry("乙", None));
    mgr.set_paused(sid, true);
    let returned = mgr.clear(sid);
    assert_eq!(
        returned,
        vec!["甲".to_owned(), "乙".to_owned()],
        "按排队序交还"
    );
    let snap = mgr.snapshot(sid);
    assert_eq!(snap.size, 0);
    assert!(!snap.paused, "清空同时解除暂停（取消返还后无暂停意义）");
    assert!(
        !dir.path()
            .join("sessions")
            .join("sess-c.queue.json")
            .exists(),
        "清空后队列文件应删除"
    );

    // queue.json 原子写与重启（重开管理器）恢复往返。
    let dir = temp_root();
    let root = dir.path().to_path_buf();

    // 第一"次运行"：入队两条 + 置暂停
    {
        let mgr = FollowUpManager::new(root.clone());
        mgr.push("sess-r", entry("待发一", Some("model-a")));
        mgr.push("sess-r", entry("待发二", None));
        mgr.set_paused("sess-r", true);
    }

    // 落盘文件即字段级格式（paused 与 entries 完整保留）
    let raw = std::fs::read_to_string(root.join("sessions").join("sess-r.queue.json"))
        .expect("queue.json 应存在");
    let file: FollowUpQueueFile = serde_json::from_str(&raw).expect("应为合法 JSON");
    assert_eq!(file.version, QUEUE_FILE_VERSION);
    assert!(file.paused, "暂停态已落盘");
    assert_eq!(file.entries.len(), 2);
    assert_eq!(file.entries[0].text, "待发一");
    assert_eq!(
        file.entries[0].run_config.model_id.as_deref(),
        Some("model-a"),
        "附着 runConfig 快照随行"
    );

    // 第二"次运行"（新管理器 = 重启）：惰性恢复队列与暂停态
    let mgr = FollowUpManager::new(root);
    let snap = mgr.snapshot("sess-r");
    assert_eq!(snap.size, 2);
    assert!(snap.paused, "暂停态跨重启保留");
    let head = mgr.peek_head("sess-r").expect("队头应恢复");
    assert_eq!(head.text, "待发一");
}

/// 表驱动：损坏档各形态整体按空队列重建——非法 JSON 与未来版本号都不半读
/// （暂停态也不采纳）。
#[test]
fn corrupt_or_unknown_version_file_rebuilds_empty() {
    let dir = temp_root();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join("sessions")).expect("建目录");
    let future = serde_json::json!({ "version": 99, "paused": true, "entries": [] });
    for (name, sid, body) in [
        ("非法 JSON", "sess-x", b"{not json".to_vec()),
        ("未来版本号", "sess-v", future.to_string().into_bytes()),
    ] {
        std::fs::write(
            root.join("sessions").join(format!("{sid}.queue.json")),
            body,
        )
        .expect("写坏档");
        let mgr = FollowUpManager::new(root.clone());
        let snap = mgr.snapshot(sid);
        assert_eq!(snap.size, 0, "「{name}」应按空队列重建（不半可用）");
        assert!(!snap.paused, "「{name}」暂停态不采纳");
    }
}
