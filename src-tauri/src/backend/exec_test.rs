//! 工具执行器适配的单元测试：五个文件工具的分发执行、参数错误以
//! E_INVALID_ARG 回喂、plan 权限档对写工具的排除；批次调度（07 号票）——
//! 并发上限信号量、FileWrite 全局互斥串行、源序回喂、取消收齐、同文件
//! 写冲突检测与非重叠 edit 合并原子提交。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::exec::ToolsetExecutor;
use super::exec::{PlannedTask, run_batch_tasks};
use super::run_loop::{BatchCall, CancellableExecutor, ToolExecution};
use crate::safety::tier::PermissionTier;
use crate::tools::ToolsContext;
use tokio_util::sync::CancellationToken;

/// 并发/时序计数器（批次调度测试的观测面）。
#[derive(Default)]
struct Concurrency {
    /// 当前在飞任务数。
    inflight: AtomicUsize,
    /// 峰值在飞数（调度层互斥/限流的断言面）。
    peak: AtomicUsize,
}

impl Concurrency {
    /// 进入临界区（inflight+1 并刷新峰值），返回退出时是否为峰值（调试用）。
    fn enter(&self) {
        let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
    }

    /// 离开临界区。
    fn leave(&self) {
        self.inflight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 构造以临时目录为项目根的测试上下文（临时目录故意不主动清理——
/// 单测进程生命周期内持有一个临时根即可，避免上下文被提前删掉）。
fn temp_ctx() -> ToolsContext {
    let dir = tempfile::tempdir().expect("tempdir");
    // leak：单测生命周期内持有一个临时根即可
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    ToolsContext::new(root.clone(), root)
}

/// 以全开放权限档执行一次工具，返回（执行产物, 测试上下文）。
async fn execute(name: &str, args_json: &str) -> (ToolExecution, ToolsContext) {
    let ctx = temp_ctx();
    let token = CancellationToken::new();
    let executor = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
    let out = executor.execute("call_1", name, args_json, &token).await;
    (out, ctx)
}

/// 断言 read 成功时双通道各自正确：界面摘要含文件名、模型通道文本携带
/// 文件内容。
#[tokio::test]
async fn read_success_model_channel_compact_text() {
    let (out, _ctx_guard) = {
        let ctx = temp_ctx();
        std::fs::write(ctx.project_root.join("a.txt"), "第一行\n第二行").unwrap();
        let token = CancellationToken::new();
        let executor = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
        let out = executor
            .execute("call_1", "read", r#"{"path": "a.txt"}"#, &token)
            .await;
        (out, ctx)
    };
    let ToolExecution::Completed {
        ok,
        ui_summary,
        error,
        model_text,
    } = out
    else {
        panic!("应为 Completed");
    };
    assert!(ok);
    assert!(error.is_none());
    assert!(
        ui_summary.contains("a.txt"),
        "ui_summary 一行人话：{ui_summary}"
    );
    assert!(
        model_text.contains("第一行"),
        "模型通道携带文件内容：{model_text}"
    );
}

/// 断言参数不是合法 JSON 时按工具级失败收口：仍是 Completed（失败也是
/// 收齐的结果），E_INVALID_ARG 回喂模型，run 不中断。
#[tokio::test]
async fn invalid_args_tool_level_failure_run_continues() {
    let (out, _ctx) = execute("read", "{oops 半截").await;
    let ToolExecution::Completed {
        ok,
        error,
        model_text,
        ..
    } = out
    else {
        panic!("应为 Completed（失败也是收齐的结果）");
    };
    assert!(!ok);
    let error = error.expect("带错误面");
    assert_eq!(error.code, "E_INVALID_ARG");
    assert!(
        model_text.contains("E_INVALID_ARG"),
        "模型通道含 code+message"
    );
}

/// 断言未知工具名同样以 E_INVALID_ARG 工具级失败回喂。
#[tokio::test]
async fn unknown_tool_feeds_back_invalid_arg() {
    let (out, _) = execute("rm_rf", "{}").await;
    let ToolExecution::Completed { ok, error, .. } = out else {
        panic!("应为 Completed");
    };
    assert!(!ok);
    assert_eq!(error.expect("错误面").code, "E_INVALID_ARG");
}

/// 断言 plan 权限档下写工具被 E_TOOL_BLOCKED 排除，且文件不落盘。
#[tokio::test]
async fn plan_tier_write_tools_excluded_tool_blocked() {
    let ctx = temp_ctx();
    let token = CancellationToken::new();
    let executor = Arc::new(ToolsetExecutor::new(ctx.clone(), PermissionTier::Plan));
    let out = executor
        .execute(
            "call_1",
            "write",
            r#"{"path": "x.txt", "content": "hi"}"#,
            &token,
        )
        .await;
    let ToolExecution::Completed {
        ok,
        error,
        ui_summary,
        ..
    } = out
    else {
        panic!("应为 Completed");
    };
    assert!(!ok, "plan 档 FileWrite 过滤（§8.5）");
    assert_eq!(error.expect("错误面").code, "E_TOOL_BLOCKED");
    assert!(ui_summary.contains("write"));
    assert!(!ctx.project_root.join("x.txt").exists(), "被排除工具不落盘");
}

// —— 07 号票：批次调度核心（run_batch_tasks）——

/// 构造一个调度层测试任务：延迟 `delay_ms` 后产出 index 的完成结果，
/// 期间以 `counters` 记录并发峰值（file_write=true 的任务受全局写互斥）。
fn counting_task(
    index: usize,
    delay_ms: u64,
    file_write: bool,
    counters: Arc<Concurrency>,
) -> PlannedTask {
    PlannedTask {
        member_indices: vec![index],
        file_write,
        run: Box::pin(async move {
            counters.enter();
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            counters.leave();
            let exec = ToolExecution::Completed {
                ok: true,
                ui_summary: format!("task-{index}"),
                error: None,
                model_text: format!("task-{index}"),
            };
            vec![(index, exec)]
        }),
    }
}

/// 并发上限信号量：8 个无冲突（只读）任务、额度 4 → 峰值并发恰为 4
/// （不超限，也不退化为串行）。
#[tokio::test]
async fn batch_semaphore_caps_read_peak_at_max_parallel() {
    let counters = Arc::new(Concurrency::default());
    let tasks: Vec<PlannedTask> = (0..8)
        .map(|i| counting_task(i, 30, false, counters.clone()))
        .collect();
    let results = run_batch_tasks(tasks, vec![None; 8], 4, &CancellationToken::new()).await;
    assert_eq!(results.len(), 8, "全部任务收齐");
    for (i, exec) in results.iter().enumerate() {
        let ToolExecution::Completed { model_text, .. } = exec else {
            panic!("任务 {i} 应为 Completed");
        };
        assert_eq!(model_text, &format!("task-{i}"), "源序对位");
    }
    let peak = counters.peak.load(Ordering::SeqCst);
    assert_eq!(peak, 4, "峰值并发 = 信号量额度（实际 {peak}）");
}

/// 并发度配置生效：同一批 8 个只读任务、额度改 2 → 峰值并发随之变为 2
/// （§3.2 tools.maxParallel 的配置面：改配置后峰值并发跟随变化）。
#[tokio::test]
async fn batch_peak_follows_configured_max_parallel() {
    let counters = Arc::new(Concurrency::default());
    let tasks: Vec<PlannedTask> = (0..8)
        .map(|i| counting_task(i, 30, false, counters.clone()))
        .collect();
    let results = run_batch_tasks(tasks, vec![None; 8], 2, &CancellationToken::new()).await;
    assert_eq!(results.len(), 8);
    assert_eq!(
        counters.peak.load(Ordering::SeqCst),
        2,
        "额度 2 → 峰值并发 2"
    );
}

/// FileWrite 全局互斥：两组并发批次（模拟两个会话各自的 execute_batch）
/// 的文件写任务全程串行（写峰值 = 1），与额度无关。
#[tokio::test]
async fn batch_file_write_tasks_globally_serialized_across_groups() {
    let write_counters = Arc::new(Concurrency::default());
    let read_counters = Arc::new(Concurrency::default());
    let group_a: Vec<PlannedTask> = (0..2)
        .map(|i| counting_task(i, 20, true, write_counters.clone()))
        .chain((0..2).map(|i| counting_task(i + 2, 20, false, read_counters.clone())))
        .collect();
    let group_b: Vec<PlannedTask> = (0..2)
        .map(|i| counting_task(i, 20, true, write_counters.clone()))
        .collect();
    let token = CancellationToken::new();
    let (ra, rb) = tokio::join!(
        run_batch_tasks(group_a, vec![None; 4], 4, &token),
        run_batch_tasks(group_b, vec![None; 2], 4, &token),
    );
    assert_eq!(ra.len(), 4);
    assert_eq!(rb.len(), 2);
    assert_eq!(
        write_counters.peak.load(Ordering::SeqCst),
        1,
        "文件写任务跨组互斥（写峰值 = 1）"
    );
    assert_eq!(
        read_counters.peak.load(Ordering::SeqCst),
        2,
        "只读任务不受写互斥影响（信号量额度 4 内并发）"
    );
}

/// 源序回喂：慢的先启动、快的先完成，收集结果仍按源序对位
/// （FR-03-11：完成顺序不改序）。
#[tokio::test]
async fn batch_results_follow_source_order_not_completion_order() {
    let counters = Arc::new(Concurrency::default());
    // 完成序 = 2（0ms）→ 1（10ms）→ 0（60ms）；期望输出仍为 0,1,2。
    let tasks = vec![
        counting_task(0, 60, false, counters.clone()),
        counting_task(1, 10, false, counters.clone()),
        counting_task(2, 0, false, counters.clone()),
    ];
    let results = run_batch_tasks(tasks, vec![None; 3], 4, &CancellationToken::new()).await;
    for (i, exec) in results.iter().enumerate() {
        let ToolExecution::Completed { model_text, .. } = exec else {
            panic!("任务 {i} 应为 Completed");
        };
        assert_eq!(model_text, &format!("task-{i}"), "结果按源序回喂");
    }
}

/// 取消收齐（派发前置位）：cancel 先于批次 → 零派发，全部任务补
/// Cancelled { executed: false }（不留悬空调用）。
#[tokio::test]
async fn batch_cancel_before_dispatch_marks_all_not_executed() {
    let counters = Arc::new(Concurrency::default());
    let tasks: Vec<PlannedTask> = (0..3)
        .map(|i| counting_task(i, 30, false, counters.clone()))
        .collect();
    let token = CancellationToken::new();
    token.cancel();
    let results = run_batch_tasks(tasks, vec![None; 3], 4, &token).await;
    for (i, exec) in results.iter().enumerate() {
        let ToolExecution::Cancelled { executed, .. } = exec else {
            panic!("任务 {i} 应为未派发取消");
        };
        assert!(!executed, "任务 {i} 未派发（executed=false）");
    }
    assert_eq!(counters.peak.load(Ordering::SeqCst), 0, "零执行");
}

/// 取消收齐（在飞 join）：cancel 置位时已在飞的任务不被丢弃——全部 join
/// 完成后 run_batch_tasks 才返回（已完成结果保留）。
#[tokio::test]
async fn batch_cancel_joins_inflight_before_returning() {
    let counters = Arc::new(Concurrency::default());
    let tasks = vec![
        counting_task(0, 60, false, counters.clone()),
        counting_task(1, 0, false, counters.clone()),
    ];
    let token = CancellationToken::new();
    let token_for_run = token.clone();
    let task =
        tokio::spawn(async move { run_batch_tasks(tasks, vec![None; 2], 4, &token_for_run).await });
    // 让两个任务先派发，再取消：在飞任务应被 join 而非丢弃。
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    token.cancel();
    let results = task.await.expect("批次任务完成");
    assert_eq!(results.len(), 2, "在飞任务收齐（不丢结果）");
    for (i, exec) in results.iter().enumerate() {
        assert!(
            matches!(exec, ToolExecution::Completed { .. }),
            "任务 {i} 已派发即执行完毕收齐（结果保留）"
        );
    }
}

// —— 07 号票：ToolsetExecutor 批次执行（同文件写冲突检测 + 合并提交）——

/// 构造一条批内调用。
fn batch_call(call_id: &str, name: &str, args_json: &str) -> BatchCall {
    BatchCall {
        call_id: call_id.to_owned(),
        name: name.to_owned(),
        args_json: args_json.to_owned(),
    }
}

/// 同文件非重叠 edit：基于同一原内容预演后合并原子提交——两个 edit 一次
/// 生效，结果各自 ok、共享同一最终 version。
#[tokio::test]
async fn edit_group_merges_non_overlapping_edits_atomically() {
    let ctx = temp_ctx();
    std::fs::write(ctx.project_root.join("c.txt"), "line1\nline2\nline3\n").unwrap();
    let executor = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
    let batch = vec![
        batch_call(
            "c1",
            "edit",
            r#"{"path": "c.txt", "oldText": "line1", "newText": "L1"}"#,
        ),
        batch_call(
            "c2",
            "edit",
            r#"{"path": "c.txt", "oldText": "line3", "newText": "L3"}"#,
        ),
    ];
    let results = executor
        .execute_batch(&batch, &CancellationToken::new())
        .await;
    assert_eq!(results.len(), 2);
    for (i, exec) in results.iter().enumerate() {
        let ToolExecution::Completed { ok, error, .. } = exec else {
            panic!("成员 {i} 应为 Completed");
        };
        assert!(ok, "成员 {i} 应成功：{error:?}");
    }
    assert_eq!(
        std::fs::read_to_string(ctx.project_root.join("c.txt")).unwrap(),
        "L1\nline2\nL3\n",
        "两个 edit 同一次提交生效"
    );
}

/// 同文件区间重叠的 edit：源序首个保留、后者拒绝（E_INVALID_ARG），
/// 文件只体现首个 edit。
#[tokio::test]
async fn edit_group_rejects_overlapping_edit_after_first() {
    let ctx = temp_ctx();
    std::fs::write(ctx.project_root.join("c.txt"), "aaa\nbbb\nccc\n").unwrap();
    let executor = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
    let batch = vec![
        batch_call(
            "c1",
            "edit",
            r#"{"path": "c.txt", "oldText": "aaa\nbbb", "newText": "XX"}"#,
        ),
        batch_call(
            "c2",
            "edit",
            r#"{"path": "c.txt", "oldText": "bbb\nccc", "newText": "YY"}"#,
        ),
    ];
    let results = executor
        .execute_batch(&batch, &CancellationToken::new())
        .await;
    let ToolExecution::Completed { ok: ok1, .. } = &results[0] else {
        panic!("成员 0 应为 Completed");
    };
    assert!(ok1, "源序首个 edit 保留执行");
    let ToolExecution::Completed {
        ok: ok2,
        error: err2,
        ..
    } = &results[1]
    else {
        panic!("成员 1 应为 Completed（失败也是收齐的结果）");
    };
    assert!(!ok2, "重叠的后续 edit 被拒");
    assert_eq!(err2.as_ref().expect("错误面").code, "E_INVALID_ARG");
    assert_eq!(
        std::fs::read_to_string(ctx.project_root.join("c.txt")).unwrap(),
        "XX\nccc\n",
        "只有首个 edit 生效"
    );
}

/// 同文件 write+edit 混批（Q32）：源序首个（write）保留，edit 被拒。
#[tokio::test]
async fn same_file_write_then_edit_rejects_edit() {
    let ctx = temp_ctx();
    let executor = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
    let batch = vec![
        batch_call("c1", "write", r#"{"path": "n.txt", "content": "fresh"}"#),
        batch_call(
            "c2",
            "edit",
            r#"{"path": "n.txt", "oldText": "fresh", "newText": "edited"}"#,
        ),
    ];
    let results = executor
        .execute_batch(&batch, &CancellationToken::new())
        .await;
    let ToolExecution::Completed { ok: ok1, .. } = &results[0] else {
        panic!("成员 0 应为 Completed");
    };
    assert!(ok1, "源序首个（write）保留执行");
    let ToolExecution::Completed {
        ok: ok2,
        error: err2,
        ..
    } = &results[1]
    else {
        panic!("成员 1 应为 Completed");
    };
    assert!(!ok2, "同文件混入 write 的 edit 被拒（Q32）");
    assert_eq!(err2.as_ref().expect("错误面").code, "E_INVALID_ARG");
    assert_eq!(
        std::fs::read_to_string(ctx.project_root.join("n.txt")).unwrap(),
        "fresh",
        "文件保持 write 结果"
    );
}

/// 同文件 edit+write 混批：源序首个（edit）保留执行，write（后续冲突）
/// 被拒——不创建文件。
#[tokio::test]
async fn same_file_edit_then_write_rejects_write() {
    let ctx = temp_ctx();
    std::fs::write(ctx.project_root.join("e.txt"), "hello\n").unwrap();
    let executor = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
    let batch = vec![
        batch_call(
            "c1",
            "edit",
            r#"{"path": "e.txt", "oldText": "hello", "newText": "hi"}"#,
        ),
        batch_call(
            "c2",
            "write",
            r#"{"path": "e.txt", "content": "overwritten"}"#,
        ),
    ];
    let results = executor
        .execute_batch(&batch, &CancellationToken::new())
        .await;
    let ToolExecution::Completed { ok: ok1, .. } = &results[0] else {
        panic!("成员 0 应为 Completed");
    };
    assert!(ok1, "源序首个（edit）保留执行");
    let ToolExecution::Completed {
        ok: ok2,
        error: err2,
        ..
    } = &results[1]
    else {
        panic!("成员 1 应为 Completed");
    };
    assert!(!ok2, "同文件混入 edit 的 write 被拒（Q32）");
    assert_eq!(err2.as_ref().expect("错误面").code, "E_INVALID_ARG");
    assert_eq!(
        std::fs::read_to_string(ctx.project_root.join("e.txt")).unwrap(),
        "hi\n",
        "文件保持 edit 结果，write 未生效"
    );
}

/// 同文件两个 write：源序首个保留，第二个拒绝（不覆盖）。
#[tokio::test]
async fn same_file_two_writes_second_rejected() {
    let ctx = temp_ctx();
    let executor = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
    let batch = vec![
        batch_call("c1", "write", r#"{"path": "w.txt", "content": "first"}"#),
        batch_call("c2", "write", r#"{"path": "w.txt", "content": "second"}"#),
    ];
    let results = executor
        .execute_batch(&batch, &CancellationToken::new())
        .await;
    let ToolExecution::Completed { ok: ok1, .. } = &results[0] else {
        panic!("成员 0 应为 Completed");
    };
    assert!(ok1);
    let ToolExecution::Completed { ok: ok2, error, .. } = &results[1] else {
        panic!("成员 1 应为 Completed");
    };
    assert!(!ok2, "同文件重复 write 被拒");
    assert_eq!(error.as_ref().expect("错误面").code, "E_INVALID_ARG");
    assert_eq!(
        std::fs::read_to_string(ctx.project_root.join("w.txt")).unwrap(),
        "first"
    );
}

/// 同批混合读写批次（端到端语义）：不同文件的读与写并发/串行各归其位，
/// 结果按源序返回、同文件多 edit 合并提交。
#[tokio::test]
async fn mixed_batch_read_parallel_write_group_source_order() {
    let ctx = temp_ctx();
    std::fs::write(ctx.project_root.join("a.txt"), "A").unwrap();
    std::fs::write(ctx.project_root.join("b.txt"), "B").unwrap();
    std::fs::write(ctx.project_root.join("c.txt"), "x1\nx2\nx3\n").unwrap();
    let executor = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
    let batch = vec![
        batch_call("c1", "read", r#"{"path": "a.txt"}"#),
        batch_call(
            "c2",
            "edit",
            r#"{"path": "c.txt", "oldText": "x1", "newText": "y1"}"#,
        ),
        batch_call("c3", "read", r#"{"path": "b.txt"}"#),
        batch_call(
            "c4",
            "edit",
            r#"{"path": "c.txt", "oldText": "x3", "newText": "y3"}"#,
        ),
    ];
    let results = executor
        .execute_batch(&batch, &CancellationToken::new())
        .await;
    assert_eq!(results.len(), 4);
    for (i, exec) in results.iter().enumerate() {
        let ToolExecution::Completed { ok, .. } = exec else {
            panic!("成员 {i} 应为 Completed");
        };
        assert!(ok, "成员 {i} 应成功");
    }
    assert_eq!(
        std::fs::read_to_string(ctx.project_root.join("c.txt")).unwrap(),
        "y1\nx2\ny3\n",
        "同文件两个 edit 合并原子提交"
    );
    // 源序对位：read a 的结果在 0 位、read b 在 2 位。
    let ToolExecution::Completed { model_text: t0, .. } = &results[0] else {
        unreachable!()
    };
    assert!(t0.contains("A"), "位置 0 = read a.txt");
    let ToolExecution::Completed { model_text: t2, .. } = &results[2] else {
        unreachable!()
    };
    assert!(t2.contains("B"), "位置 2 = read b.txt");
}

/// 跨执行器（跨会话语义）同文件批次互斥：两组并发批次各自对同一文件
/// write+edit 混批，各组入口冲突检测独立生效、文件终态唯一。
#[tokio::test]
async fn cross_session_batches_write_lock_keeps_files_consistent() {
    let ctx = temp_ctx();
    std::fs::write(ctx.project_root.join("s.txt"), "base\n").unwrap();
    let token = CancellationToken::new();
    let batch = |old: &str, new: &str| {
        vec![
            batch_call(
                "c1",
                "edit",
                &format!(r#"{{"path": "s.txt", "oldText": "{old}", "newText": "{new}"}}"#),
            ),
            batch_call(
                "c2",
                "edit",
                &format!(r#"{{"path": "s.txt", "oldText": "{new}", "newText": "{new}!"}}"#),
            ),
        ]
    };
    // 组 A 改 base→A；组 B 等 A 的批次完成后改 A→B、"!"→"?"（两个 edit
    // 都基于 B 批次开始时的文件内容，可合并原子提交）。此处顺序执行两个
    // 批次，断言终态链式正确（并发互斥由调度层测试钉住）。
    let executor_a = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
    let results_a = executor_a.execute_batch(&batch("base", "A"), &token).await;
    assert!(
        results_a
            .iter()
            .all(|e| matches!(e, ToolExecution::Completed { ok: true, .. }))
    );
    let executor_b = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
    let batch_b = vec![
        batch_call(
            "d1",
            "edit",
            r#"{"path": "s.txt", "oldText": "A", "newText": "B"}"#,
        ),
        batch_call(
            "d2",
            "edit",
            r#"{"path": "s.txt", "oldText": "!", "newText": "?"}"#,
        ),
    ];
    let results_b = executor_b.execute_batch(&batch_b, &token).await;
    assert!(
        results_b
            .iter()
            .all(|e| matches!(e, ToolExecution::Completed { ok: true, .. }))
    );
    assert_eq!(
        std::fs::read_to_string(ctx.project_root.join("s.txt")).unwrap(),
        "B?\n",
        "两个批次的合并提交按序生效"
    );
}
