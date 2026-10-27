/**
 * IPC 客户端唯一出口（§2.4 ipc/client.ts）。
 *
 * 命令名字符串集中在本文件的 `COMMANDS` 注册表，供 §12.3 manifest 双向扫描
 * （ui/src/ipc/contract.test.ts 对照 cargo test 产出的 ipc-manifest.json）；
 * 其余代码一律经由本文件的函数发起 IPC，禁止散落携带命令名字面量的 invoke 调用。
 */

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AppConfig,
  AskAnswer,
  ChatFrame,
  DeleteProviderArgs,
  IpcError,
  ProviderConfig,
  SaveConfigArgs,
  SaveProviderArgs,
  SaveSessionTreeArgs,
  SaveSessionTreeResult,
  SessionTreeData,
  StartChatArgs,
  StartChatResult,
} from "./types";

/** 本文件已接线的命令名（§3.3 键面，snake_case）。 */
export const COMMANDS = {
  activateCustomTitlebar: "activate_custom_titlebar",
  restoreNativeTitlebar: "restore_native_titlebar",
  saveSessionTree: "save_session_tree",
  loadSessionTree: "load_session_tree",
  getConfig: "get_config",
  saveConfig: "save_config",
  saveProvider: "save_provider",
  deleteProvider: "delete_provider",
  startChat: "start_chat",
  cancelRun: "cancel_run",
  resolveAsk: "resolve_ask",
  approvePlan: "approve_plan",
} as const;

/** 本文件已接线的事件键（§3.5，14 号票实装 5 键）。 */
export const EVENT_KEYS = {
  askOpened: "ask:opened",
  askClosed: "ask:closed",
  planUpdate: "plan:update",
  runSuggestions: "run:suggestions",
  tokensUpdate: "tokens:update",
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

/** 看门狗超时后的退化路径：恢复系统标题栏。 */
export function restoreNativeTitlebar(): Promise<void> {
  return invoke<void>(COMMANDS.restoreNativeTitlebar);
}

/** 全树同步（§6.3）：thread.export() 全树 → Rust 校验后仅追加缺失节点，返回新 revision。 */
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

/** §3.3 #18：启动一次 run。channel 由调用方建立（onmessage 收 ChatFrame 帧流）。 */
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

/** §3.3 #26c：冻结当前计划为批准快照（计划门 §8.6，plan 卡批准按钮）。 */
export function approvePlan(sessionId: string): Promise<void> {
  return invoke<void>(COMMANDS.approvePlan, { sessionId } satisfies { sessionId: string });
}

/** 把 invoke 的 reject 归一为 §3.1 错误负载（Rust 侧已按 {code,message,detail} 序列化）。 */
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
