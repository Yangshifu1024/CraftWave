//! plan 工具纯逻辑单测（§8.3 plan 行 + §8.6 计划门）。

use crate::storage::entries::{PlanItem, PlanItemStatus};
use crate::util::errors::ToolError;

use super::plan::{MAX_ITEMS, PlanAction, PlanArgs, PlanState, apply};

/// 测试辅助：构造一个计划条目（文本由 id 派生）。
fn item(id: &str, status: PlanItemStatus) -> PlanItem {
    PlanItem {
        id: id.to_string(),
        text: format!("任务 {id}"),
        status,
    }
}

/// 测试辅助：构造 set 动作的参数。
fn set_args(items: Vec<PlanItem>) -> PlanArgs {
    PlanArgs {
        action: PlanAction::Set,
        items,
        paths: Vec::new(),
    }
}

/// 测试辅助：用 (id, text, status) 三元组构造条目列表。
fn json_items(raw: &[(&str, &str, &str)]) -> Vec<PlanItem> {
    raw.iter()
        .map(|(id, text, status)| PlanItem {
            id: id.to_string(),
            text: text.to_string(),
            status: serde_json::from_value::<PlanItemStatus>(serde_json::json!(status))
                .expect("合法状态"),
        })
        .collect()
}

// 断言：set 动作整表替换而非追加，返回的条目即最新状态。
#[test]
fn set_replaces_all() {
    let mut state = PlanState::default();
    let items = apply(
        &mut state,
        &set_args(vec![item("1", PlanItemStatus::Pending)]),
    )
    .unwrap();
    assert_eq!(items.len(), 1);
    let items = apply(
        &mut state,
        &set_args(json_items(&[
            ("a", "读文件", "pending"),
            ("b", "写文件", "in_progress"),
        ])),
    )
    .unwrap();
    assert_eq!(items.len(), 2, "set 替换而非追加");
    assert_eq!(state.items[1].text, "写文件");
}

// 断言：set 空条目与重复 id 均以 E_INVALID_ARG 拒绝。
#[test]
fn set_empty_items_duplicate_id_rejected() {
    let mut state = PlanState::default();
    let err = apply(&mut state, &set_args(vec![])).unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
    let err = apply(
        &mut state,
        &set_args(vec![
            item("x", PlanItemStatus::Pending),
            item("x", PlanItemStatus::Completed),
        ]),
    )
    .unwrap_err();
    assert!(err.message.contains("重复"), "{}", err.message);
}

// 断言：条目数超过 100 上限被拒绝。
#[test]
fn limit_100_items() {
    let mut state = PlanState::default();
    let items: Vec<PlanItem> = (0..MAX_ITEMS + 1)
        .map(|i| item(&format!("i{i}"), PlanItemStatus::Pending))
        .collect();
    let err = apply(&mut state, &set_args(items)).unwrap_err();
    assert!(err.message.contains("100"));
}

// 断言：同时多个 in_progress 条目被拒（最多 1 个）；0 个合法。
#[test]
fn single_in_progress_multiple_rejected() {
    let mut state = PlanState::default();
    let err = apply(
        &mut state,
        &set_args(vec![
            item("a", PlanItemStatus::InProgress),
            item("b", PlanItemStatus::InProgress),
        ]),
    )
    .unwrap_err();
    assert!(err.message.contains("in_progress"), "{}", err.message);
    // 0 个 in_progress 合法
    apply(
        &mut state,
        &set_args(vec![item("a", PlanItemStatus::Pending)]),
    )
    .unwrap();
}

// 断言：update 按 id 替换已有条目、追加新条目。
#[test]
fn update_merges_by_id_append_and_replace() {
    let mut state = PlanState::default();
    apply(
        &mut state,
        &set_args(vec![
            item("a", PlanItemStatus::Pending),
            item("b", PlanItemStatus::Pending),
        ]),
    )
    .unwrap();
    // a 推进、c 追加
    let items = apply(
        &mut state,
        &PlanArgs {
            action: PlanAction::Update,
            items: vec![
                item("a", PlanItemStatus::Completed),
                item("c", PlanItemStatus::Pending),
            ],
            paths: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(state.items[0].status, PlanItemStatus::Completed);
    assert_eq!(state.items[2].id, "c");
}

// 断言：计划门守门条件——仅"已批准且条目全部 completed"时为真
//（未批准/批准但未全完成均不触发），批准会冻结快照。
#[test]
fn plan_gate_blocks_only_when_approved_all_completed() {
    let mut state = PlanState::default();
    apply(
        &mut state,
        &set_args(vec![item("a", PlanItemStatus::Pending)]),
    )
    .unwrap();
    assert!(!state.approved_and_all_completed(), "未批准不拦");
    state.approve(1000);
    assert!(!state.approved_and_all_completed(), "批准但未全完成不拦");
    apply(
        &mut state,
        &PlanArgs {
            action: PlanAction::Update,
            items: vec![item("a", PlanItemStatus::Completed)],
            paths: Vec::new(),
        },
    )
    .unwrap();
    assert!(state.approved_and_all_completed(), "批准且全完成 → 守门");
    assert_eq!(state.approved_at, Some(1000));
    assert_eq!(state.approved_snapshot.as_ref().unwrap().len(), 1);
}

// 断言：apply 的错误载体是 ToolError（错误码 E_INVALID_ARG）。
#[test]
fn error_carrier_is_tool_error() {
    let mut state = PlanState::default();
    let err: ToolError = apply(&mut state, &set_args(vec![])).unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
}

// ---- 写范围声明与批准基线（25 号票，§8.3 plan 行 paths + §8.6）----

/// 测试辅助：带 paths 的 set 参数。
fn set_args_with_paths(items: Vec<PlanItem>, paths: &[&str]) -> PlanArgs {
    PlanArgs {
        action: PlanAction::Set,
        items,
        paths: paths.iter().map(|p| p.to_string()).collect(),
    }
}

/// set 带 paths 全量覆盖声明；set 不带 paths 清空声明（全量替换语义）。
#[test]
fn paths_declaration_set_overrides_and_absent_clears() {
    let mut state = PlanState::default();
    apply(
        &mut state,
        &set_args_with_paths(
            vec![item("a", PlanItemStatus::Pending)],
            &["src/a.rs", "tests/"],
        ),
    )
    .unwrap();
    assert_eq!(state.paths, vec!["src/a.rs", "tests/"]);
    // set 未带 paths = 清空声明（模型重新声明范围的合法路径）
    apply(
        &mut state,
        &set_args(vec![item("a", PlanItemStatus::Pending)]),
    )
    .unwrap();
    assert!(state.paths.is_empty(), "set 不带 paths 应清空声明");
}

/// update 不带 paths 保持既有声明（条目微调不清范围）；带 paths 覆盖。
#[test]
fn paths_declaration_update_keeps_when_absent_overrides_when_present() {
    let mut state = PlanState::default();
    apply(
        &mut state,
        &set_args_with_paths(vec![item("a", PlanItemStatus::Pending)], &["src/a.rs"]),
    )
    .unwrap();
    // update 条目不带 paths：声明保持
    apply(
        &mut state,
        &PlanArgs {
            action: PlanAction::Update,
            items: vec![item("a", PlanItemStatus::InProgress)],
            paths: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(state.paths, vec!["src/a.rs"], "update 不带 paths 不清声明");
    // update 带 paths：覆盖
    apply(
        &mut state,
        &PlanArgs {
            action: PlanAction::Update,
            items: vec![item("a", PlanItemStatus::Completed)],
            paths: vec!["src/a.rs".to_owned(), "docs/guide.md".to_owned()],
        },
    )
    .unwrap();
    assert_eq!(state.paths, vec!["src/a.rs", "docs/guide.md"]);
}

/// 批准（approve_plan）冻结写范围基线：baseline_paths = 批准时刻 paths 的
/// 快照副本；批准后模型再改 paths 不影响已冻结基线（基线只随用户重新
/// 批准更新，§8.6）。
#[test]
fn approve_freezes_baseline_paths() {
    let mut state = PlanState::default();
    apply(
        &mut state,
        &set_args_with_paths(vec![item("a", PlanItemStatus::InProgress)], &["src/a.rs"]),
    )
    .unwrap();
    state.approve(1000);
    assert_eq!(state.baseline_paths, vec!["src/a.rs"], "批准冻结基线");
    // 批准后模型改 paths：基线不动
    apply(
        &mut state,
        &PlanArgs {
            action: PlanAction::Update,
            items: vec![item("a", PlanItemStatus::Completed)],
            paths: vec!["src/escape.rs".to_owned()],
        },
    )
    .unwrap();
    assert_eq!(state.paths, vec!["src/escape.rs"]);
    assert_eq!(state.baseline_paths, vec!["src/a.rs"], "基线不随模型改");
}

/// 批准空声明范围：baseline_paths 为空且合法（§8.6：空范围不豁免首写，
/// 豁免判定在审批门范围门）。
#[test]
fn approve_with_empty_paths_baseline_is_legal() {
    let mut state = PlanState::default();
    apply(
        &mut state,
        &set_args(vec![item("a", PlanItemStatus::Pending)]),
    )
    .unwrap();
    state.approve(1000);
    assert!(state.is_approved());
    assert!(state.baseline_paths.is_empty(), "空声明冻结为空基线");
}

/// paths 校验：超 64 条拒绝；空白条目拒绝（E_INVALID_ARG）。
#[test]
fn paths_validation_rejects_over_limit_and_blank() {
    use super::plan::MAX_PATHS;
    let mut state = PlanState::default();
    let many: Vec<String> = (0..=MAX_PATHS).map(|i| format!("f{i}.rs")).collect();
    let err = apply(
        &mut state,
        &PlanArgs {
            action: PlanAction::Set,
            items: vec![item("a", PlanItemStatus::Pending)],
            paths: many,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG", "超上限拒绝");
    let err = apply(
        &mut state,
        &set_args_with_paths(vec![item("a", PlanItemStatus::Pending)], &["  "]),
    )
    .unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG", "空白路径拒绝");
}
