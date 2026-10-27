/**
 * 唯一 invoke 出口（§2.4 ipc/client.ts）。
 *
 * 命令名字符串集中在本文件的 `COMMANDS` 注册表，供 ui/src/ipc/contract.test.ts
 * 与 src-tauri/target/ipc-manifest.json 做 §12.3 双向校验；其余代码一律经由
 * 本文件的函数发起 IPC，禁止散落携带命令名字面量的 invoke 调用。
 */

import { invoke } from "@tauri-apps/api/core";
import type { AppConfig, IpcErrorShape, ProviderConfig } from "./types";

/** 本文件已接线的命令名（§3.3 键面，snake_case）。 */
export const COMMANDS = {
  getConfig: "get_config",
  saveConfig: "save_config",
  saveProvider: "save_provider",
  deleteProvider: "delete_provider",
} as const;

/** §3.3 #6：读取当前配置。 */
export async function getConfig(): Promise<AppConfig> {
  return invoke<AppConfig>(COMMANDS.getConfig);
}

/** §3.3 #7：整份配置经协调器归一化原子落盘，返回归一化结果。 */
export async function saveConfig(config: AppConfig): Promise<AppConfig> {
  return invoke<AppConfig>(COMMANDS.saveConfig, { config });
}

/** §3.3 #8：保存供应商；newKey 有值时走 keyring 提交协议（§9.4）。 */
export async function saveProvider(
  provider: ProviderConfig,
  newKey?: string,
): Promise<ProviderConfig> {
  return invoke<ProviderConfig>(COMMANDS.saveProvider, { provider, newKey });
}

/** §3.3 #9：删除供应商并同步删除 keyring 账户。 */
export async function deleteProvider(providerId: string): Promise<void> {
  return invoke<void>(COMMANDS.deleteProvider, { providerId });
}

/** 把 invoke 的 reject 归一为 §3.1 错误负载（Rust 侧已按 {code,message,detail} 序列化）。 */
export function toIpcError(err: unknown): IpcErrorShape {
  if (
    typeof err === "object" &&
    err !== null &&
    "code" in err &&
    typeof (err as { code: unknown }).code === "string"
  ) {
    return err as IpcErrorShape;
  }
  return { code: "E_INTERNAL", message: String(err) };
}
