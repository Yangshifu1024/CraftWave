//! plan 工具纯逻辑单测（工具约束 + 计划门）。

use crate::storage::entries::{PlanItem, PlanItemStatus};

use super::plan::{MAX_ITEMS, MAX_PATHS, PlanAction, PlanArgs, PlanState, apply};

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

/// 测试辅助：带 paths 的 set 参数。
fn set_args_with_paths(items: Vec<PlanItem>, paths: &[&str]) -> PlanArgs {
    PlanArgs {
        action: PlanAction::Set,
        items,
        paths: paths.iter().map(|p| p.to_string()).collect(),
    }
}

// 断言：plan 状态机矩阵——set 整表替换、空条目/重复 id/超 100 上限/多个
// in_progress 拒绝、update 按 id 合并追加；paths 声明 set 全量覆盖、set 不带
// 清空、update 不带保持、带则覆盖、超 64 条与空白条目拒绝；计划门仅「批准
// 且全部 completed」触发、批准冻结 paths 基线（模型再改不动基线）、空声明
// 冻结为空基线合法；错误载体为 ToolError。
#[test]
fn plan_state_gate_and_paths_declaration_matrix() {
    // —— set / update 语义 ——
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

    // 空条目拒绝（错误载体 ToolError，码 E_INVALID_ARG）。
    let err: crate::util::errors::ToolError = apply(&mut state, &set_args(vec![])).unwrap_err();
    assert_eq!(err.code, "E_INVALID_ARG");
    // 重复 id 拒绝。
    let err = apply(
        &mut state,
        &set_args(vec![
            item("x", PlanItemStatus::Pending),
            item("x", PlanItemStatus::Completed),
        ]),
    )
    .unwrap_err();
    assert!(err.message.contains("重复"), "{}", err.message);

    // 超 100 上限拒绝。
    let items: Vec<PlanItem> = (0..MAX_ITEMS + 1)
        .map(|i| item(&format!("i{i}"), PlanItemStatus::Pending))
        .collect();
    let err = apply(&mut state, &set_args(items)).unwrap_err();
    assert!(err.message.contains("100"));

    // 多个 in_progress 拒绝（最多 1 个）。
    let err = apply(
        &mut state,
        &set_args(vec![
            item("a", PlanItemStatus::InProgress),
            item("b", PlanItemStatus::InProgress),
        ]),
    )
    .unwrap_err();
    assert!(err.message.contains("in_progress"), "{}", err.message);

    // update 按 id 替换已有条目、追加新条目。
    apply(
        &mut state,
        &set_args(vec![
            item("a", PlanItemStatus::Pending),
            item("b", PlanItemStatus::Pending),
        ]),
    )
    .unwrap();
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

    // —— paths 声明语义 ——
    // set 带 paths 全量覆盖。
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
    // set 不带 paths = 清空声明。
    apply(
        &mut state,
        &set_args(vec![item("a", PlanItemStatus::Pending)]),
    )
    .unwrap();
    assert!(state.paths.is_empty(), "set 不带 paths 应清空声明");
    // 重新声明后 update 不带 paths 保持、带 paths 覆盖。
    apply(
        &mut state,
        &set_args_with_paths(vec![item("a", PlanItemStatus::Pending)], &["src/a.rs"]),
    )
    .unwrap();
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
    // paths 校验：超 64 条拒绝；空白条目拒绝。
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

    // —— 计划门与批准基线 ——
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

    // 批准冻结写范围基线：批准后模型改 paths，基线不动。
    let mut state = PlanState::default();
    apply(
        &mut state,
        &set_args_with_paths(vec![item("a", PlanItemStatus::InProgress)], &["src/a.rs"]),
    )
    .unwrap();
    state.approve(1000);
    assert_eq!(state.baseline_paths, vec!["src/a.rs"], "批准冻结基线");
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
    // 批准空声明范围：baseline_paths 为空且合法（豁免判定在审批门范围门）。
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
