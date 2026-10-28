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
  ChatFrame,
  DeleteProviderArgs,
  IpcError,
  NotificationPermissionStatus,
  ProjectMeta,
  ProviderConfig,
  ReadWorkspaceResult,
  SaveConfigArgs,
  SaveProjectArgs,
  SaveProviderArgs,
  SaveSessionTreeArgs,
  SaveSessionTreeResult,
  SessionMeta,
  SessionTreeData,
  StartChatArgs,
  StartChatResult,
  ToolOutcomeJson,
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
  startChat: "start_chat",
  cancelRun: "cancel_run",
  resolveAsk: "resolve_ask",
  approvePlan: "approve_plan",
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
} as const;

/**
 * 本文件已接线的事件键注册表（§3.5：14 号票 5 键 + 17 号票 5 键）。
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
  notifyActivate: "notify:activate",
  appExitRequested: "app:exit_requested",
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
): Promise<Record<string, ToolOutcomeJson>> {
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

/** §3.3 #18：启动一次 run（一轮模型对话）。channel 由调用方建立：Channel 是
 *  Tauri 提供的流式通道——后端把逐帧产出（文本增量、工具调用等 ChatFrame）
 *  连续推给 onmessage 回调，Promise 本身只在 run 结束时兑现。 */
export function startChat(args: {
  sessionId: string;
  messages: StartChatArgs["messages"];
  runConfig: StartChatArgs["runConfig"];
  onFrame: (frame: ChatFrame) => void;
}): Promise<StartChatResult> {
  const channel = new Channel<ChatFrame>();
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
