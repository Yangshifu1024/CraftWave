/**
 * IPC 客户端唯一出口（§2.4 ipc/client.ts）。
 *
 * invoke 命令名字符串集中在此，供 §12.3 manifest 双向扫描
 * （ui/src/ipc/contract.test.ts 解析本文件源码，逐个对齐 ipc-manifest.json）。
 */

import { invoke } from "@tauri-apps/api/core";
import type { SaveSessionTreeArgs, SaveSessionTreeResult, SessionTreeData } from "./types";

/** 全树同步（§6.3）：thread.export() 全树 → Rust 校验后仅追加缺失节点，返回新 revision。 */
export function saveSessionTree(args: SaveSessionTreeArgs): Promise<SaveSessionTreeResult> {
  return invoke("save_session_tree", {
    sessionId: args.sessionId,
    messages: args.messages,
    activeLeafId: args.activeLeafId,
    baseRevision: args.baseRevision,
  });
}

/** 会话树加载（承载 §6.1 定稿结构：messages 含 status? + activeLeafId + revision）。 */
export function loadSessionTree(sessionId: string): Promise<SessionTreeData> {
  return invoke("load_session_tree", { sessionId });
}
