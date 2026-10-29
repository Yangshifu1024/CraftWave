//! RunManager 与 FrameHub 单元测试（测试代码独立于源码文件，仅 cargo test 时编译）。
//!
//! 覆盖（05 号票，§5.8/D16）：
//! - 注册表行为：同会话互斥（E_SESSION_RUNNING）、取消只做置位、终态摘除
//!   与摘除后允许重启、has_active 与注册表空/非空一致；
//! - 环形帧缓冲：容量 512 溢出丢最旧 + dropped 计数；
//! - attach_run 重放区间 `(sinceSeq, head]` 边界（0 / 中间 / 头 / 超越头）；
//! - 窗口耗尽兜底：sinceSeq 落在缓冲之前补 runSnapshot 摘要帧；
//! - 热切换：attach 后新帧只走新下发端，旧端断流；
//! - RunState.usage 累计（帧面 usage 同步进 RunState）与 run_snapshot 对账。

use std::sync::Arc;

use super::{FRAME_BUFFER_CAPACITY, FrameHub, RunManager};
use crate::backend::frames::{
    ChatFrame, EndReason, RunSnapshotStatus, SeqCollectingSink, SequencedSink, UsageTotal,
};

/// 共享收集器句柄（Arc + Mutex 内部可变性：SequencedSink 的 &mut self 与
/// 断言侧只读访问共用一份收集器）。
type SharedCollector = Arc<std::sync::Mutex<SeqCollectingSink>>;

/// 收集器快照（断言侧简写）。
fn collect(c: &SharedCollector) -> Vec<crate::backend::frames::SequencedFrame> {
    c.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .snapshot()
}

/// 构造一个带收集器下发端的枢纽（返回枢纽与收集器，测试双端断言用）。
fn hub_with_collector() -> (Arc<FrameHub>, SharedCollector) {
    let collector: SharedCollector = Arc::new(std::sync::Mutex::new(SeqCollectingSink::default()));
    let hub = Arc::new(FrameHub::new(Box::new(CloningSink(collector.clone()))));
    (hub, collector)
}

/// 把共享收集器包成 SequencedSink（收集器本体经 Arc+Mutex 共享，断言侧持另一份）。
#[derive(Clone)]
struct CloningSink(SharedCollector);

impl SequencedSink for CloningSink {
    fn send_seq(&mut self, frame: crate::backend::frames::SequencedFrame) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .send_seq(frame);
    }
}

/// 向枢纽推 n 个文本帧（delta 内嵌序号便于断言）。
fn push_text_frames(hub: &FrameHub, n: usize) {
    for i in 0..n {
        hub.push(ChatFrame::TextDelta {
            turn: 1,
            delta: format!("t{i}"),
        });
    }
}

// ---- 注册表行为（既有用例，适配 hub 入参） ----

/// 断言同会话的第二个 run 被拒（E_SESSION_RUNNING），不同会话不受影响。
#[test]
fn second_run_same_session_rejected() {
    let mgr = RunManager::new();
    mgr.start(
        "s1",
        "run-1".to_owned(),
        1,
        Arc::new(FrameHub::new(Box::new(SeqCollectingSink::default()))),
    )
    .expect("首 run 应注册");
    let err = mgr
        .start(
            "s1",
            "run-2".to_owned(),
            2,
            Arc::new(FrameHub::new(Box::new(SeqCollectingSink::default()))),
        )
        .unwrap_err();
    assert_eq!(err.code, "E_SESSION_RUNNING", "§3.3 #18 同会话互斥");
    // 不同会话不受影响
    mgr.start(
        "s2",
        "run-3".to_owned(),
        3,
        Arc::new(FrameHub::new(Box::new(SeqCollectingSink::default()))),
    )
    .expect("他会话可注册");
}

/// 断言 cancel 在无活跃 run 时返回 false；命中活跃 run 时置位令牌并返回 true。
#[test]
fn cancel_hits_active_run_true_missing_false() {
    let mgr = RunManager::new();
    assert!(!mgr.cancel("s1"), "无活跃 run → false");
    let handle = mgr
        .start(
            "s1",
            "run-1".to_owned(),
            1,
            Arc::new(FrameHub::new(Box::new(SeqCollectingSink::default()))),
        )
        .unwrap();
    assert!(!handle.cancel.is_cancelled());
    assert!(mgr.cancel("s1"), "命中 → 置位并返回 true（§3.3 #19）");
    assert!(handle.cancel.is_cancelled());
}

/// 断言 remove 会校验 run_id（不匹配不摘除，防止竞态摘掉新 run），
/// 正确摘除后同会话可以再次启动新 run。
#[test]
fn remove_validates_run_id_allows_restart() {
    let mgr = RunManager::new();
    mgr.start(
        "s1",
        "run-1".to_owned(),
        1,
        Arc::new(FrameHub::new(Box::new(SeqCollectingSink::default()))),
    )
    .unwrap();
    // run_id 不匹配不摘（防竞态摘掉新 run）
    mgr.remove("s1", "run-stale");
    assert!(mgr.get("s1").is_some());
    mgr.remove("s1", "run-1");
    assert!(mgr.get("s1").is_none(), "终态摘除");
    mgr.start(
        "s1",
        "run-2".to_owned(),
        2,
        Arc::new(FrameHub::new(Box::new(SeqCollectingSink::default()))),
    )
    .expect("摘除后同会话可再启动");
}

/// 断言 has_active 与注册表空/非空一致：空表放行应用退出，任一会话有
/// 活跃 run 则拦截退出。
#[test]
fn has_active_reflects_registry_emptiness() {
    let mgr = RunManager::new();
    assert!(!mgr.has_active(), "空注册表 → 无活跃 run（退出拦截放行）");
    mgr.start(
        "s1",
        "run-1".to_owned(),
        1,
        Arc::new(FrameHub::new(Box::new(SeqCollectingSink::default()))),
    )
    .unwrap();
    assert!(mgr.has_active(), "任一会话有活跃 run → 拦截退出（§9.6）");
    mgr.remove("s1", "run-1");
    assert!(!mgr.has_active());
}

// ---- 环形帧缓冲（§5.8/D16，05 号票） ----

/// 断言环形缓冲容量 512：溢出丢最旧，dropped 计数准确，缓冲内 seq 连续。
#[test]
fn ring_buffer_overflows_oldest_dropped_counted() {
    let (hub, collector) = hub_with_collector();
    // 推容量 + 10 帧：最旧 10 帧被丢
    let total = FRAME_BUFFER_CAPACITY + 10;
    push_text_frames(&hub, total);
    let outcome_dropped = hub.dropped();
    assert_eq!(outcome_dropped, 10, "溢出 10 帧 → dropped 计数");
    // 下发端照常收到全部帧（入环丢帧不影响续流下发）
    assert_eq!(collect(&collector).len(), total, "下发不受丢帧影响");
    // RunState.last_seq 以枢纽实际序号为准（最后一帧 seq = total - 1）
    assert_eq!(hub.last_seq(), (total - 1) as u64);
}

/// 断言重放区间 `(sinceSeq, head]` 的四个边界：0（全量）/中间/头（空）/
/// 超越头（空），且重放帧原样携带 seq/ts。
#[test]
fn attach_replay_window_boundaries() {
    let (hub, _collector) = hub_with_collector();
    push_text_frames(&hub, 5); // seq 0..=4
    let probe: SharedCollector = Arc::new(std::sync::Mutex::new(SeqCollectingSink::default()));

    // ① sinceSeq=0 → 全量重放 5 帧（seq 0..=4，原 seq/ts 保留）
    let outcome = hub.attach(
        0,
        placeholder_snapshot(),
        Box::new(CloningSink(probe.clone())),
    );
    assert_eq!(outcome.replayed, 5, "sinceSeq=0 → 全量重放");
    assert!(!outcome.snapshot_sent, "无缺口不补摘要帧");
    let replayed = collect(&probe);
    assert_eq!(replayed.len(), 5);
    for (i, frame) in replayed.iter().enumerate() {
        assert_eq!(frame.seq, i as u64, "重放帧原 seq 不变");
        assert!(frame.ts.is_some(), "重放帧携带 ts");
    }

    // ② sinceSeq=2（中间）→ 重放 seq 3、4 两帧
    let probe2: SharedCollector = Arc::new(std::sync::Mutex::new(SeqCollectingSink::default()));
    let outcome = hub.attach(
        2,
        placeholder_snapshot(),
        Box::new(CloningSink(probe2.clone())),
    );
    assert_eq!(outcome.replayed, 2, "sinceSeq=2 → (2,4] 两帧");
    let seqs: Vec<u64> = collect(&probe2).iter().map(|f| f.seq).collect();
    assert_eq!(seqs, vec![3, 4]);

    // ③ sinceSeq=head（4）→ 空重放
    let probe3: SharedCollector = Arc::new(std::sync::Mutex::new(SeqCollectingSink::default()));
    let outcome = hub.attach(
        4,
        placeholder_snapshot(),
        Box::new(CloningSink(probe3.clone())),
    );
    assert_eq!(outcome.replayed, 0, "sinceSeq=头 → 空重放");
    assert!(collect(&probe3).is_empty());

    // ④ sinceSeq 超越头（99）→ 空重放且不补摘要（区间为空，无缺口语义）
    let probe4: SharedCollector = Arc::new(std::sync::Mutex::new(SeqCollectingSink::default()));
    let outcome = hub.attach(
        99,
        placeholder_snapshot(),
        Box::new(CloningSink(probe4.clone())),
    );
    assert_eq!(outcome.replayed, 0, "超越头 → 空重放");
    assert!(!outcome.snapshot_sent, "超越头不补摘要");
    assert!(collect(&probe4).is_empty());
}

/// 断言窗口耗尽兜底：缓冲只留最近 3 帧（前面 2 帧被溢出丢弃）时，
/// sinceSeq=0 的 attach 补发 runSnapshot 摘要帧且 lastSeq 正确。
#[test]
fn attach_snapshot_frame_when_window_exhausted() {
    let (hub, _collector) = hub_with_collector();
    // 只推 3 帧不满环——用手工缩短窗口的办法不可行（容量常量），改为
    // 直接构造小容量场景：推满 512 + 2 帧后最旧 2 帧被丢，环内最旧 seq=2，
    // sinceSeq=0 → (0, 2) 有不可见缺口 → 补摘要。
    push_text_frames(&hub, FRAME_BUFFER_CAPACITY + 2);
    let probe: SharedCollector = Arc::new(std::sync::Mutex::new(SeqCollectingSink::default()));
    let outcome = hub.attach(
        0,
        placeholder_snapshot(),
        Box::new(CloningSink(probe.clone())),
    );
    assert!(outcome.snapshot_sent, "窗口耗尽 → 补 runSnapshot 摘要帧");
    let frames = collect(&probe);
    // 重放帧 = 环内 512 帧 + 摘要帧 1 帧
    assert_eq!(outcome.replayed, FRAME_BUFFER_CAPACITY as u64);
    assert_eq!(frames.len(), FRAME_BUFFER_CAPACITY + 1);
    let last = frames.last().expect("有摘要帧");
    assert_eq!(
        last.seq,
        (FRAME_BUFFER_CAPACITY + 2) as u64,
        "摘要帧占用新 seq 号"
    );
    let ChatFrame::RunSnapshot { snapshot } = &last.frame else {
        panic!("末帧应为 runSnapshot 摘要帧，实际 {:?}", last.frame);
    };
    assert_eq!(snapshot.status, RunSnapshotStatus::Running);
    assert_eq!(
        snapshot.last_seq,
        (FRAME_BUFFER_CAPACITY + 1) as u64,
        "lastSeq = 摘要帧之前的最大帧序号"
    );

    // 对照组：sinceSeq=2（恰好等于环内最旧帧 seq）→ 无缺口，不补摘要；
    // 重放 [3, head] = 环内 seq 3..=513 共 511 帧
    let probe2: SharedCollector = Arc::new(std::sync::Mutex::new(SeqCollectingSink::default()));
    let outcome = hub.attach(
        2,
        placeholder_snapshot(),
        Box::new(CloningSink(probe2.clone())),
    );
    assert!(!outcome.snapshot_sent, "sinceSeq 覆盖到缺口边缘不补摘要");
    assert_eq!(outcome.replayed, (FRAME_BUFFER_CAPACITY - 1) as u64);
}

/// 断言热切换：attach 后新帧只走新下发端，旧端断流；重放帧先于新帧发出
/// （切换在锁内完成，帧序严格不乱）。
#[test]
fn attach_hot_switch_new_frames_go_to_new_sink_only() {
    let (hub, old_collector) = hub_with_collector();
    push_text_frames(&hub, 3); // seq 0..=2 走旧端
    assert_eq!(collect(&old_collector).len(), 3);

    let new_collector: SharedCollector =
        Arc::new(std::sync::Mutex::new(SeqCollectingSink::default()));
    let outcome = hub.attach(
        0,
        placeholder_snapshot(),
        Box::new(CloningSink(new_collector.clone())),
    );
    assert_eq!(outcome.replayed, 3, "重放 3 帧走新端");

    // 切换后新帧只走新端：旧端停在新帧出现前的数量
    push_text_frames(&hub, 2); // seq 3、4
    assert_eq!(
        collect(&old_collector).len(),
        3,
        "热切换后旧端断流（不再收新帧）"
    );
    let new_frames = collect(&new_collector);
    assert_eq!(new_frames.len(), 5, "新端 = 重放 3 帧 + 新帧 2 帧");
    let seqs: Vec<u64> = new_frames.iter().map(|f| f.seq).collect();
    assert_eq!(seqs, vec![0, 1, 2, 3, 4], "重放帧先于新帧，帧序严格递增");
}

/// 断言 RunState.usage 累计：usage 帧同步由 EventfulSink 承担（chat.rs），
/// 这里验证 run_snapshot 的对账面——RunState 写入后快照字段完整，
/// last_seq 以枢纽为准。
#[test]
fn run_snapshot_state_fields_and_usage() {
    let mgr = RunManager::new();
    let (hub, _collector) = hub_with_collector();
    let handle = mgr
        .start("s-usage", "run-usage".to_owned(), 1000, hub.clone())
        .unwrap();
    push_text_frames(&hub, 3);
    // 模拟 run 任务的 RunState 同步（EventfulSink 路径）：turn/tool_count/usage
    {
        let mut state = handle.state.lock().unwrap();
        state.turn = 2;
        state.tool_count = 1;
        state.usage = UsageTotal {
            input: 120,
            output: 48,
            cache_read: 0,
            cache_write: 0,
        };
    }
    let snapshot = handle.run_snapshot();
    assert_eq!(snapshot.status, RunSnapshotStatus::Running);
    assert_eq!(snapshot.run_id.as_deref(), Some("run-usage"));
    assert_eq!(snapshot.started_at, Some(1000));
    assert_eq!(snapshot.turn, Some(2));
    assert_eq!(snapshot.tool_count, Some(1));
    assert_eq!(
        snapshot.usage,
        UsageTotal {
            input: 120,
            output: 48,
            cache_read: 0,
            cache_write: 0
        },
        "usage 累计随 RunState 同步"
    );
    assert_eq!(snapshot.last_seq, 2, "lastSeq 以帧枢纽实际序号为准");
    // RunHandle::attach 的摘要快照同面（构造侧用同一数据源）
    let probe: SharedCollector = Arc::new(std::sync::Mutex::new(SeqCollectingSink::default()));
    handle.attach(0, Box::new(CloningSink(probe.clone())));
    let last = collect(&probe).last().cloned();
    // sinceSeq=0 无缺口（环内 3 帧全在）→ 无摘要帧；直接断言重放面
    assert_eq!(collect(&probe).len(), 3);
    let _ = last;
}

/// 无活跃 run 的空 runSnapshot 占位（RunSnapshot 契约形状锚点：idle 形态
/// 由命令面 null 承载，不构造快照对象）。
#[test]
fn empty_snapshot_has_running_shape() {
    let snap = placeholder_snapshot();
    assert_eq!(snap.status, RunSnapshotStatus::Running);
    assert_eq!(snap.last_seq, 0);
    assert_eq!(snap.usage, UsageTotal::default(), "占位快照 usage 为零值");
    // 终态原因枚举仍被帧面使用（防未用告警面）
    let _ = EndReason::Done;
}

/// attach 用占位快照（真实数据由 RunHandle::attach 构造；hub 直驱测试用）。
fn placeholder_snapshot() -> crate::backend::frames::RunSnapshot {
    crate::backend::frames::RunSnapshot {
        status: RunSnapshotStatus::Running,
        run_id: Some("run-test".to_owned()),
        started_at: Some(1),
        turn: Some(1),
        tool_count: Some(0),
        usage: UsageTotal::default(),
        last_seq: 0,
    }
}
