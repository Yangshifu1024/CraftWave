/**
 * IPC 客户端唯一出口（§2.4 ipc/client.ts）。
 *
 * IPC（Inter-Process Communication，进程间通信）：前端界面（WebView）与
 * Rust 后端之间的调用通道，由 Tauri 的 invoke 函数承担——前端以命令名发起
 * 请求，Rust 侧按命令名执行并返回结果。事件（listenEvent）则是反方向的
 * 通知：后端主动推送给前端。
 *
 * "唯一出口"的含义：所有命令名字符串集中在本文件的 COMMANDS 注册表，其余
 * 代码一律经由本文件导出的函数发起 IPC，禁止散落携带命令名字面量的 invoke
 * 调用。好处有二：命令名改动只需改一处；契约测试（contract.test.ts）可扫描
 * 本文件与 Rust 侧产出的命令清单双向比对，确保前后端接口没有漂移。
 */

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AppConfig,
  AskAnswer,
  AttachRunResult,
  CompactSessionArgs,
  CompactSessionResult,
  DeleteProviderArgs,
  InjectRunMessageArgs,
  InjectRunMessageResult,
  IpcError,
  LaunchDecision,
  LoadSubagentHistoryArgs,
  McpListConfigResult,
  McpSaveConfigArgs,
  McpScope,
  McpServerRefArgs,
  McpSetLaunchTrustArgs,
  McpSnapshotResult,
  McpTestArgs,
  McpTestResult,
  MessageEntry,
  NotificationPermissionStatus,
  OpenDirArgs,
  PreviewRegisterResult,
  ProjectMeta,
  ProviderConfig,
  ReadWorkspaceResult,
  RemoveFollowupResult,
  ResumeFollowupQueueResult,
  RunSnapshot,
  SaveConfigArgs,
  SaveProjectArgs,
  SaveProviderArgs,
  SaveSessionTreeArgs,
  SaveSessionTreeResult,
  SearchTestArgs,
  SearchTestResult,
  SequencedFrame,
  SessionMeta,
  SessionPendingAsk,
  SessionTreeData,
  SkillInspectArgs,
  SkillInspectResult,
  SkillListEntry,
  SkillToggleArgs,
  StartChatArgs,
  StartChatResult,
  StopSubagentArgs,
  ToolOutcome,
  WorkspacePathHit,
} from "./types";

/**
 * 本文件已接线的命令名注册表（§3.3 键面）。值用 snake_case（单词小写、下划线
 * 分隔，如 start_chat），与 Rust 侧 #[tauri::command] 的命令名一致；契约测试
 * 据此与 Rust 产出的 ipc-manifest.json（命令清单文件）双向比对。
 */
export const COMMANDS = {
  activateCustomTitlebar: "activate_custom_titlebar",
  restoreNativeTitlebar: "restore_native_titlebar",
  selectDir: "select_dir",
  saveSessionTree: "save_session_tree",
  loadSessionTree: "load_session_tree",
  listSessions: "list_sessions",
  loadToolOutcomes: "load_tool_outcomes",
  getConfig: "get_config",
  saveConfig: "save_config",
  saveProvider: "save_provider",
  deleteProvider: "delete_provider",
  openDir: "open_dir",
  startChat: "start_chat",
  cancelRun: "cancel_run",
  injectRunMessage: "inject_run_message",
  resumeFollowupQueue: "resume_followup_queue",
  removeFollowup: "remove_followup",
  resolveAsk: "resolve_ask",
  approvePlan: "approve_plan",
  compactSession: "compact_session",
  attachRun: "attach_run",
  sessionRunSnapshot: "session_run_snapshot",
  sessionPendingAsk: "session_pending_ask",
  listProjects: "list_projects",
  saveProject: "save_project",
  deleteProject: "delete_project",
  createSession: "create_session",
  deleteSession: "delete_session",
  renameSession: "rename_session",
  searchWorkspacePaths: "search_workspace_paths",
  readWorkspaceFile: "read_workspace_file",
  getUiState: "get_ui_state",
  setUiState: "set_ui_state",
  resolveExitRequest: "resolve_exit_request",
  notificationPermissionStatus: "notification_permission_status",
  openNotificationSettings: "open_notification_settings",
  mcpListConfig: "mcp_list_config",
  mcpSaveConfig: "mcp_save_config",
  mcpConnect: "mcp_connect",
  mcpDisconnect: "mcp_disconnect",
  mcpReconnect: "mcp_reconnect",
  mcpTest: "mcp_test",
  mcpSetLaunchTrust: "mcp_set_launch_trust",
  mcpSnapshot: "mcp_snapshot",
  skillList: "skill_list",
  skillToggle: "skill_toggle",
  skillReload: "skill_reload",
  skillInspect: "skill_inspect",
  // 子代理两命令（10 号票，§3.3 #58/#59）
  stopSubagent: "stop_subagent",
  loadSubagentHistory: "load_subagent_history",
  // 网络搜索连通测试（13 号票，§3.3 #59f）
  searchTest: "search_test",
  // HTML 沙箱预览注册（14 号票，§3.3 #59g）
  previewRegister: "preview_register",
} as const;

/**
 * 本文件已接线的事件键注册表（§3.5：14 号票 5 键 + 17 号票 5 键 + 04 号票
 * 压缩三键 + 06 号票 run:start/run:inject/run:queue 三键）。
 * 事件是后端主动推送给前端的通知（与命令"前端调后端"方向相反），键名按
 * "域名:动作"命名（如 ask:opened = 询问面板被打开）。
 */
export const EVENT_KEYS = {
  askOpened: "ask:opened",
  askClosed: "ask:closed",
  planUpdate: "plan:update",
  runSuggestions: "run:suggestions",
  tokensUpdate: "tokens:update",
  runDone: "run:done",
  runError: "run:error",
  runCancelled: "run:cancelled",
  runStart: "run:start",
  runInject: "run:inject",
  runQueue: "run:queue",
  notifyActivate: "notify:activate",
  appExitRequested: "app:exit_requested",
  runCompacting: "run:compacting",
  runCompacted: "run:compacted",
  runCompactFailed: "run:compact_failed",
  mcpStatus: "mcp:status",
  // 子代理生命周期（10 号票，§3.5【M2】：跨 Tab Inspector 订阅）
  subSpawn: "sub:spawn",
  subStep: "sub:step",
  subReport: "sub:report",
  subUsage: "sub:usage",
  subDone: "sub:done",
  subError: "sub:error",
} as const;

/**
 * 订阅一个后端事件（§3.5）：返回取消订阅函数。
 * 事件监听与 invoke 同为本文件唯一出口——测试注入 fake（见 stores/ask.test.ts）。
 */
export function listenEvent<T>(key: string, handler: (payload: T) => void): Promise<() => void> {
  return listen<T>(key, (event) => handler(event.payload));
}

/** 激活覆盖式自绘标题栏（§2.1 防闪启动协议，App.tsx 看门狗用）。 */
export function activateCustomTitlebar(): Promise<void> {
  return invoke<void>(COMMANDS.activateCustomTitlebar);
}

/** §3.3 #5：原生目录选择；用户取消 → null。 */
export function selectDir(): Promise<string | null> {
  return invoke<string | null>(COMMANDS.selectDir, {});
}

/** 看门狗超时后的退化路径：恢复系统标题栏。 */
export function restoreNativeTitlebar(): Promise<void> {
  return invoke<void>(COMMANDS.restoreNativeTitlebar);
}

/** 全树同步（§6.3）：把 thread.export() 导出的整棵消息树发给 Rust 校验落盘，
 *  Rust 只追加缺失节点并返回新 revision（版本号，用于乐观并发：保存须带上
 *  拿到时的版本，不匹配说明有更新先落了盘，会被拒绝并要求重试）。 */
export function saveSessionTree(args: SaveSessionTreeArgs): Promise<SaveSessionTreeResult> {
  return invoke(COMMANDS.saveSessionTree, {
    sessionId: args.sessionId,
    messages: args.messages,
    activeLeafId: args.activeLeafId,
    baseRevision: args.baseRevision,
  });
}

/** 会话树加载（承载 §6.1 定稿结构：messages 含 status? + activeLeafId + revision）。 */
export function loadSessionTree(sessionId: string): Promise<SessionTreeData> {
  return invoke(COMMANDS.loadSessionTree, { sessionId });
}

/** §3.3 #14：会话列表（index.json，updatedAt 倒序）。 */
export function listSessions(projectId?: string): Promise<SessionMeta[]> {
  return invoke(COMMANDS.listSessions, { projectId });
}

/** §3.3 #29：工具卡恢复——按 callId 批量拉边车；缺文件调用不在返回表。 */
export function loadToolOutcomes(
  sessionId: string,
  callIds: string[],
): Promise<Record<string, ToolOutcome>> {
  return invoke(COMMANDS.loadToolOutcomes, { sessionId, callIds });
}

/** §3.3 #6：读取当前配置。 */
export async function getConfig(): Promise<AppConfig> {
  return invoke<AppConfig>(COMMANDS.getConfig);
}

/** §3.3 #7：整份配置经协调器归一化原子落盘，返回归一化结果。 */
export async function saveConfig(config: SaveConfigArgs["config"]): Promise<AppConfig> {
  return invoke<AppConfig>(COMMANDS.saveConfig, { config } satisfies SaveConfigArgs);
}

/** §3.3 #8：保存供应商；newKey 有值时走 keyring 提交协议（§9.4）。 */
export async function saveProvider(
  provider: ProviderConfig,
  newKey?: string,
): Promise<ProviderConfig> {
  const args: SaveProviderArgs = { provider, newKey };
  return invoke<ProviderConfig>(COMMANDS.saveProvider, { ...args });
}

/** §3.3 #9：删除供应商并同步删除 keyring 账户。 */
export async function deleteProvider(providerId: DeleteProviderArgs["providerId"]): Promise<void> {
  return invoke<void>(COMMANDS.deleteProvider, { providerId } satisfies DeleteProviderArgs);
}

/** §3.3 #3：系统打开目录（03 号票，FR-14-07）。路径必须落在后端白名单内
 *  （数据目录/日志目录/项目数据目录，§7.1），越界 reject E_PATH_OUTSIDE_ROOTS，
 *  目录不存在 reject E_NOT_FOUND，OS 打开失败 reject E_IO。 */
export function openDir(path: string): Promise<void> {
  return invoke<void>(COMMANDS.openDir, { path } satisfies OpenDirArgs);
}

/** §3.3 #18：启动一次 run（一轮模型对话）。channel 由调用方建立：Channel 是
 *  Tauri 提供的流式通道——后端把逐帧产出（文本增量、工具调用等 SequencedFrame）
 *  连续推给 onmessage 回调，Promise 本身只在 run 结束时兑现。 */
export function startChat(args: {
  sessionId: string;
  messages: StartChatArgs["messages"];
  runConfig: StartChatArgs["runConfig"];
  onFrame: (frame: SequencedFrame) => void;
}): Promise<StartChatResult> {
  const channel = new Channel<SequencedFrame>();
  channel.onmessage = args.onFrame;
  return invoke<StartChatResult>(COMMANDS.startChat, {
    sessionId: args.sessionId,
    messages: args.messages,
    runConfig: args.runConfig,
    channel,
  });
}

/** §3.3 #19：取消运行（置位即返回；终态以 runEnded 帧为权威）。 */
export async function cancelRun(sessionId: string): Promise<boolean> {
  return invoke<boolean>(COMMANDS.cancelRun, { sessionId } satisfies { sessionId: string });
}

/** §3.3 #21：注入消息（06 号票）。mode=steer（缺省）为运行中插话（turn 边界
 *  消费，run:inject 回执）；mode=followUp 为排队（Alt+Enter，run 正常完成后
 *  自动投递；空闲时缺省 mode 也落 followUp）。runConfig 为附着快照（投递
 *  新 run 的运行配置来源）。 */
export function injectRunMessage(args: {
  sessionId: string;
  text: string;
  mode?: "steer" | "followUp";
  runConfig?: StartChatArgs["runConfig"];
}): Promise<InjectRunMessageResult> {
  return invoke<InjectRunMessageResult>(COMMANDS.injectRunMessage, {
    sessionId: args.sessionId,
    text: args.text,
    mode: args.mode,
    runConfig: args.runConfig,
  } satisfies InjectRunMessageArgs);
}

/** §3.3 #59d：继续 followUp 队列（06 号票失败暂停的明确继续）。解除暂停后
 *  无活跃 run 且队列非空 → 立即投递队列头开新 run（delivered=true）。 */
export function resumeFollowupQueue(sessionId: string): Promise<ResumeFollowupQueueResult> {
  return invoke<ResumeFollowupQueueResult>(COMMANDS.resumeFollowupQueue, { sessionId });
}

/** §3.3 #59e：移除一条待发消息（队列条 × 删除与「编辑取出」共用）。 */
export function removeFollowup(sessionId: string, entryId: string): Promise<RemoveFollowupResult> {
  return invoke<RemoveFollowupResult>(COMMANDS.removeFollowup, { sessionId, entryId });
}

/** §3.3 #22：应答询问/审批（AskPanel）。槽不存在或已关闭 reject E_NOT_FOUND。 */
export function resolveAsk(askId: string, answer: AskAnswer): Promise<void> {
  return invoke<void>(COMMANDS.resolveAsk, { askId, answer } satisfies {
    askId: string;
    answer: AskAnswer;
  });
}

/** §3.3 #26c：冻结当前计划为批准快照。计划门（§8.6）指"计划需人工批准后
 *  模型才动手执行"的关卡；对应计划卡片上的批准按钮。 */
export function approvePlan(sessionId: string): Promise<void> {
  return invoke<void>(COMMANDS.approvePlan, { sessionId } satisfies { sessionId: string });
}

/** §3.3 #59b：手动压缩会话上下文（FR-03-05，04 号票）。无条件执行一次
 *  压缩（不走 0.6 阈值判定）；运行中 reject E_SESSION_RUNNING。 */
export function compactSession(sessionId: string): Promise<CompactSessionResult> {
  return invoke<CompactSessionResult>(COMMANDS.compactSession, {
    sessionId,
  } satisfies CompactSessionArgs);
}

/** §3.3 #57：attach_run 帧重放重连（§5.8，05 号票）。重放 (sinceSeq, 当前头]
 *  区间帧（经 onFrame 回调逐帧到达）并把帧通道热切换到新 channel；重放窗口
 *  耗尽时补发一次 runSnapshot 摘要帧（负载见 ChatFrame）。无活跃 run /
 *  sessionId 未知 → { replayed: 0 }（runId 省略，幂等查询语义）。 */
export function attachRun(
  sessionId: string,
  onFrame: (frame: SequencedFrame) => void,
  sinceSeq = 0,
): Promise<AttachRunResult> {
  const channel = new Channel<SequencedFrame>();
  channel.onmessage = onFrame;
  return invoke<AttachRunResult>(COMMANDS.attachRun, { sessionId, sinceSeq, channel });
}

/** §3.3 #20：session_run_snapshot 运行快照（§5.6 对账，05 号票）。会话有
 *  活跃 run → 快照（status/usage/lastSeq）；无活跃 run / sessionId 未知
 *  → null（幂等查询语义）。 */
export function sessionRunSnapshot(sessionId: string): Promise<RunSnapshot | null> {
  return invoke<RunSnapshot | null>(COMMANDS.sessionRunSnapshot, { sessionId });
}

/** §3.3 #59c：待审批/询问恢复查询（§5.6，05 号票）。返回该会话仍有效的
 *  未决 ask（原始载荷随行，供重连后恢复 AskPanel）；无未决 ask / sessionId
 *  未知 → null（幂等查询语义）。 */
export function sessionPendingAsk(sessionId: string): Promise<SessionPendingAsk | null> {
  return invoke<SessionPendingAsk | null>(COMMANDS.sessionPendingAsk, { sessionId });
}

/** 把 invoke 失败抛出的错误归一为 §3.1 统一错误负载（{code, message, detail?}）。
 *  Rust 侧正常失败已按此结构序列化；非 IPC 来源的异常兜底为 E_INTERNAL，
 *  让调用方拿到稳定的 code 做分支处理（如 E_SESSION_REV 重试）。 */
export function toIpcError(err: unknown): IpcError {
  if (
    typeof err === "object" &&
    err !== null &&
    "code" in err &&
    typeof (err as { code: unknown }).code === "string"
  ) {
    return err as IpcError;
  }
  return { code: "E_INTERNAL", message: String(err) };
}

// ---- 项目注册表（§3.3 #10–12，16 号票）----

/** §3.3 #10：项目注册表全量。 */
export function listProjects(): Promise<ProjectMeta[]> {
  return invoke(COMMANDS.listProjects, {});
}

/** §3.3 #11：项目新建/更新（homeDir 变更触发 project.json 重绑）。 */
export function saveProject(args: SaveProjectArgs): Promise<ProjectMeta> {
  return invoke<ProjectMeta>(COMMANDS.saveProject, { project: args.project });
}

/** §3.3 #12：删除项目注册（不删项目数据目录；名下会话转临时）。 */
export function deleteProject(projectId: string): Promise<void> {
  return invoke(COMMANDS.deleteProject, { projectId });
}

// ---- 会话生命周期（§3.3 #13/#16/#17，16 号票）----

/** §3.3 #13：新建会话（projectId 缺省 = 临时会话）。 */
export function createSession(projectId?: string): Promise<SessionMeta> {
  return invoke<SessionMeta>(COMMANDS.createSession, { projectId });
}

/** §3.3 #16：删除会话（运行中拒绝 E_SESSION_RUNNING）。 */
export function deleteSession(sessionId: string): Promise<void> {
  return invoke(COMMANDS.deleteSession, { sessionId });
}

/** §3.3 #17：会话重命名。 */
export function renameSession(sessionId: string, title: string): Promise<SessionMeta> {
  return invoke<SessionMeta>(COMMANDS.renameSession, { sessionId, title });
}

// ---- workspace 只读（§3.3 #27/#28，16 号票）----

/** §3.3 #27：@ 引用补全（会话 roots 内基名子串匹配）。 */
export function searchWorkspacePaths(
  sessionId: string,
  query: string,
  limit?: number,
): Promise<WorkspacePathHit[]> {
  return invoke(COMMANDS.searchWorkspacePaths, { sessionId, query, limit });
}

/** §3.3 #28：预览读取（roots 内校验；range 按行翻页；单次 ≤512KB）。 */
export function readWorkspaceFile(
  sessionId: string,
  path: string,
  range?: { offset: number; limit: number },
): Promise<ReadWorkspaceResult> {
  return invoke(COMMANDS.readWorkspaceFile, { sessionId, path, range });
}

// ---- UI 状态（§3.3 #30/#31，16 号票）----

/** §3.3 #30：读取 UI 状态（Json 原样；从未保存过 → null）。 */
export function getUiState(): Promise<unknown> {
  return invoke(COMMANDS.getUiState, {});
}

/** §3.3 #31：整份替换 UI 状态（防抖由前端负责）。 */
export function setUiState(state: unknown): Promise<void> {
  return invoke(COMMANDS.setUiState, { state });
}

// ---- 系统域（§9.6 退出拦截，17 号票）----

/** 退出确认回执（§9.6）：accepted=true 放行退出；false 取消回到托盘常驻。 */
export function resolveExitRequest(accepted: boolean): Promise<void> {
  return invoke<void>(COMMANDS.resolveExitRequest, { accepted });
}

// ---- 通知引导（§9.6，19 号票）----

/** 通知授权态查询：macOS 实查系统授权；查不出（dev 裸进程）→ unknown。 */
export function notificationPermissionStatus(): Promise<NotificationPermissionStatus> {
  return invoke<NotificationPermissionStatus>(COMMANDS.notificationPermissionStatus, {});
}

/** 打开系统通知设置（macOS 直达「通知」面板；Windows 走 ms-settings:）。 */
export function openNotificationSettings(): Promise<void> {
  return invoke<void>(COMMANDS.openNotificationSettings, {});
}

// ---- MCP 客户端（§3.3 #40–47，08 号票）----

/** §3.3 #40：双作用域 mcp.json 原样配置（未知字段保真）。projectAvailable
 *  = 当前有项目会话上下文（false 时 project 内容为空配置、不可编辑）。 */
export function mcpListConfig(): Promise<McpListConfigResult> {
  return invoke(COMMANDS.mcpListConfig, {});
}

/** §3.3 #41：整份保存一个作用域的 mcp.json（原子写；未知字段随文件
 *  round-trip 不丢；保存前校验全部条目可解析，坏条目 reject E_INVALID_ARG）。 */
export function mcpSaveConfig(args: McpSaveConfigArgs): Promise<void> {
  return invoke<void>(COMMANDS.mcpSaveConfig, {
    scope: args.scope,
    config: args.config,
  } satisfies McpSaveConfigArgs);
}

/** §3.3 #42：连接一个服务器。首次连接（或配置指纹变更）触发 TOFU——
 *  弹 mcpTrust 询问（ask:opened 事件，sessionId 为空串 = 应用级询问）；
 *  用户拒绝 → reject E_CANCELLED。连接为后台任务，状态经 mcp:status 可见。 */
export function mcpConnect(scope: McpScope, name: string): Promise<void> {
  return invoke<void>(COMMANDS.mcpConnect, { scope, name } satisfies McpServerRefArgs);
}

/** §3.3 #43：断开（desired=false 停止自动重连；动态工具段即时收窄）。 */
export function mcpDisconnect(scope: McpScope, name: string): Promise<void> {
  return invoke<void>(COMMANDS.mcpDisconnect, { scope, name } satisfies McpServerRefArgs);
}

/** §3.3 #44：重连（先断开收口再连接；配置指纹变更会重新走 TOFU）。 */
export function mcpReconnect(scope: McpScope, name: string): Promise<void> {
  return invoke<void>(COMMANDS.mcpReconnect, { scope, name } satisfies McpServerRefArgs);
}

/** §3.3 #45：草稿试连（initialize + tools/list，总预算 60 秒）。不进连接池、
 *  不写信任、不发状态事件——管理页「测试连接」按钮的探测面。 */
export function mcpTest(args: McpTestArgs): Promise<McpTestResult> {
  return invoke<McpTestResult>(COMMANDS.mcpTest, { draft: args.draft } satisfies McpTestArgs);
}

/** §3.3 #46：TOFU 决策写入 `~/.craftwave/trust/mcp.json`（remember 记住此
 *  版本 / revoke 撤销；revoke 连带撤销该服务器全部工具级许可，Q44）。 */
export function mcpSetLaunchTrust(fingerprint: string, decision: LaunchDecision): Promise<void> {
  return invoke<void>(COMMANDS.mcpSetLaunchTrust, {
    fingerprint,
    decision,
  } satisfies McpSetLaunchTrustArgs);
}

/** §3.3 #47：连接池状态快照（管理页服务器行：名称/状态/工具数）。 */
export function mcpSnapshot(): Promise<McpSnapshotResult> {
  return invoke<McpSnapshotResult>(COMMANDS.mcpSnapshot, {});
}

// ---- 技能系统（§3.3 #48–51，09 号票）----

/** §3.3 #48：技能清单（三来源合并视图，同名覆盖后唯一 + 启停标注）。
 *  后端按当前活跃会话锚定项目作用域（设置页无会话参数的收敛点，08 号票
 *  同款）。 */
export function skillList(): Promise<SkillListEntry[]> {
  return invoke<SkillListEntry[]>(COMMANDS.skillList, {});
}

/** §3.3 #49：启停切换（FR-14-04「禁用技能」，落 skills-state.json 原子写，
 *  跨重启保留）。名字不存在 reject E_NOT_FOUND。 */
export function skillToggle(name: string, enabled: boolean): Promise<void> {
  const args: SkillToggleArgs = { name, enabled };
  return invoke<void>(COMMANDS.skillToggle, { name: args.name, enabled: args.enabled });
}

/** §3.3 #50：重扫全部来源并重建索引。活跃 run 的技能快照不受影响
 *  （Q20 版本固定），只更新管理页与下一 run。 */
export function skillReload(): Promise<void> {
  return invoke<void>(COMMANDS.skillReload, {});
}

/** §3.3 #51：正文预览（SKILL.md 剥离 frontmatter 后的 Markdown；只读，
 *  不走工具、不进上下文）。名字不存在 reject E_NOT_FOUND。 */
export function skillInspect(args: SkillInspectArgs): Promise<SkillInspectResult> {
  return invoke<SkillInspectResult>(COMMANDS.skillInspect, {
    name: args.name,
  } satisfies SkillInspectArgs);
}

// ---- 子代理（§3.3 #58/#59，10 号票）----

/** §3.3 #58：级联取消一个活跃子代理（令牌置位即返回；终态以该子代理的
 *  runEnded + subDone 帧为权威）。id 不存在或已结束 reject E_NOT_FOUND。 */
export function stopSubagent(args: StopSubagentArgs): Promise<void> {
  return invoke<void>(COMMANDS.stopSubagent, { subId: args.subId } satisfies StopSubagentArgs);
}

/** §3.3 #59：子代理历史回看（抽屉面）。返回该子代理的 WireMessage 序列
 *  （assistant 汇报与工具调用，与主会话消息格式同形）；不存在 reject
 *  E_NOT_FOUND。 */
export function loadSubagentHistory(args: LoadSubagentHistoryArgs): Promise<MessageEntry[]> {
  return invoke<MessageEntry[]>(COMMANDS.loadSubagentHistory, {
    sessionId: args.sessionId,
    subId: args.subId,
  } satisfies LoadSubagentHistoryArgs);
}

// ---- 网络搜索连通测试（§3.3 #59f，13 号票）----

/** §3.3 #59f：通道连通测试（草稿试连，不经保存直接用表单当前值探测）。
 *  target="searxng" 走本地回环直连（不经代理）；target="tavily" 走 Bearer
 *  Key 试连（消耗 1 credit）。失败不 reject（ok=false + error 带引导文案），
 *  仅参数错误（未知 target）reject E_INVALID_ARG。 */
export function searchTest(args: SearchTestArgs): Promise<SearchTestResult> {
  // Rust 命令签名为单结构体参数 `args: SearchTestArgs`（13 号票），invoke
  // 须按参数名包一层（14 号票修正：原平铺形状真实运行会报 missing key）。
  return invoke<SearchTestResult>(COMMANDS.searchTest, {
    args,
  } satisfies { args: SearchTestArgs });
}

/**
 * 注册 HTML 沙箱预览内容（14 号票，§3.3 #59g）：把 HTML 全文交给后端进程级
 * 注册表（token 寻址，不落盘），返回 iframe 直接可用的预览协议地址。幂等：
 * 同内容返回同地址。历史卡片重新展开时按参数原文重注册即可。
 */
export function registerPreview(html: string): Promise<PreviewRegisterResult> {
  return invoke<PreviewRegisterResult>(COMMANDS.previewRegister, {
    args: { html },
  } satisfies { args: { html: string } });
}
