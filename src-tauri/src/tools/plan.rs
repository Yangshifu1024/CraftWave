//! plan 工具（§8.3 Meta 行 + §8.6 计划门）：计划条目的 set/update 纯逻辑。
//!
//! 状态承载（PlanState）由 RunHandle 持有、executor 读写（工具是模型更新
//! 计划的唯一入口）；批准快照（approvedSnapshot）由 `approve_plan` 命令
//! 冻结（§8.6），守门判定在 ApprovalGate。

use serde::Deserialize;

use crate::safety::tier::Capability;
use crate::storage::entries::{PlanItem, PlanItemStatus};
use crate::util::errors::ToolError;

use super::{ToolAnnotations, ToolOutcome, ToolSpec};

/// 单项上限（§8.3 plan 行：≤100 项）。
pub const MAX_ITEMS: usize = 100;

/// 计划状态（§8.6 PlanState）。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanState {
    /// 当前计划条目（模型通过 plan 工具维护）。
    pub items: Vec<PlanItem>,
    /// 用户批准时冻结的快照（None = 未批准，不阻写）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_snapshot: Option<Vec<PlanItem>>,
    /// 用户批准的时间戳（毫秒）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<i64>,
}

impl PlanState {
    /// 冻结批准快照（approve_plan 命令）。
    pub fn approve(&mut self, now_ms: i64) {
        self.approved_snapshot = Some(self.items.clone());
        self.approved_at = Some(now_ms);
    }

    /// 计划门守门条件（§8.6）：已批准且当前条目全部 completed。
    /// （调用方再按工具能力决定是否拦写。）
    pub fn approved_and_all_completed(&self) -> bool {
        self.approved_snapshot.is_some()
            && !self.items.is_empty()
            && self
                .items
                .iter()
                .all(|i| i.status == PlanItemStatus::Completed)
    }
}

/// plan 工具参数（§8.3）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanArgs {
    /// set = 全量替换（必须给 items）；update = 按 id 合并（追加或替换）。
    pub action: PlanAction,
    #[serde(default)]
    pub items: Vec<PlanItem>,
}

/// plan 工具的动作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanAction {
    /// 全量替换计划条目。
    Set,
    /// 按 id 合并更新（存在则替换、不存在则追加）。
    Update,
}

/// 校验 + 应用到 PlanState，返回更新后的条目（工具 data）。
/// 纯逻辑：状态写入与 plan:update 事件由 executor 承担。
pub fn apply(state: &mut PlanState, args: &PlanArgs) -> Result<Vec<PlanItem>, ToolError> {
    if args.items.len() > MAX_ITEMS {
        return Err(ToolError::invalid_arg(format!(
            "计划条目超上限：{}（≤{MAX_ITEMS}）",
            args.items.len()
        )));
    }
    if let Some(dup) = first_duplicate_id(&args.items) {
        return Err(ToolError::invalid_arg(format!("计划条目 id 重复：{dup}")));
    }
    match args.action {
        PlanAction::Set => {
            if args.items.is_empty() {
                return Err(ToolError::invalid_arg("set 必须给出非空 items"));
            }
            validate_single_in_progress(&args.items)?;
            state.items = args.items.clone();
        }
        PlanAction::Update => {
            if args.items.is_empty() {
                return Err(ToolError::invalid_arg("update 必须给出非空 items"));
            }
            for item in &args.items {
                match state.items.iter_mut().find(|i| i.id == item.id) {
                    Some(existing) => *existing = item.clone(),
                    None => state.items.push(item.clone()),
                }
            }
            validate_single_in_progress(&state.items)?;
        }
    }
    Ok(state.items.clone())
}

/// 单 in_progress 校验（§8.3 plan 行）。
fn validate_single_in_progress(items: &[PlanItem]) -> Result<(), ToolError> {
    let in_progress = items
        .iter()
        .filter(|i| i.status == PlanItemStatus::InProgress)
        .count();
    if in_progress > 1 {
        return Err(ToolError::invalid_arg(format!(
            "计划同时有 {in_progress} 个 in_progress 条目（最多 1 个）"
        )));
    }
    Ok(())
}

/// 找出第一处重复的条目 id（无重复返回 None）。
fn first_duplicate_id(items: &[PlanItem]) -> Option<&str> {
    let mut seen = std::collections::HashSet::new();
    items.iter().find_map(|i| {
        if !seen.insert(i.id.as_str()) {
            Some(i.id.as_str())
        } else {
            None
        }
    })
}

/// plan 工具规格（能力 Meta，全档放行）。
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "plan",
        description: "维护任务计划清单。set 全量替换条目；update 按 id 更新或追加。\
最多 100 项，同一时刻最多 1 个 in_progress。开始执行前先建计划，完成一项更新一项。"
            .to_string(),
        capability: Capability::Meta,
        params_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["set", "update"] },
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" },
                            "text": { "type": "string" },
                            "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] }
                        },
                        "required": ["id", "text", "status"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["action"],
            "additionalProperties": false
        }),
        annotations: ToolAnnotations {
            read_only_hint: false,
            destructive_hint: false,
            idempotent_hint: false,
            open_world_hint: false,
        },
    }
}

/// plan 工具的 data 形态（条目列表；executor 组装 ToolOutcome 用）。
pub fn outcome_data(items: &[PlanItem]) -> serde_json::Value {
    serde_json::json!({ "items": items })
}

/// 空计划结果兜底（状态尚未初始化时 set 前查询无意义，正常不出现）。
pub fn empty_outcome() -> ToolOutcome {
    ToolOutcome::success(serde_json::json!({ "items": [] }), "计划为空")
}
