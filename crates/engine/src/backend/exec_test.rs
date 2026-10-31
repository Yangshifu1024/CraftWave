//! 工具执行器适配的单元测试：五个文件工具的分发执行、参数错误以
//! E_INVALID_ARG 回喂、plan 权限档对写工具的排除；批次调度——
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

/// 断言单工具分发各面：read 成功时双通道各自正确（界面摘要一行人话、模型
/// 通道携带内容）；参数非合法 JSON / 未知工具名均按工具级失败收口（仍是
/// Completed，E_INVALID_ARG 回喂模型，run 不中断）；plan 权限档下写工具被
/// E_TOOL_BLOCKED 排除且文件不落盘（权限矩阵）。
#[tokio::test]
async fn single_tool_dispatch_cases() {
    // ① read 成功：ui_summary 与 model_text 双通道
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

    // ② 参数非合法 JSON：工具级失败 + E_INVALID_ARG
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
    assert_eq!(error.expect("带错误面").code, "E_INVALID_ARG");
    assert!(
        model_text.contains("E_INVALID_ARG"),
        "模型通道含 code+message：{model_text}"
    );

    // ③ 未知工具名：同样 E_INVALID_ARG 工具级失败
    let (out, _) = execute("rm_rf", "{}").await;
    let ToolExecution::Completed { ok, error, .. } = out else {
        panic!("应为 Completed");
    };
    assert!(!ok);
    assert_eq!(error.expect("错误面").code, "E_INVALID_ARG");

    // ④ plan 档写工具排除：E_TOOL_BLOCKED 且不落盘
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
    assert!(!ok, "plan 档 FileWrite 过滤（权限矩阵）");
    assert_eq!(error.expect("错误面").code, "E_TOOL_BLOCKED");
    assert!(ui_summary.contains("write"));
    assert!(!ctx.project_root.join("x.txt").exists(), "被排除工具不落盘");
}

// —— 批次调度核心（run_batch_tasks）——

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

/// 断言批次调度各面：并发上限信号量跟随配置额度（8 任务 × 额度 4 → 峰值
/// 4、额度 2 → 峰值 2，不超限也不退化串行，tools.maxParallel）；源序
/// 回喂（完成顺序不改序）；FileWrite 全局互斥跨执行器组串行
/// （写峰值 = 1，只读不受影响）；取消收齐两态——派发前置位零派发全部补
/// Cancelled { executed: false }、在飞置位则 join 收齐不丢结果。
#[tokio::test]
async fn batch_scheduling_cases() {
    // ① 信号量额度表：(额度, 期望峰值并发)
    for (limit, want_peak) in [(4usize, 4usize), (2, 2)] {
        let counters = Arc::new(Concurrency::default());
        let tasks: Vec<PlannedTask> = (0..8)
            .map(|i| counting_task(i, 30, false, counters.clone()))
            .collect();
        let results = run_batch_tasks(tasks, vec![None; 8], limit, &CancellationToken::new()).await;
        assert_eq!(results.len(), 8, "全部任务收齐（额度 {limit}）");
        let peak = counters.peak.load(Ordering::SeqCst);
        assert_eq!(
            peak, want_peak,
            "额度 {limit} → 峰值并发 {want_peak}（实际 {peak}）"
        );
    }

    // ② 源序回喂：完成序 2（0ms）→ 1（10ms）→ 0（60ms），期望输出仍 0,1,2
    let counters = Arc::new(Concurrency::default());
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

    // ③ FileWrite 全局互斥：两组并发批次的写任务全程串行（写峰值 = 1）
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

    // ④ 取消（派发前置位）：零派发，全部补 Cancelled { executed: false }
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

    // ⑤ 取消（在飞 join）：cancel 置位时在飞任务不被丢弃，join 完成后返回
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

// —— ToolsetExecutor 批次执行（同文件写冲突检测 + 合并提交）——

/// 构造一条批内调用。
fn batch_call(call_id: &str, name: &str, args_json: &str) -> BatchCall {
    BatchCall {
        call_id: call_id.to_owned(),
        name: name.to_owned(),
        args_json: args_json.to_owned(),
    }
}

/// 断言同文件分组两态：非重叠 edit 合并原子提交（两个 edit 基于同一原
/// 内容预演后一次提交生效、共享同一最终 version）+ 冲突检测表（源序
/// 首个保留、后续冲突成员 E_INVALID_ARG 拒绝且不生效）——edit 区间重叠、
/// write 后混入 edit、edit 后混入 write、重复 write——四类冲突文件终态都
/// 只体现源序首个成员。
#[tokio::test]
async fn same_file_group_cases() {
    // ① 非重叠合并：两个 edit 同一次提交生效
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

    // ② 冲突表：源序首个保留、冲突成员拒绝且不生效
    // (目标文件, 初始内容可空, 首个成员, 冲突成员, 期望文件终态)
    let cases: Vec<(&str, Option<&str>, BatchCall, BatchCall, &str)> = vec![
        // edit 区间重叠：首个保留、后者拒绝
        (
            "c.txt",
            Some("aaa\nbbb\nccc\n"),
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
            "XX\nccc\n",
        ),
        // write + edit 混批：edit 被拒
        (
            "n.txt",
            None,
            batch_call("c1", "write", r#"{"path": "n.txt", "content": "fresh"}"#),
            batch_call(
                "c2",
                "edit",
                r#"{"path": "n.txt", "oldText": "fresh", "newText": "edited"}"#,
            ),
            "fresh",
        ),
        // edit + write 混批：write 被拒、不创建覆盖
        (
            "e.txt",
            Some("hello\n"),
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
            "hi\n",
        ),
        // 重复 write：第二个拒绝（不覆盖）
        (
            "w.txt",
            None,
            batch_call("c1", "write", r#"{"path": "w.txt", "content": "first"}"#),
            batch_call("c2", "write", r#"{"path": "w.txt", "content": "second"}"#),
            "first",
        ),
    ];
    for (idx, (path, seed, first, conflicting, want_content)) in cases.into_iter().enumerate() {
        let ctx = temp_ctx();
        if let Some(content) = seed {
            std::fs::write(ctx.project_root.join(path), content).unwrap();
        }
        let executor = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
        let results = executor
            .execute_batch(&[first, conflicting], &CancellationToken::new())
            .await;
        let ToolExecution::Completed { ok: ok1, .. } = &results[0] else {
            panic!("用例 {idx}：成员 0 应为 Completed");
        };
        assert!(ok1, "用例 {idx}：源序首个成员保留执行");
        let ToolExecution::Completed {
            ok: ok2,
            error: err2,
            ..
        } = &results[1]
        else {
            panic!("用例 {idx}：成员 1 应为 Completed（失败也是收齐的结果）");
        };
        assert!(!ok2, "用例 {idx}：冲突成员应被拒");
        assert_eq!(
            err2.as_ref().expect("错误面").code,
            "E_INVALID_ARG",
            "用例 {idx}：冲突拒绝码"
        );
        assert_eq!(
            std::fs::read_to_string(ctx.project_root.join(path)).unwrap(),
            want_content,
            "用例 {idx}：文件只体现首个成员"
        );
    }
}

/// 断言混合批次端到端语义：不同文件的读与写并发/串行各归其位、结果按源
/// 序返回、同文件多 edit 合并提交；跨执行器（跨会话语义）批次链式提交
///——两组各自对同一文件 edit 混批，冲突检测独立生效、文件终态唯一。
#[tokio::test]
async fn mixed_batch_and_cross_executor_cases() {
    // ① 混批：read a + edit c（两处）+ read b → 全部成功、源序对位、合并提交
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
    let ToolExecution::Completed { model_text: t0, .. } = &results[0] else {
        unreachable!()
    };
    assert!(t0.contains("A"), "位置 0 = read a.txt");
    let ToolExecution::Completed { model_text: t2, .. } = &results[2] else {
        unreachable!()
    };
    assert!(t2.contains("B"), "位置 2 = read b.txt");

    // ② 跨执行器：组 A 改 base→A（两个 edit 合并提交），组 B 改 A→B、
    // "!"→"?"——顺序执行断言终态链式正确（并发互斥由调度层测试钉住）
    let ctx = temp_ctx();
    std::fs::write(ctx.project_root.join("s.txt"), "base\n").unwrap();
    let token = CancellationToken::new();
    let executor_a = ToolsetExecutor::new(ctx.clone(), PermissionTier::FullAccess);
    let batch_a = vec![
        batch_call(
            "c1",
            "edit",
            r#"{"path": "s.txt", "oldText": "base", "newText": "A"}"#,
        ),
        batch_call(
            "c2",
            "edit",
            r#"{"path": "s.txt", "oldText": "A", "newText": "A!"}"#,
        ),
    ];
    let results_a = executor_a.execute_batch(&batch_a, &token).await;
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

// ---- 产物登记接线：write/edit 成功落盘后登记、
// 失败不登记、合并组逐成员计数、未装配 store 不登记。----

use std::sync::Mutex as StdMutex;

/// 带会话存储器环境的执行器（登记测试脚手架）：数据根 = 临时目录上的
/// SessionStore，events 用录制桩（ask/plan 槽空装配）。
fn executor_with_store() -> (
    ToolsetExecutor,
    Arc<crate::storage::SessionStore>,
    std::path::PathBuf,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    let ctx = ToolsContext::new(root.clone(), root.clone());
    let store = Arc::new(crate::storage::SessionStore::with_root(root.clone()));
    let executor =
        ToolsetExecutor::new(ctx, PermissionTier::FullAccess).with_env(super::exec::ExecutorEnv {
            events: Arc::new(crate::util::events::RecordingEventSink::new()),
            ask: Arc::new(StdMutex::new(None)),
            plan: Arc::new(StdMutex::new(None)),
            session_id: "s-art".to_owned(),
            run_id: "run-1".to_owned(),
            data_root: root.clone(),
            ask_log: None,
            mcp: None,
            skills: None,
            subagents: None,
            allowed_tools: None,
            network: None,
            store: Some(store.clone()),
        });
    (executor, store, root)
}

/// 断言产物登记接线：write 成功登记（runCount=1）、
/// 失败（已存在仅新建语义）不登记、edit 同路径重复编辑 runCount 递增、
/// 同批合并组逐成员各计一次、未装配 store（子代理/headless）成功但不登记
/// 不报错。
#[tokio::test]
async fn artifact_recording_cases() {
    // ① write 成功登记 + 失败不登记
    let (executor, store, root) = executor_with_store();
    let token = CancellationToken::new();
    executor
        .execute(
            "c1",
            "write",
            r#"{"path": "a.txt", "content": "v1"}"#,
            &token,
        )
        .await;
    executor
        .execute(
            "c2",
            "write",
            r#"{"path": "b.txt", "content": "v1"}"#,
            &token,
        )
        .await;
    // 已存在 → write 拒绝（仅新建语义）→ 不登记
    executor
        .execute(
            "c3",
            "write",
            r#"{"path": "a.txt", "content": "v2"}"#,
            &token,
        )
        .await;
    let rows = store.list_artifacts("s-art");
    assert_eq!(rows.len(), 2, "成功两条各一条记录，失败的 a.txt 覆盖不追加");
    let a = rows.iter().find(|r| r.path.ends_with("a.txt")).unwrap();
    assert_eq!(a.run_count, 1);
    let _ = root;

    // ② edit 同路径重复编辑：runCount 递增
    let (executor, store, _root) = executor_with_store();
    executor
        .execute(
            "c1",
            "write",
            r#"{"path": "s.txt", "content": "x\n"}"#,
            &token,
        )
        .await;
    executor
        .execute(
            "c2",
            "edit",
            r#"{"path": "s.txt", "oldText": "x", "newText": "y"}"#,
            &token,
        )
        .await;
    executor
        .execute(
            "c3",
            "edit",
            r#"{"path": "s.txt", "oldText": "y", "newText": "z"}"#,
            &token,
        )
        .await;
    let rows = store.list_artifacts("s-art");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].run_count, 3, "write 1 次 + edit 2 次");

    // ③ 同批同文件多 edit 合并提交：每个接受成员都登记一次
    let (executor, store, _root) = executor_with_store();
    executor
        .execute(
            "c0",
            "write",
            r#"{"path": "m.txt", "content": "a b\n"}"#,
            &token,
        )
        .await;
    let batch = vec![
        batch_call(
            "c1",
            "edit",
            r#"{"path": "m.txt", "oldText": "a", "newText": "A"}"#,
        ),
        batch_call(
            "c2",
            "edit",
            r#"{"path": "m.txt", "oldText": "b", "newText": "B"}"#,
        ),
    ];
    let results = executor.execute_batch(&batch, &token).await;
    assert!(
        results
            .iter()
            .all(|e| matches!(e, ToolExecution::Completed { ok: true, .. }))
    );
    let rows = store.list_artifacts("s-art");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].run_count, 3, "write 1 次 + 合并组两个成员各 1 次");

    // ④ 未装配 store：write 成功但不登记、不报错（无登记文件）
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    std::mem::forget(dir);
    let ctx = ToolsContext::new(root.clone(), root.clone());
    let executor =
        ToolsetExecutor::new(ctx, PermissionTier::FullAccess).with_env(super::exec::ExecutorEnv {
            events: Arc::new(crate::util::events::RecordingEventSink::new()),
            ask: Arc::new(StdMutex::new(None)),
            plan: Arc::new(StdMutex::new(None)),
            session_id: "s-x".to_owned(),
            run_id: "run-1".to_owned(),
            data_root: root.clone(),
            ask_log: None,
            mcp: None,
            skills: None,
            subagents: None,
            allowed_tools: None,
            network: None,
            store: None,
        });
    let out = executor
        .execute(
            "c1",
            "write",
            r#"{"path": "a.txt", "content": "v"}"#,
            &token,
        )
        .await;
    assert!(matches!(out, ToolExecution::Completed { ok: true, .. }));
    assert!(!root.join("sessions").join("s-x.artifacts.json").exists());
}
