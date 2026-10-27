/**
 * IPC 客户端唯一出口（§2.4 ipc/client.ts）。
 *
 * 命令名字符串集中在本文件的 `COMMANDS` 注册表，供 §12.3 manifest 双向扫描
 * （ui/src/ipc/contract.test.ts 对照 cargo test 产出的 ipc-manifest.json）；
 * 其余代码一律经由本文件的函数发起 IPC，禁止散落携带命令名字面量的 invoke 调用。
 */

import { invoke } from "@tauri-apps/api/core";
import type {
  AppConfig,
  DeleteProviderArgs,
  IpcError,
  ProviderConfig,
  SaveConfigArgs,
  SaveProviderArgs,
  SaveSessionTreeArgs,
  SaveSessionTreeResult,
  SessionTreeData,
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
} as const;

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
