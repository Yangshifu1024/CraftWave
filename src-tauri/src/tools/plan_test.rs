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
