//! 更新器支撑命令测试：update_status 禁用门、update_precheck
//! 安装门三阻止面（活跃 run / 应用级未决询问 / 未暂停队列）、队列磁盘全量
//! 扫描与 pause_followup_queue 主动暂停持久化。

use std::sync::Arc;

use super::updater::{pause_followup_queue_core, update_precheck_core, update_status_core};
use craftwave_engine::app_state::AppState;
use craftwave_engine::followup::FollowUpEntry;
use craftwave_engine::safety::tier::PermissionTier;
use craftwave_engine::util::atomic::write_atomic;
use craftwave_engine::util::events::RecordingEventSink;

/// 测试脚手架：临时数据目录上的 AppState（事件汇用录制桩）。
fn app(data: &std::path::Path) -> AppState {
    AppState::new_in(data.to_path_buf(), Arc::new(RecordingEventSink::new()))
}

/// 造一条待发消息（字段值无关紧要，只关心条数）。
fn entry(id: &str) -> FollowUpEntry {
    FollowUpEntry {
        id: id.to_owned(),
        text: "待发消息".to_owned(),
        created_at: 1,
        run_config: craftwave_engine::followup::QueuedRunConfig {
            model_id: None,
            thinking: None,
            tier: PermissionTier::ConfirmEach,
            max_turns: 24,
        },
        attachments: Vec::new(),
    }
}

/// 断言两道更新门与队列支撑面：update_status 禁用门——占位空串 pubkey →
/// 入口禁用兜底、实配 pubkey 放行、endpoints 原样透传、装配缺失全空；
/// update_precheck 三阻止面——活跃 run / 应用级未决询问 / 未暂停队列各自
/// 产生人话 reason 且 blocked=true，已暂停队列只计数不阻止，空队列不阻止，
/// 全部干净时放行；磁盘全量扫描——从未在内存触碰过的会话队列也被计入
/// （惰性加载不漏扫），损坏文件保守计入「未暂停」（更新门从严），已暂停
/// 文件保持；主动暂停命令——未暂停队列置暂停并持久化（queue.json 落盘，
/// 重启后惰性加载仍暂停）、暂停不丢待发消息、空 sessionId 拒绝、明确继续
/// 后解除暂停（门放行路径闭环）。
#[test]
fn update_gates_queue_scan_and_pause_persistence() {
    // 禁用门。
    let placeholder =
        update_status_core(Some(""), vec!["https://example.com/latest.json".to_owned()]);
    assert!(
        !placeholder.pubkey_configured,
        "占位空串 pubkey = 未实配，入口禁用"
    );
    assert_eq!(
        placeholder.endpoints.len(),
        1,
        "endpoints 照常透传（诊断面）"
    );
    let configured = update_status_core(
        Some("dW50cnVzdGVkIGNvbW1lbnQ6"),
        vec!["https://e/x.json".to_owned()],
    );
    assert!(configured.pubkey_configured, "实配 pubkey 后放行");
    assert_eq!(configured.endpoints, vec!["https://e/x.json".to_owned()]);
    let missing = update_status_core(None, Vec::new());
    assert!(
        !missing.pubkey_configured && missing.endpoints.is_empty(),
        "装配缺失 = 全空兜底"
    );

    // 安装门：全干净放行。
    let clean = update_precheck_core(0, 0, &[]);
    assert!(
        !clean.blocked && clean.reasons.is_empty(),
        "无任何活跃面应放行"
    );
    // 活跃 run。
    let running = update_precheck_core(2, 0, &[]);
    assert!(running.blocked);
    assert!(
        running.reasons[0].contains("2 个会话"),
        "阻止原因带数量：{:?}",
        running.reasons
    );
    // 应用级未决询问。
    let asks = update_precheck_core(0, 1, &[]);
    assert!(asks.blocked && asks.reasons[0].contains("应用级请求"));
    // 未暂停队列阻止；已暂停队列只计数不阻止。
    let queues = vec![
        (
            "s-a".to_owned(),
            craftwave_engine::followup::QueueSnapshot {
                size: 2,
                paused: false,
            },
        ),
        (
            "s-b".to_owned(),
            craftwave_engine::followup::QueueSnapshot {
                size: 1,
                paused: true,
            },
        ),
    ];
    let queued = update_precheck_core(0, 0, &queues);
    assert!(queued.blocked);
    assert_eq!(
        queued.running_queue_count, 1,
        "只有未暂停的非空队列计入阻止面"
    );
    assert_eq!(
        queued.paused_queue_count, 1,
        "已暂停队列单独计数（更新后保持暂停）"
    );
    assert!(
        queued.reasons[0].contains("暂停"),
        "阻止原因指向暂停动作：{:?}",
        queued.reasons
    );
    // 空队列（size=0 未暂停——正常投递完的残留形态）不阻止。
    let empty = update_precheck_core(
        0,
        0,
        &[(
            "s-c".to_owned(),
            craftwave_engine::followup::QueueSnapshot {
                size: 0,
                paused: false,
            },
        )],
    );
    assert!(!empty.blocked, "空队列不阻止更新");

    // 队列支撑面：磁盘全量扫描。
    let dir = tempfile::tempdir().expect("tempdir");
    let state = app(dir.path());
    std::fs::create_dir_all(dir.path().join("sessions")).expect("sessions");
    for (sid, paused) in [("s-a", false), ("s-b", true)] {
        write_atomic(
            &dir.path()
                .join("sessions")
                .join(format!("{sid}.queue.json")),
            format!(r#"{{"version":1,"paused":{paused},"entries":[]}}"#).as_bytes(),
        )
        .expect("队列文件");
    }
    std::fs::write(
        dir.path().join("sessions").join("s-broken.queue.json"),
        b"{",
    )
    .expect("坏文件");

    // 磁盘全量扫描面。
    let snapshots = state.followups.all_snapshots();
    assert_eq!(
        snapshots.len(),
        3,
        "磁盘三文件全扫（内存零触碰）：{snapshots:?}"
    );
    let find = |sid: &str| {
        snapshots
            .iter()
            .find(|(s, _)| s == sid)
            .map(|(_, q)| q.clone())
    };
    assert!(!find("s-a").expect("s-a").paused, "未暂停态读出");
    assert!(find("s-b").expect("s-b").paused, "已暂停态读出");
    let broken = find("s-broken").expect("损坏文件也应出现");
    assert!(
        !broken.paused && broken.size > 0,
        "损坏文件保守计入未暂停（更新门从严）"
    );

    // 主动暂停命令：置暂停并持久化。
    state.followups.push("s-1", entry("fu-1"));
    state.followups.push("s-1", entry("fu-2"));
    let result = pause_followup_queue_core(&state, "s-1").expect("暂停成功");
    assert_eq!(result.size, 2, "暂停不丢待发消息");
    assert!(result.paused);
    let file = std::fs::read_to_string(dir.path().join("sessions").join("s-1.queue.json"))
        .expect("队列文件存在");
    let value: serde_json::Value = serde_json::from_str(&file).expect("JSON");
    assert_eq!(value["paused"], true, "暂停态落盘");
    assert_eq!(value["entries"].as_array().expect("条目").len(), 2);
    // 空 sessionId 拒绝。
    let err = pause_followup_queue_core(&state, " ").expect_err("空 id 应拒绝");
    assert_eq!(err.code, "E_INVALID_ARG");
    // 队列继续（resume）后快照恢复未暂停。
    state.followups.set_paused("s-1", false);
    assert!(
        !state.followups.snapshot("s-1").paused,
        "明确继续后解除暂停"
    );
}
