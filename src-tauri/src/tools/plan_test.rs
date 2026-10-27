//! plan 工具纯逻辑单测（§8.3 plan 行 + §8.6 计划门）。

use crate::storage::entries::{PlanItem, PlanItemStatus};
use crate::util::errors::ToolError;

use super::plan::{MAX_ITEMS, PlanAction, PlanArgs, PlanState, apply};

fn item(id: &str, status: PlanItemStatus) -> PlanItem {
    PlanItem {
        id: id.to_string(),
        text: format!("任务 {id}"),
        status,
    }
}

fn set_args(items: Vec<PlanItem>) -> PlanArgs {
    PlanArgs {
        action: PlanAction::Set,
        items,
    }
}

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

#[test]
fn limit_100_items() {
    let mut state = PlanState::default();
    let items: Vec<PlanItem> = (0..MAX_ITEMS + 1)
        .map(|i| item(&format!("i{i}"), PlanItemStatus::Pending))
        .collect();
    let err = apply(&mut state, &set_args(items)).unwrap_err();
    assert!(err.message.contains("100"));
}

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

#[test]
fn error_carrier_is_tool_error() {
    let mut state = PlanState::default();
    let err: ToolError = apply(&mut state, &set_args(vec![])).unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
}
