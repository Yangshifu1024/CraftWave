/**
 * IPC 契约类型出口（§2.4 ipc/types.ts）。
 *
 * 02 号票起（C-2 定案）payload 类型的单一来源切换为 ts-rs 生成物：Rust 契约
 * 类型 derive(TS) 后由 cargo test 统一导出到 `./generated/`（生成物进 git，
 * 见 src-tauri/src/ipc_manifest_test.rs 的导出面），本文件只做再导出
 * （re-export）——禁止在本文件手写 payload 类型，防止双源漂移。
 *
 * 纪律说明：
 * - 生成物按 Rust 类型一文件一名（D14 两侧同名）；字段可缺省（`?`）即 serde
 *   Option/缺省字段，线上传输时该键可省略；
 * - 历史手写镜像别名已收编为 Rust 同名：ToolOutcomeJson→ToolOutcome、
 *   ThemeName→Theme、WorkspaceReadRange→ReadRange；无消费面的
 *   NotificationPermission 镜像已删除（status 字段为 string，与 Rust 发射面一致）；
 * - 仅存的手写面是 §3.5 事件载荷族（Rust 侧以 json! 字面量发射、无结构体
 *   来源，随对应功能票收编），其引用的类型仍来自生成物。
 *
 * 键面不动：client.ts 的 COMMANDS/EVENT_KEYS 注册表与 manifest 双向扫描照旧
 * （§12.3，C-2 明文）；生成物 ↔ 再导出面的一致性由 contract.test 扫描钉住。
 */

// ---- 生成物再导出（与 ui/src/ipc/generated/ 一一对应）----
// 本文件后半的事件载荷族与收窄别名引用生成物类型，本地可见性走显式 import
// （`export *` 只转发、不引入模块作用域）。
import type { AskPayload } from "./generated/AskPayload";
import type { McpScope } from "./generated/McpScope";
import type { PlanItem } from "./generated/PlanItem";
import type { StoredEntry } from "./generated/StoredEntry";
import type { UsageTotal } from "./generated/UsageTotal";

export * from "./generated/AgentConfig";
export * from "./generated/AllowExternalDirArgs";
export * from "./generated/ApiFormat";
export * from "./generated/AppConfig";
export * from "./generated/ApprovalConfig";
export * from "./generated/ApprovalDiff";
export * from "./generated/ApprovePlanArgs";
export * from "./generated/AskAnswer";
export * from "./generated/AskChoice";
export * from "./generated/AskMode";
export * from "./generated/AskOption";
export * from "./generated/AskPayload";
export * from "./generated/AskQuestion";
export * from "./generated/AttachImageArgs";
export * from "./generated/AttachImageResult";
export * from "./generated/AttachRunArgs";
export * from "./generated/AttachRunResult";
export * from "./generated/BuiltinPresets";
export * from "./generated/CancelRunArgs";
export * from "./generated/ChatFrame";
export * from "./generated/CheckExternalPathArgs";
export * from "./generated/CheckExternalPathResult";
export * from "./generated/CompactionConfig";
export * from "./generated/CompactSessionArgs";
export * from "./generated/CompactSessionResult";
export * from "./generated/CompleteReason";
export * from "./generated/CreateSessionArgs";
export * from "./generated/DayUsage";
export * from "./generated/DeleteProjectArgs";
export * from "./generated/DeleteProviderArgs";
export * from "./generated/DeleteSessionArgs";
export * from "./generated/DiffHunk";
export * from "./generated/DiffLine";
export * from "./generated/DiffLineKind";
export * from "./generated/DiffPatch";
export * from "./generated/EndReason";
export * from "./generated/ExternalRootsResult";
export * from "./generated/FrameError";
export * from "./generated/GetSessionPrefsResult";
export * from "./generated/GetUiStateArgs";
export * from "./generated/GitCommitInfo";
export * from "./generated/GitDiffArgs";
export * from "./generated/GitDiffResult";
export * from "./generated/GitLogArgs";
export * from "./generated/GitLogResult";
export * from "./generated/GitStatusArgs";
export * from "./generated/GitStatusFile";
export * from "./generated/GitStatusResult";
export * from "./generated/GitUser";
export * from "./generated/IncompleteReason";
export * from "./generated/InjectMode";
export * from "./generated/InjectRunMessageArgs";
export * from "./generated/InjectRunMessageResult";
export * from "./generated/IpcError";
export * from "./generated/Language";
export * from "./generated/LaunchDecision";
export * from "./generated/ListSessionsArgs";
export * from "./generated/LoadSessionTreeArgs";
export * from "./generated/LoadSubagentHistoryArgs";
export * from "./generated/LoadToolOutcomesArgs";
export * from "./generated/LogConfig";
export * from "./generated/LogLevel";
export * from "./generated/McpConfigFile";
export * from "./generated/McpListConfigArgs";
export * from "./generated/McpListConfigResult";
export * from "./generated/McpSaveConfigArgs";
export * from "./generated/McpScope";
export * from "./generated/McpServerConfig";
export * from "./generated/McpServerRefArgs";
export * from "./generated/McpSetLaunchTrustArgs";
export * from "./generated/McpSnapshotArgs";
export * from "./generated/McpSnapshotResult";
export * from "./generated/McpSnapshotServer";
export * from "./generated/McpTestArgs";
export * from "./generated/McpTestResult";
export * from "./generated/McpTestTool";
export * from "./generated/McpToolFilter";
export * from "./generated/McpTransportKind";
export * from "./generated/MessageEntry";
export * from "./generated/MessageRole";
export * from "./generated/MessageStatus";
export * from "./generated/MetaEntry";
export * from "./generated/ModelUsage";
export * from "./generated/NetworkConfig";
export * from "./generated/NewProjectInput";
export * from "./generated/NotificationConfig";
export * from "./generated/NotificationPermissionStatus";
export * from "./generated/OpenDirArgs";
export * from "./generated/PathKindArgs";
export * from "./generated/PathKindResult";
export * from "./generated/PermissionTier";
export * from "./generated/PlanItem";
export * from "./generated/PlanItemStatus";
export * from "./generated/PresetModel";
// HTML 沙箱预览注册（14 号票，§3.3 #59g）
export * from "./generated/PreviewRegisterArgs";
export * from "./generated/PreviewRegisterResult";
export * from "./generated/ProbeError";
export * from "./generated/ProjectMeta";
export * from "./generated/ProviderConfig";
export * from "./generated/ProviderModel";
export * from "./generated/ProviderPreset";
export * from "./generated/ProxyConfig";
export * from "./generated/ProxyMode";
export * from "./generated/QueuedImageAttachment";
// 配额快照（21 号票，§3.3 #53）
export * from "./generated/QuotaData";
export * from "./generated/QuotaEntry";
export * from "./generated/QuotaSnapshot";
export * from "./generated/QuotaStatus";
export * from "./generated/ReadImageBlobArgs";
export * from "./generated/ReadImageBlobResult";
export * from "./generated/ReadRange";
export * from "./generated/ReadWorkspaceFileArgs";
export * from "./generated/ReadWorkspaceResult";
export * from "./generated/ReasoningEffort";
export * from "./generated/RemoveFollowupArgs";
export * from "./generated/RemoveFollowupResult";
export * from "./generated/RenameSessionArgs";
export * from "./generated/RequiresActionReason";
export * from "./generated/ResolveAskArgs";
export * from "./generated/ResolveExitRequestArgs";
export * from "./generated/ResumeFollowupQueueArgs";
export * from "./generated/ResumeFollowupQueueResult";
export * from "./generated/RevokeExternalDirArgs";
export * from "./generated/RunConfig";
export * from "./generated/RunSnapshot";
export * from "./generated/RunSnapshotStatus";
export * from "./generated/SaveConfigArgs";
export * from "./generated/SaveProjectArgs";
export * from "./generated/SaveProviderArgs";
export * from "./generated/SaveSessionTreeArgs";
export * from "./generated/SaveSessionTreeResult";
export * from "./generated/SearchConfig";
export * from "./generated/SearchTestArgs";
export * from "./generated/SearchTestResult";
export * from "./generated/SearchWorkspacePathsArgs";
export * from "./generated/SearxngConfig";
export * from "./generated/SequencedFrame";
export * from "./generated/SessionEntry";
export * from "./generated/SessionMeta";
export * from "./generated/SessionPendingAsk";
export * from "./generated/SessionPendingAskArgs";
export * from "./generated/SessionPrefsArgs";
export * from "./generated/SessionRunSnapshotArgs";
export * from "./generated/SessionTreeData";
// 用量统计（20 号票，§3.3 #52/#59k）
export * from "./generated/SessionUsageArgs";
export * from "./generated/SessionUsageResult";
export * from "./generated/SetSessionPrefsArgs";
export * from "./generated/SetSessionPrefsPayload";
export * from "./generated/SetUiStateArgs";
export * from "./generated/SkillInspectArgs";
export * from "./generated/SkillInspectResult";
export * from "./generated/SkillListEntry";
export * from "./generated/SkillRevokeTrustArgs";
export * from "./generated/SkillSource";
export * from "./generated/SkillToggleArgs";
export * from "./generated/SkillTrustScope";
export * from "./generated/StartChatArgs";
export * from "./generated/StartChatResult";
export * from "./generated/StatsRange";
export * from "./generated/StatsTokenArgs";
export * from "./generated/StatsTokenResult";
export * from "./generated/StopSubagentArgs";
export * from "./generated/StoredCompactionEntry";
export * from "./generated/StoredEntry";
export * from "./generated/SubMeta";
export * from "./generated/SubOutcome";
export * from "./generated/SubSpawnArgs";
export * from "./generated/serde_json/JsonValue";
export * from "./generated/TestProviderArgs";
export * from "./generated/TestProviderResult";
export * from "./generated/Theme";
export * from "./generated/ThinkingLevel";
export * from "./generated/ToolError";
export * from "./generated/ToolOutcome";
export * from "./generated/ToolsConfig";
export * from "./generated/TotalUsage";
export * from "./generated/TrustDecision";
export * from "./generated/UiConfig";
export * from "./generated/UsageTotal";
export * from "./generated/WatchdogConfig";
export * from "./generated/WirePart";
export * from "./generated/WorkspacePathHit";

/** message 存储条目 = StoredEntry 的 message 变体（kind 判别收窄，§6.1）。
 *  类型级派生别名，非手写镜像。 */
export type StoredMessageEntry = Extract<StoredEntry, { kind: "message" }>;

/** meta 存储条目 = StoredEntry 的 meta 变体（会话级元信息，§6.1）。 */
export type StoredMetaEntry = Extract<StoredEntry, { kind: "meta" }>;

/** session 存储条目 = StoredEntry 的 session 变体（会话头，§6.1）。 */
export type StoredSessionEntry = Extract<StoredEntry, { kind: "session" }>;

// ---- §3.5 事件载荷族（手写聚合面：Rust 发射点为 json! 字面量，无结构体来源）----

/** ask:opened 事件载荷：后端打开了询问/审批，前端应弹出对应面板。 */
export type AskOpenedEvent = {
  sessionId: string;
  askId: string;
  payload: AskPayload;
};

/** ask:closed 事件载荷：询问被关闭（answered = 是否已有应答）。 */
export type AskClosedEvent = {
  askId: string;
  answered: boolean;
};

/**
 * plan:update 事件载荷：全量计划态（25 号票扩展——计划卡实时刷新 + 基线
 * 标记与待批准范围展示）。paths = 模型声明的写范围；approved/baselinePaths
 * = 批准态与冻结基线（approve_plan 冻结后同键补发一次）。
 */
export type PlanUpdateEvent = {
  sessionId: string;
  items: PlanItem[];
  paths: string[];
  approved: boolean;
  baselinePaths: string[];
};

/** run:suggestions 事件载荷：run 结束后给出的建议提示词（输入框 ghost 预填）。 */
export type RunSuggestionsEvent = {
  sessionId: string;
  runId: string;
  text: string;
};

/** tokens:update 事件载荷：会话累计 token 用量的权威校正（覆盖前端累计值）。
 *  20 号票增补 modelId：本次增量贡献的实际执行模型（Inspector 统计区块对
 *  当日/该模型条目做本地增量修正的依据；缺省 = 老载荷兼容，回退整段重拉）。 */
export type TokensUpdateEvent = {
  sessionId: string;
  usage: UsageTotal;
  modelId?: string;
};

/** run:compacting 事件载荷：上下文压缩开始（Composer 指示条驱动，04 号票）。 */
export type RunCompactingEvent = {
  sessionId: string;
};

/** run:compacted 事件载荷：压缩完成（前后上下文占用估算，指示条收口 +
 *  边界投影刷新）。 */
export type RunCompactedEvent = {
  sessionId: string;
  tokensBefore: number;
  tokensAfter: number;
};

/** run:compact_failed 事件载荷：压缩失败（行内轻提示，不打断输入）。 */
export type RunCompactFailedEvent = {
  sessionId: string;
};

/** run:start 事件载荷（06 号票实装）：run 任务确认启动——手动 start_chat 与
 *  后端自动投递的新 run 都发。前端以「本会话是否运行态」区分：非运行态收到
 *  即后端自动投递，经 attach_run 观察态（§5.8）接入续流。 */
export type RunStartEvent = {
  sessionId: string;
  runId: string;
};

/** run:inject 事件载荷（06 号票实装）：steer 被消费（turn 边界）。前端据
 *  entryId/text 本地插入插话节点（canonical 由 Rust 侧同 id 落盘，§5.5）。 */
export type RunInjectEvent = {
  sessionId: string;
  runId: string;
  acceptedAtTurn: number;
  entryId: string;
  text: string;
};

/** run:queue 事件载荷（06 号票）：followUp 队列状态变化——入队/出队/移除/
 *  暂停/恢复即发，队列条以此驱动增删（后端 queue.json 为权威）。 */
export type RunQueueEvent = {
  sessionId: string;
  size: number;
  paused: boolean;
  /** 清空返还的文本数组（取消/失败交还草稿；正常状态变化时缺省）。 */
  returned?: string[];
};

/** mcp:status 事件载荷（08 号票，§3.5 M2 键）：MCP 连接池状态迁移——
 *  设置页服务器行的状态徽标实时驱动（Rust 发射点为 json! 字面量）。 */
export type McpStatusEvent = {
  scope: McpScope;
  name: string;
  state: "starting" | "ready" | "stopped" | "error";
  /** 状态明细（Error 原因 / Ready 工具数说明；无则缺省）。 */
  detail?: string;
};

// ---- 子代理生命周期事件载荷族（10 号票，§3.5【M2】六键；Rust 发射面为
// json! 字面量、无结构体来源，随本票手写收编——payload 统一形如
// { sessionId, runId, subId, ...可选字段 }）----

/** sub:spawn：子代理委派启动（跨 Tab Inspector 订阅）。 */
export type SubSpawnEvent = {
  sessionId: string;
  runId: string;
  subId: string;
  role: string;
};

/** sub:step：子代理轮次推进。 */
export type SubStepEvent = {
  sessionId: string;
  runId: string;
  subId: string;
  turn: number;
};

/** sub:report：`<report>` 结构化汇报全文产出。 */
export type SubReportEvent = {
  sessionId: string;
  runId: string;
  subId: string;
  report: string;
};

/** sub:usage：子代理自身用量累计（Q40：仅子面板展示，不重复累进会话总量）。 */
export type SubUsageEvent = {
  sessionId: string;
  runId: string;
  subId: string;
  usage: UsageTotal;
};

/** sub:done：子代理终态（四象限）。 */
export type SubDoneEvent = {
  sessionId: string;
  runId: string;
  subId: string;
  outcome: "report" | "partial" | "no_report" | "cancelled";
};

/** sub:error：子代理异常终态说明。 */
export type SubErrorEvent = {
  sessionId: string;
  runId: string;
  subId: string;
  code?: string;
  message?: string;
};

/** session:title 事件载荷（27 号票，§3.5 P0 键）：自动起名完成——meta 行 +
 *  index 双写落盘之后发出；失败静默兜底的标题同样经本事件带出（只有
 *  「无标题可落」才不发）。前端据以就地更新左栏列表与卡头标题。 */
export type SessionTitleEvent = {
  sessionId: string;
  title: string;
};
