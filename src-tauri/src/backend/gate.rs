//! 审批门（技术设计文档 §5.4 步骤①–④ × §8.5 权限矩阵 × §8.7 围栏 × §8.6
//! 计划门）：主循环 `ToolGate` 接口的生产策略实现。
//!
//! 名词解释：审批门 = 工具执行前的安全判定，结论有三种——放行（直接执行）、
//! 挂起（暂停整个 run 等用户批准）、硬拦（直接拒绝并把错误回喂给模型）。
//! 权限档 = 控制模型可用工具范围的档位（plan/confirm_each/auto_edit/
//! full_access，从只读到全开放）；围栏 = 命令工具的硬性安全底线（黑名单
//! 命令任何档位都拦）；计划门 = "已批准的计划全部完成后不允许继续写文件"
//! 的约束。
//!
//! 判定顺序（首个命中即出结论；23 号票起禁区分两层前置）：
//! 0. **内部状态硬拒（23 号票，FR-05-08/§9.3）**：FileWrite 工具路径与
//!    command 写目标命中禁区（config/凭据/MCP 配置/索引/锁面等宿主内部
//!    状态）→ E_INTERNAL_STATE 硬拦——置于全部放行路径之前，任何权限档、
//!    白名单、自动确认、审批总开关与已批准签名都不豁免（禁区调用不可能
//!    曾获放行，签名放行前再查一道是防御纵深）；
//! 1. 已批准签名：本 run 内同"工具名 + 参数 JSON"的调用此前已批准，或该
//!    签名在本会话「始终允许」桶内（23 号票 always_session 升域，跨 run
//!    生效）→ 放行（挂起恢复后主循环会原样重放挂起批，按签名放行避免
//!    再次挂起死循环）；
//! 2. 围栏（command 专属）：Block → E_FENCE_BLOCKED 硬拦（任何档位不豁
//!    免，白名单也不豁免黑名单）；Confirm → 挂起（围栏原因随载荷展示给
//!    用户）——例外两条：命令命中 commandAllowlist 且非 plan 档时免除
//!    （23 号票白名单消费）；审批总开关关闭且非 plan 档时不弹审批直接
//!    放行（① 开关，plan 档的只读围栏是档位语义不属审批弹窗，保留）；
//! 3. 计划门（写文件/联网类）：已批准计划存在且条目全部完成仍要写 →
//!    E_PLAN_STALE 硬拦回喂（提示模型先更新计划）；
//! 4. 权限矩阵（§8.5 的单一实现 `decide_gate`）：Filtered → E_TOOL_BLOCKED
//!    硬拦；NeedsApproval → 挂起（白名单命中或审批总开关关闭且非 plan 档
//!    时放行，豁免口径同上）；
//! 5. **MCP 动态工具分支**（08 号票，§8.5 注 + §9.7）：MCP 工具未声明
//!    read_only（用户配置）且无 Q44 许可（三元组 = 服务器启动指纹 + 原始
//!    工具名 + 已确认输入 schema 全等；来源为审批「始终允许」或服务器级
//!    always_allow 的同步自动授权）时——**即便 full_access 也挂起确认**
//!    （FR-08-04 高危外部调用永远过人，审批总开关不豁免本分支）。read_only
//!    声明的 MCP 工具按 ReadOnly 能力走矩阵放行；plan 档已由
//!    filter_for_tier 全过滤。
//!
//! 挂起之后的载荷构造与 `resolve_ask` 恢复循环在宿主命令层（commands/chat.rs）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::safety::fence::ast::{self, FsCanonicalizer, TargetPath};
use crate::safety::fence::{Fence, FenceVerdict};
use crate::safety::internal_state::InternalStateGuard;
use crate::safety::safe_join::canonicalize_best_effort;
use crate::safety::tier::{Capability, GateDecision, PermissionTier, decide_gate};
use crate::tools::plan::PlanState;
use crate::tools::{ToolOrigin, ToolSpec, lookup_spec};
use crate::util::errors::{
    E_FENCE_BLOCKED, E_INTERNAL_STATE, E_INVALID_ARG, E_PLAN_STALE, E_TOOL_BLOCKED,
};

use super::run_loop::{GateVerdict, ToolGate};

/// 审批策略快照（23 号票，§3.2 approval 四开关中由门消费的两项）：run
/// 装配时从 AppConfig.approval 读取（宿主侧构造），判定面只见快照——
/// save_config 后下一次 start_chat 应用，进行中 run 不变。
///
/// Default（未显式 `with_policy` 的构造面，含既有测试）：审批开、白名单
/// 关——与 AppConfig 的 serde 缺省（③④ 开）语义域不同：这里是「未配置
/// 时的安全缺省」，宁可多审批；白名单关保证无条目快照零行为差异。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPolicy {
    /// ① 审批总开关（false = 不再弹出任何审批：围栏 Confirm 与矩阵
    /// NeedsApproval 不挂起直接放行；黑名单 Block/内部状态硬拒/计划门/
    /// MCP 分支与信任询问不受影响；plan 档判定不变——只读围栏属档位
    /// 语义而非审批弹窗，关闭总开关不解锁计划档写入）。
    pub enabled: bool,
    /// ④ 命令白名单启用（false = commandAllowlist 不参与判定，条目保留
    /// 在配置里不动）。
    pub allowlist_enabled: bool,
    /// 命令白名单条目快照（匹配语义见 [`allowlist_hit`]）。
    pub command_allowlist: Vec<String>,
}

impl Default for ApprovalPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            allowlist_enabled: false,
            command_allowlist: Vec::new(),
        }
    }
}

/// 单会话免批签名集合（(工具名, 参数 JSON) 签名；类型别名仅为收敛签名
/// 复杂度，语义见 [`ApprovalMemory`] 与 [`SessionApprovalBucket`]）。
type SignatureSet = Arc<Mutex<HashSet<(String, String)>>>;

/// 会话级审批记忆（23 号票，§3.2 always_session 升域）：全部活跃会话的
/// 「始终允许（本会话）」签名桶表。进程内单例（AppState 持有，Arc 共享），
/// **不持久化**——应用退出即清、恢复旧备份不复活（Q41：审批记忆须重新
/// 确认）；会话删除随清（`forget`）。同一会话的主 run 与子代理共用会话桶。
#[derive(Debug, Default, Clone)]
pub struct ApprovalMemory {
    sessions: Arc<Mutex<HashMap<String, SignatureSet>>>,
}

impl ApprovalMemory {
    /// 空表构造。
    pub fn new() -> Self {
        Self::default()
    }

    /// 取某会话的免批签名桶（无则建空桶）。桶句柄与表内条目共享同一
    /// 集合（Arc 同源）：`remember` 事后登记对已取出的桶句柄即时可见。
    pub fn bucket(&self, session_id: &str) -> SessionApprovalBucket {
        let entry = self
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(session_id.to_owned())
            .or_default()
            .clone();
        SessionApprovalBucket { entries: entry }
    }

    /// 登记一条免批签名（审批恢复循环在应答 always_session 时调用；
    /// (name, argsJson) 口径同 run 内 approved 集）。
    pub fn remember(&self, session_id: &str, name: &str, args_json: &str) {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(session_id.to_owned())
            .or_default()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((name.to_owned(), args_json.to_owned()));
    }

    /// 会话删除/关闭即清（Q41：记忆不跨会话生命周期）。
    pub fn forget(&self, session_id: &str) {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id);
    }

    /// 全部清空（33 号票备份恢复流程的接线面：恢复后一律重新确认，
    /// Q41「审批记忆不因恢复复活」——内存记忆随恢复动作显式失效）。
    pub fn clear_all(&self) {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// 某会话桶内签名数（测试与诊断面）。
    pub fn len_of(&self, session_id: &str) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(session_id)
            .map(|s| {
                s.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .len()
            })
            .unwrap_or(0)
    }
}

/// 单会话免批签名桶（会话级记忆的只读判定面；写入口在
/// [`ApprovalMemory::remember`]，桶句柄只做命中查询）。克隆共享同一集合。
#[derive(Debug, Clone, Default)]
pub struct SessionApprovalBucket {
    entries: SignatureSet,
}

impl SessionApprovalBucket {
    /// 签名是否在会话免批桶内。
    pub fn contains(&self, name: &str, args_json: &str) -> bool {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&(name.to_owned(), args_json.to_owned()))
    }
}

/// 命令白名单的词序列前缀匹配（23 号票字段级定义，回写 §3.2 approval
/// 注记）：条目与命令都按空白切词，条目词序列是命令词序列的前缀（逐词
/// 相等）即命中。`git status` 匹配 `git status --short`，不匹配
/// `git statusx`（词边界）；条目为空不命中。不做大小写归一（shell 命令
/// 名大小写敏感，保守方向宁多审不放行）；复合命令（`cd x && git status`）
/// 整串切词后与单命令条目不构成前缀——同样保守不命中。
pub fn allowlist_hit(command: &str, entry: &str) -> bool {
    let cmd: Vec<&str> = command.split_whitespace().collect();
    let entry: Vec<&str> = entry.split_whitespace().collect();
    !entry.is_empty() && cmd.len() >= entry.len() && cmd[..entry.len()] == entry[..]
}

/// MCP 工具身份的判定视图（Q44 三元组 + 门控输入；08 号票）。
/// backend 层自定义的窄结构——审批门不依赖 mcp 模块（依赖方向 §2.2：
/// host 装配时注入实现，依赖倒置与「tools/agent 经接口交互」同精神）。
#[derive(Debug, Clone)]
pub struct McpToolView {
    /// 配置里的服务器名（审批标题展示）。
    pub server_name: String,
    /// 服务器启动指纹（Q44 许可三元组第一元）。
    pub server_fingerprint: String,
    /// 原始工具名（许可三元组第二元）。
    pub original_name: String,
    /// 输入 schema 哈希（许可三元组第三元）。
    pub schema_hash: String,
    /// 用户配置的只读声明（true → 能力按 ReadOnly 进矩阵）。
    pub read_only: bool,
}

/// MCP 判定视图（宿主装配；测试传 [`NoopMcpGate`] 或 None）。
pub trait McpGateView: Send + Sync {
    /// 归一名 → 工具身份视图（非 MCP 归一名返回 None）。
    fn view(&self, normalized_name: &str) -> Option<McpToolView>;
    /// Q44 许可三元组是否命中（serverFingerprint + originalName + schemaHash
    /// 全等 → 免审批）。
    fn license_hit(&self, view: &McpToolView) -> bool;
}

/// 空实现（无 MCP 装配时的默认视图：一切查询 None，走静态路径）。
pub struct NoopMcpGate;

impl McpGateView for NoopMcpGate {
    fn view(&self, _normalized_name: &str) -> Option<McpToolView> {
        None
    }

    fn license_hit(&self, _view: &McpToolView) -> bool {
        false
    }
}

/// 围栏 AST 判定的工作区上下文（22 号票，§8.7 四象限）：canonical 后的
/// roots + 相对路径默认基准（项目主目录）。装配后围栏对写目标做
/// Allow/Confirm/Block 判界；未装配时退化为「有写目标即 Confirm」。
#[derive(Debug, Clone)]
pub struct GateWorkspace {
    /// canonical 后的允许根集合（项目主目录 + workspace 子目录，§9.1）。
    pub roots: Vec<PathBuf>,
    /// canonical 后的项目主目录（command 的 cwd 缺省基准）。
    pub base: PathBuf,
}

/// 生产环境使用的审批门。
pub struct ApprovalGate {
    /// 当前会话的权限档。
    tier: PermissionTier,
    /// 命令围栏判定器（黑名单 / AST 写目标分析，22 号票起 v0.2）。
    fence: Fence,
    /// 本 run 内已批准的（工具名, 参数 JSON）签名集（resolve_ask 放行后登记）。
    approved: Mutex<HashSet<(String, String)>>,
    /// 计划状态（与 RunHandle 持有的是同一单元；plan 工具写、approve_plan 冻结）。
    plan: Arc<Mutex<Option<PlanState>>>,
    /// MCP 判定视图（None = 未装配 MCP，全部走静态路径）。
    mcp: Option<Arc<dyn McpGateView>>,
    /// 围栏 AST 判定的工作区上下文（None = 未装配，退化为有写目标即 Confirm）。
    workspace: Option<GateWorkspace>,
    /// 内部状态禁区判定面（23 号票 FR-05-08/§9.3）：None = 未装配（headless
    /// 测试缺省不拦），装配后对 FileWrite 工具路径与 command 写目标做禁区
    /// 硬拒（E_INTERNAL_STATE）。
    internal: Option<InternalStateGuard>,
    /// 会话级免批签名桶（23 号票 always_session 升域）：None = 未装配。
    session_memory: Option<SessionApprovalBucket>,
    /// 审批策略快照（23 号票四开关中由门消费的 ①④ + 白名单条目；
    /// 缺省 = 审批开、白名单关——最保守，M1 既有行为零漂移）。
    policy: ApprovalPolicy,
    /// 外部目录授权扩展 roots（24 号票，§9.9）：run 装配时 = 会话已授权
    /// 目录的 canonical 集（SessionMeta 快照）；恢复循环在用户授权后经
    /// [`Self::grant_external_dir`] 热追加（重放调用由此放行）。空 = 未
    /// 装配/无授权（外部目录判定仍生效——workspace 装配了就判）。
    external_dirs: Mutex<Vec<PathBuf>>,
    /// 宿主数据目录（24 号票禁授权目标判定的输入：「整体放行」禁区 =
    /// 文件系统根/家目录/`~/.craftwave`；None = 未装配，外部挂起不查禁区，
    /// 禁区校验由 allow_external_dir 命令面兜底）。
    data_dir: Option<PathBuf>,
}

/// ApprovalGate 的登记与辅助判定。
impl ApprovalGate {
    /// 构造：给定权限档与共享计划状态（围栏与签名集内部自建）。MCP 视图
    /// 可后续经 [`Self::with_mcp`] 装配（M1 测试零改动）。
    pub fn new(tier: PermissionTier, plan: Arc<Mutex<Option<PlanState>>>) -> Self {
        Self {
            tier,
            fence: Fence::new(),
            approved: Mutex::new(HashSet::new()),
            plan,
            mcp: None,
            workspace: None,
            internal: None,
            session_memory: None,
            policy: ApprovalPolicy::default(),
            external_dirs: Mutex::new(Vec::new()),
            data_dir: None,
        }
    }

    /// 装配 MCP 判定视图（08 号票：MCP 分支判定与 Q44 许可核验）。
    pub fn with_mcp(mut self, mcp: Arc<dyn McpGateView>) -> Self {
        self.mcp = Some(mcp);
        self
    }

    /// 装配围栏 AST 判定的工作区上下文（22 号票）：roots/base 须传
    /// canonical 后路径（词法与 canonical 同域比较的前提）；装配失败
    /// （canonicalize 失败）不调用本方法即可，围栏自动走保守退化面。
    pub fn with_workspace(mut self, workspace: GateWorkspace) -> Self {
        self.workspace = Some(workspace);
        self
    }

    /// 装配内部状态禁区判定面（23 号票 FR-05-08）：宿主以会话的数据目录
    /// 根 + 项目主目录构造（两根 canonical 与否由 guard 自行尽力解析）。
    pub fn with_internal_guard(mut self, guard: InternalStateGuard) -> Self {
        self.internal = Some(guard);
        self
    }

    /// 装配会话级免批签名桶（23 号票 always_session）：同会话主 run 与
    /// 子代理传同一桶（[`ApprovalMemory::bucket`]）。
    pub fn with_session_memory(mut self, bucket: SessionApprovalBucket) -> Self {
        self.session_memory = Some(bucket);
        self
    }

    /// 装配审批策略快照（23 号票四开关 ①④ + 白名单条目，宿主按 run 从
    /// AppConfig.approval 读取）。
    pub fn with_policy(mut self, policy: ApprovalPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// 装配外部目录授权的初始集合与宿主数据目录（24 号票，§9.9）：dirs
    /// 传会话已授权目录的 canonical 集（run 装配时从 SessionMeta 读取）；
    /// data_dir 传数据目录根（禁授权目标判定输入）。
    pub fn with_external_dirs(mut self, dirs: Vec<PathBuf>, data_dir: PathBuf) -> Self {
        self.external_dirs = Mutex::new(dirs);
        self.data_dir = Some(data_dir);
        self
    }

    /// 热追加一条外部目录授权（恢复循环在用户点「始终允许」后调用）——
    /// 挂起恢复的重放调用经本集合放行。
    pub fn grant_external_dir(&self, dir: PathBuf) {
        let mut dirs = self
            .external_dirs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }

    /// 审批通过后登记签名（挂起恢复重放同一调用时按签名放行）。
    pub fn approve(&self, name: &str, args_json: &str) {
        self.approved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((name.to_owned(), args_json.to_owned()));
    }

    /// 从参数 JSON 中提取 command 字段（围栏判定的输入）。
    fn command_of(args_json: &str) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(args_json)
            .ok()?
            .get("command")?
            .as_str()
            .map(str::to_owned)
    }

    /// 围栏判定的工作区解析基准：command 的 cwd（roots 内相对/绝对路径）
    /// 相对项目主目录解析；未给 cwd 或解析异常时用项目主目录（cwd 的
    /// 存在性与归属校验在执行层 command::prepare 做，门控层宽松取基准）。
    fn judge_base(&self, args_json: &str) -> PathBuf {
        let default = self
            .workspace
            .as_ref()
            .map(|ws| ws.base.clone())
            .unwrap_or_default();
        let cwd = serde_json::from_str::<serde_json::Value>(args_json)
            .ok()
            .and_then(|v| v.get("cwd").and_then(|c| c.as_str()).map(str::to_owned));
        match cwd {
            Some(cwd) if !default.as_os_str().is_empty() => {
                let p = std::path::Path::new(&cwd);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    default.join(p)
                }
            }
            _ => default,
        }
    }

    /// 围栏判定（22 号票：AST 写目标分析 + 四象限）。
    fn fence_judge(&self, command: &str, args_json: &str) -> crate::safety::fence::FenceOutcome {
        let shell = ast::Shell::host_default();
        match &self.workspace {
            Some(ws) => {
                let base = self.judge_base(args_json);
                let workspace = ast::Workspace {
                    roots: &ws.roots,
                    base: &base,
                    fs: &FsCanonicalizer,
                };
                self.fence
                    .judge_in_workspace(command, self.tier, shell, Some(&workspace))
            }
            // 未装配工作区上下文：退化语义——有写目标即 Confirm（保守）。
            None => self
                .fence
                .judge_in_workspace(command, self.tier, shell, None),
        }
    }

    /// 计划是否"已过期"：存在已批准的计划且条目全部完成——此时继续写文件
    /// 应被计划门拦下（§8.6）。
    fn plan_stale(&self) -> bool {
        self.plan
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(PlanState::approved_and_all_completed)
    }

    /// 内部状态禁区命中判定（23 号票，§9.3）：FileWrite 静态工具（write/
    /// edit）取 `path` 参数按项目主目录展开；command 取 AST 写目标（22 号
    /// 票接缝）逐个判定。返回 `Some(人话原因)` = 命中禁区（E_INTERNAL_STATE
    /// 的 message）。未装配 guard / 目标含未解析变量（Opaque——值未知不猜，
    /// 交围栏字面量掩码保守 Confirm + 36 号票审计）/ AST 解析失败（交围栏
    /// fail-closed）均不命中。
    fn internal_state_hit(
        &self,
        name: &str,
        args_json: &str,
        spec: Option<&ToolSpec>,
    ) -> Option<String> {
        let guard = self.internal.as_ref()?;
        let is_file_write = spec.is_some_and(|s| {
            s.origin == ToolOrigin::Static && s.capability == Capability::FileWrite
        });
        if name == "command" {
            let command = Self::command_of(args_json)?;
            // AST 解析失败 → 无目标可查（解析失败面由围栏 fail-closed Confirm 承接）。
            let ast::ParsedCommand::Ok { targets, .. } =
                ast::parse_command(ast::Shell::host_default(), &command)
            else {
                return None;
            };
            let base = self.judge_base(args_json);
            for target in &targets {
                // 词法目标才可判定：Opaque（含 $VAR/命令替换/波浪号）不硬拒。
                let TargetPath::Literal(literal) = &target.path else {
                    continue;
                };
                let lexical = lexical_abs(literal, &base);
                if let Some(reason) = guard.hit(&target.raw, &lexical, &FsCanonicalizer) {
                    return Some(reason);
                }
            }
            None
        } else if is_file_write {
            let path = serde_json::from_str::<serde_json::Value>(args_json)
                .ok()?
                .get("path")?
                .as_str()?
                .to_owned();
            // write/edit 的相对路径基准与执行侧 PathGuard 一致 = 项目主目录
            // （§9.1 roots[0]）；未装配工作区时无基准（headless 测试缺省面，
            // 相对路径无从展开——交执行层 safe_join 兜底）。
            let base = self.workspace.as_ref()?.base.clone();
            let lexical = lexical_abs(&path, &base);
            guard.hit(&path, &lexical, &FsCanonicalizer)
        } else {
            None
        }
    }

    /// 命令是否命中 commandAllowlist（④ 启用且条目词序列前缀命中）。
    fn command_whitelisted(&self, command: &str) -> bool {
        self.policy.allowlist_enabled
            && !self.policy.command_allowlist.is_empty()
            && self
                .policy
                .command_allowlist
                .iter()
                .any(|entry| allowlist_hit(command, entry))
    }

    /// 外部目录授权命中判定（24 号票，FR-05-12/§9.9）：静态文件类工具的
    /// 路径参数经 safe_join 同款归一后落在会话 roots（含已授权目录）之外
    /// → 返回 Some(授权目标目录)。授权目标按参数语义取：文件类工具
    /// （read/write/edit/batch_read）= 路径的父目录；目录类工具
    /// （list_files/glob/grep）= 路径本身——即「该目录及子目录」的授权
    /// 粒度。未装配 workspace（headless 测试缺省）/参数无路径/路径解析
    /// 失败 → None（交执行层回喂错误）。
    fn external_dir_hit(
        &self,
        name: &str,
        args_json: &str,
        spec: Option<&ToolSpec>,
    ) -> Option<PathBuf> {
        let ws = self.workspace.as_ref()?;
        let is_static_file_tool = spec.is_some_and(|s| {
            s.origin == ToolOrigin::Static
                && matches!(s.capability, Capability::ReadOnly | Capability::FileWrite)
                && matches!(
                    name,
                    "read" | "write" | "edit" | "list_files" | "grep" | "glob" | "batch_read"
                )
        });
        if !is_static_file_tool {
            return None;
        }
        let args: serde_json::Value = serde_json::from_str(args_json).ok()?;
        // 路径语义分型：目录类工具授权目标 = 路径本身；文件类 = 父目录。
        let dir_semantics = matches!(name, "list_files" | "glob" | "grep");
        let raw_paths: Vec<String> = if name == "batch_read" {
            args.get("paths")?
                .as_array()?
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        } else {
            vec![args.get("path")?.as_str()?.to_owned()]
        };
        if raw_paths.is_empty() {
            return None;
        }
        let external_roots = self
            .external_dirs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for raw in raw_paths {
            let p = Path::new(&raw);
            let absolute = if p.is_absolute() {
                p.to_path_buf()
            } else {
                ws.base.join(p)
            };
            // 归一失败（路径畸形/悬挂过深）：交执行层回喂参数错误，不挂起。
            let Ok(resolved) = canonicalize_best_effort(&absolute) else {
                continue;
            };
            let inside = ws
                .roots
                .iter()
                .chain(external_roots.iter())
                .any(|r| resolved == *r || resolved.starts_with(r));
            if inside {
                continue;
            }
            // 授权目标：目录类工具（list_files/glob/grep）= 路径本身；
            // 文件类工具 = 父目录（「该目录及子目录」粒度）。目标不存在时
            // canonicalize_best_effort 已归到最长存在前缀，父目录回退安全。
            let target = if dir_semantics {
                resolved
            } else {
                resolved.parent().map(Path::to_path_buf).unwrap_or(resolved)
            };
            return Some(target);
        }
        None
    }

    /// 授权目标是否属「整体放行」禁区（§9.9：文件系统根/家目录本身/宿主
    /// 托管目录及其上/下范围——与 host 侧 allow_external_dir 同口径，见
    /// workspace.rs [`is_protected_grant_target`] 的注释）。未装配
    /// data_dir（headless）不拦——命令面兜底校验。
    fn protected_grant_target(&self, dir: &Path) -> Option<String> {
        let data_dir = self.data_dir.as_ref()?;
        let is_root = dir.components().all(|c| {
            matches!(
                c,
                std::path::Component::RootDir | std::path::Component::Prefix(_)
            )
        });
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()
            .map(PathBuf::from);
        let protected = is_root
            || home.is_some_and(|h| dir == h || h.starts_with(dir))
            || dir == data_dir.as_path()
            || data_dir.starts_with(dir)
            || dir.starts_with(data_dir.as_path());
        protected.then(|| {
            format!(
                "拒绝整体放行：不允许授权文件系统根、家目录本身或宿主托管目录（{}）及其上/下范围：{}",
                data_dir.display(),
                dir.display()
            )
        })
    }

    /// 白名单豁免与审批总开关豁免的公共前提：非 plan 档——plan 档的只读
    /// 围栏与过滤是档位语义（FR-05-01），白名单与总开关都不解锁计划档写入。
    fn exemptions_apply(&self) -> bool {
        self.tier != PermissionTier::Plan
    }
}

/// 词法绝对化：绝对路径原样；相对路径基于给定基准展开（command 的 cwd
/// 基准 / write-edit 的项目主目录）。
fn lexical_abs(literal: &str, base: &Path) -> PathBuf {
    let p = Path::new(literal);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    }
}

/// 审批门策略实现：按文件头列出的顺序判定（首个命中即出结论）。
impl ToolGate for ApprovalGate {
    /// 判定一次工具调用：返回放行/挂起/硬拦（各步骤见内部行注释）。
    fn judge(&self, name: &str, args_json: &str) -> GateVerdict {
        let spec = lookup_spec(name);
        // (0) 内部状态硬拒（23 号票，FR-05-08/§9.3）：禁区写入直接拒绝——
        // 置于全部放行路径之前，权限档/白名单/自动确认/审批总开关/已批准
        // 签名都不豁免（签名放行前再查一道属防御纵深：禁区调用不可能曾获
        // 批准，zone 清单在同会话内扩展时旧签名也不会复活写权）。
        if let Some(reason) = self.internal_state_hit(name, args_json, spec.as_ref()) {
            return GateVerdict::Blocked {
                code: E_INTERNAL_STATE,
                message: reason,
            };
        }
        // (0.5) 外部目录授权（24 号票，FR-05-12/§9.9）：文件类工具访问会话
        // roots（含已授权目录）外的路径 → 挂起弹「始终允许该目录」询问
        // （粒度 = 该目录及子目录的读/写/列）；授权目标属「整体放行」禁区
        // （根/家目录/宿主托管目录）→ E_INVALID_ARG 硬拦。置于签名放行之前：
        // 恢复重放时授权已并入 roots/external_dirs，判定不再命中，签名放行
        // 语义不变；批量调用（batch_read 跨多个外部目录）每次只授权一个，
        // 其余在重放时继续逐个询问（签名放行不吞并未授权路径）。信任面
        // 判定不受审批总开关/白名单/权限档豁免（同内部状态硬拒精神）。
        if let Some(target) = self.external_dir_hit(name, args_json, spec.as_ref()) {
            if let Some(reason) = self.protected_grant_target(&target) {
                return GateVerdict::Blocked {
                    code: E_INVALID_ARG,
                    message: reason,
                };
            }
            return GateVerdict::Suspend {
                fence_reason: None,
                external_dir: Some(target.to_string_lossy().into_owned()),
            };
        }
        let Some(spec) = spec else {
            // 未知工具：放行给 executor 回喂 E_INVALID_ARG（不在这里拦截）
            return GateVerdict::Allow;
        };
        // (1) 已批准签名：恢复重放的挂起调用放行；会话级「始终允许」桶
        // 命中（23 号票 always_session，跨 run 生效）同样放行
        let approved_here = self
            .approved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&(name.to_owned(), args_json.to_owned()));
        let approved_session = self
            .session_memory
            .as_ref()
            .is_some_and(|m| m.contains(name, args_json));
        if approved_here || approved_session {
            return GateVerdict::Allow;
        }
        // 白名单判定一次（围栏 Confirm 与矩阵 NeedsApproval 两处消费）
        let whitelisted = name == "command"
            && Self::command_of(args_json).is_some_and(|c| self.command_whitelisted(&c));
        // (2) 围栏（command 专属，§5.4 ③；22 号票起 AST 写目标分析：
        // Block 硬拦 / Confirm 挂起并带具体原因 / Allow 落矩阵合成）。
        // 黑名单 Block 永不豁免（§8.5 灾难级铁律，白名单也不例外）；
        // Confirm 在白名单命中（非 plan 档）时免除（23 号票）。
        if name == "command"
            && let Some(command) = Self::command_of(args_json)
        {
            let outcome = self.fence_judge(&command, args_json);
            match outcome.verdict {
                FenceVerdict::Block => {
                    return GateVerdict::Blocked {
                        code: E_FENCE_BLOCKED,
                        message: outcome
                            .reason
                            .unwrap_or_else(|| format!("围栏拦截（灾难级命令）：{command}")),
                    };
                }
                FenceVerdict::Confirm => {
                    // 免除围栏 Confirm 挂起的两个条件（均限非 plan 档）：
                    // 白名单命中（④ commandAllowlist）/ 审批总开关关闭（①）。
                    // 免除后继续走计划门、MCP 分支与矩阵合成，不直接放行。
                    if self.exemptions_apply() && (whitelisted || !self.policy.enabled) {
                        // 落入 (3)(4)(5)
                    } else {
                        return GateVerdict::Suspend {
                            fence_reason: Some(
                                outcome
                                    .reason
                                    .unwrap_or_else(|| "命令含写目标，需确认".to_owned()),
                            ),
                            external_dir: None,
                        };
                    }
                }
                FenceVerdict::Allow => {}
            }
        }
        // (3) 计划门（§8.6：FileWrite/Network 执行前校验）
        if matches!(
            spec.capability,
            crate::safety::tier::Capability::FileWrite | crate::safety::tier::Capability::Network
        ) && self.plan_stale()
        {
            return GateVerdict::Blocked {
                code: E_PLAN_STALE,
                message: "已批准的计划条目全部完成，继续写入前请先用 plan 工具更新计划".to_owned(),
            };
        }
        // (4) MCP 动态工具分支（08 号票，§9.7 Q44）：未声明 read_only 且
        // 无许可 → 即便 full_access 也挂起。放行条件二选一：
        //   a. 用户配置 read_only=true（spec.capability 已是 ReadOnly，落
        //      到 (5) 矩阵按 ReadOnly 放行）；
        //   b. Q44 许可三元组命中（服务器启动指纹 + 原始工具名 + 输入
        //      schema 全等）——来源是用户审批「始终允许（当前版本）」或
        //      服务器级 always_allow=true 的清单同步自动授权（schema 有
        //      未确认变更时不自动放行，见 mcp/pool 的 auto-grant）。
        //      许可判定与服务器级声明解耦：用户点「始终允许」后立即生效
        //      （热更新，FR-08-04），不要求配置里另开开关。
        //      本分支不受审批总开关/白名单豁免（FR-08-04 高危外部调用
        //      永远过人，23 号票口径）。
        if spec.origin == ToolOrigin::Mcp
            && let Some(mcp) = &self.mcp
            && let Some(view) = mcp.view(name)
            && !view.read_only
            && !mcp.license_hit(&view)
        {
            return GateVerdict::Suspend {
                fence_reason: None,
                external_dir: None,
            };
        }
        // (5) 权限矩阵（§8.5；plan 档 MCP 已被 filter_for_tier 全过滤，
        // 兜底 E_TOOL_BLOCKED）。NeedsApproval 在白名单命中（非 plan 档，
        // 23 号票）或审批总开关关闭（非 plan 档，①）时放行。
        match decide_gate(spec.capability, self.tier) {
            GateDecision::Filtered => GateVerdict::Blocked {
                code: E_TOOL_BLOCKED,
                message: format!("权限档排除：{name}（当前档 {}）", tier_key(self.tier)),
            },
            GateDecision::NeedsApproval => {
                if (whitelisted || !self.policy.enabled) && self.exemptions_apply() {
                    GateVerdict::Allow
                } else {
                    GateVerdict::Suspend {
                        fence_reason: None,
                        external_dir: None,
                    }
                }
            }
            _ => GateVerdict::Allow,
        }
    }
}

/// 权限档的对外键名（错误提示文案里用）。
fn tier_key(tier: PermissionTier) -> &'static str {
    match tier {
        PermissionTier::Plan => "plan",
        PermissionTier::ConfirmEach => "confirm_each",
        PermissionTier::AutoEdit => "auto_edit",
        PermissionTier::FullAccess => "full_access",
    }
}
