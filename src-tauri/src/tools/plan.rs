//! plan 工具：计划条目的 set/update 纯逻辑（Meta 能力 + 计划门）。
//!
//! 状态承载（PlanState）由 RunHandle 持有、executor 读写（工具是模型更新
//! 计划的唯一入口）；批准快照（approvedSnapshot）与写范围基线（baselinePaths）
//! 由 `approve_plan` 命令冻结，守门判定在 ApprovalGate。

use serde::Deserialize;

use crate::safety::tier::Capability;
use crate::storage::entries::{PlanItem, PlanItemStatus};
use crate::util::errors::ToolError;

use super::{ToolAnnotations, ToolOutcome, ToolSpec};

/// 单项上限（≤100 项）。
pub const MAX_ITEMS: usize = 100;

/// 写范围声明上限（paths ≤64 条）。
pub const MAX_PATHS: usize = 64;

/// 计划状态（含批准快照与写范围基线）。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanState {
    /// 当前计划条目（模型通过 plan 工具维护）。
    pub items: Vec<PlanItem>,
    /// 模型声明的计划写范围（plan 工具 `paths` 参数）：具体文件/目录
    /// 路径字符串，随 `plan:update` 事件展示给用户——批准时冻结为基线。
    /// 未声明 = 空（批准后首写也须范围门确认）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// 用户批准时冻结的快照（None = 未批准，不阻写）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_snapshot: Option<Vec<PlanItem>>,
    /// 用户批准的时间戳（毫秒）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<i64>,
    /// 用户批准时冻结的文件/目录范围（基线）：批准时刻
    /// `paths` 的快照副本；判定口径（目录含子路径、文件精确匹配、不自动
    /// 扩顶层目录）在 ApprovalGate（canonical 归一需要 workspace 基准）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub baseline_paths: Vec<String>,
}

impl PlanState {
    /// 冻结批准快照（approve_plan 命令）：条目快照 + 写范围基线同时
    /// 冻结（用户看到并批准的范围）。基线为空不豁免首写（范围门判定面）。
    pub fn approve(&mut self, now_ms: i64) {
        self.approved_snapshot = Some(self.items.clone());
        self.approved_at = Some(now_ms);
        self.baseline_paths = self.paths.clone();
    }

    /// 计划门守门条件：已批准且当前条目全部 completed。
    /// （调用方再按工具能力决定是否拦写。）
    pub fn approved_and_all_completed(&self) -> bool {
        self.approved_snapshot.is_some()
            && !self.items.is_empty()
            && self
                .items
                .iter()
                .all(|i| i.status == PlanItemStatus::Completed)
    }

    /// 是否已批准（范围门与硬门的共同前提：无批准计划不守门——
    /// 缺省"有计划即守门"）。
    pub fn is_approved(&self) -> bool {
        self.approved_snapshot.is_some()
    }
}

/// plan 工具参数（含 `paths` 写范围声明）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanArgs {
    /// set = 全量替换（必须给 items）；update = 按 id 合并（追加或替换）。
    pub action: PlanAction,
    #[serde(default)]
    pub items: Vec<PlanItem>,
    /// 计划写范围声明（可选）：具体文件/目录路径。set = 全量覆盖（未带 =
    /// 清空声明）；update = 非空才覆盖（条目微调不带 paths 不清范围）。
    #[serde(default)]
    pub paths: Vec<String>,
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
    // paths 声明校验（≤64 条、单条非空）。
    if args.paths.len() > MAX_PATHS {
        return Err(ToolError::invalid_arg(format!(
            "计划写范围声明超上限：{}（≤{MAX_PATHS}）",
            args.paths.len()
        )));
    }
    if let Some(blank) = args.paths.iter().find(|p| p.trim().is_empty()) {
        return Err(ToolError::invalid_arg(format!(
            "计划写范围声明含空白路径：{blank:?}"
        )));
    }
    match args.action {
        PlanAction::Set => {
            if args.items.is_empty() {
                return Err(ToolError::invalid_arg("set 必须给出非空 items"));
            }
            validate_single_in_progress(&args.items)?;
            state.items = args.items.clone();
            // set = 全量替换语义：paths 未带 = 清空声明（模型重新声明范围）。
            state.paths = args.paths.clone();
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
            // update = 条目微调语义：paths 非空才覆盖（不带 paths 不清范围）。
            if !args.paths.is_empty() {
                state.paths = args.paths.clone();
            }
        }
    }
    Ok(state.items.clone())
}

/// 单 in_progress 校验。
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
        name: "plan".to_string(),
        description: "维护任务计划清单。set 全量替换条目；update 按 id 更新或追加。\
最多 100 项，同一时刻最多 1 个 in_progress。开始执行前先建计划，完成一项更新一项。\
paths 声明本计划要写入的具体文件/目录范围（批准后用户按此范围授权，范围外写入会请求确认）。"
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
                },
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "本计划涉及的具体文件/目录路径（写范围声明，≤64 条）"
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
        origin: crate::tools::ToolOrigin::Static,
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
