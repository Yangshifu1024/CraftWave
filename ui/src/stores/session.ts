/**
 * 会话持久化前端侧（15 号票，§6.3/§6.4 双写协议的发送方）。
 *
 * 协议要点（票 15 Answer 详述）：
 * - **canonical 条目 Rust 权威**：run 产物由 Rust RunRecorder 按帧落盘
 *   （turn 级条目，id 规则 `{runId}:{turn}` / `{runId}:{turn}.tools` /
 *   `{runId}:marker`）；前端树上的运行态节点（assistant-ui 单 run 单消息，
 *   id 由 runtime 分配）**不进 save 载荷**——剔除并把引用改写为
 *   `{runId}:marker`（canonical 链尾）。
 * - **发送流程**：adapter run() 开头先 save（活动路径线性链 + 新 user 拓扑
 *   正确落盘）再 start_chat；分支操作（编辑/重生成/切换）的拓扑也在下一次
 *   save 自然同步。
 * - **E_SESSION_REV 自愈**：canonical 落盘推进了 revision 而 baseRevision
 *   过期时，load 拿新 revision 重试一次（§6.4 重载重试）。
 */

import type { ThreadMessage } from "@assistant-ui/react";
import { create } from "zustand";
import { toWire } from "@/chat/runtime";
import * as client from "@/ipc/client";
import type { MessageEntry, SessionTreeData } from "@/ipc/types";

/** 运行态节点记账：adapter 产出（messageId → runId）。 */
export interface SessionPersistState {
  /** 乐观并发基线（save_session_tree.baseRevision；0 = 尚未同步过）。 */
  revision: number;
  /** 本 app 生命周期内 adapter 产出的 assistant 消息（不进 save 载荷）。 */
  runtimeNodes: Map<string, string>;
  setRevision: (revision: number) => void;
  noteRuntimeNode: (messageId: string, runId: string) => void;
}

export const useSessionPersistStore = create<SessionPersistState>((set) => ({
  revision: 0,
  runtimeNodes: new Map(),
  setRevision: (revision) => set({ revision }),
  noteRuntimeNode: (messageId, runId) =>
    set((s) => ({ runtimeNodes: new Map(s.runtimeNodes).set(messageId, runId) })),
}));

/** 运行态节点的 canonical 等价 id（链尾 marker）。 */
function canonicalEquiv(messageId: string, runtimeNodes: Map<string, string>): string | null {
  const runId = runtimeNodes.get(messageId);
  return runId ? `${runId}:marker` : null;
}

/**
 * 活动路径（adapter messages 线性序）→ save 载荷：
 * - 运行态节点剔除（canonical 已有 run 产物）；
 * - 剔除后子节点的 parentId、以及末条节点派生的 activeLeafId 改写为
 *   `{runId}:marker`；其余按线性序 parent=前一条。
 */
export function toSavePayload(
  messages: readonly ThreadMessage[],
  runtimeNodes: Map<string, string>,
): { messages: MessageEntry[]; activeLeafId?: string } {
  const wire = toWire(messages);
  const items = messages.map((m, i) => ({ message: m, entry: wire[i] }));
  const out: MessageEntry[] = [];
  let prevId: string | null = null;
  for (const { message, entry } of items) {
    if (runtimeNodes.has(message.id)) {
      // 运行态节点不进载荷，但其 canonical 等价（runMarker 链尾）接管链指针，
      // 后续节点（如编辑/追问的 user）挂到 marker 下，与 Rust canonical 链连续
      prevId = canonicalEquiv(message.id, runtimeNodes);
      continue;
    }
    const e = { ...entry };
    delete (e as { parentId?: string }).parentId;
    if (prevId !== null) e.parentId = prevId;
    out.push(e);
    prevId = e.id;
  }
  // 末条若是被剔除的运行态节点（或路径尾），activeLeaf 指其 canonical 等价
  const last = messages.at(-1);
  const activeLeafId =
    last && runtimeNodes.has(last.id)
      ? (canonicalEquiv(last.id, runtimeNodes) ?? prevId ?? undefined)
      : (prevId ?? undefined);
  return { messages: out, activeLeafId };
}

/**
 * 发送前树同步（§6.3 双写的先手）：save 活动路径。E_SESSION_REV 时 load
 * 校正 revision 重试一次；再失败仅记 console（发送流程不因此中断——
 * Rust 侧 user 兜底会补落，分支拓扑延后到下次成功 save）。
 */
export async function syncTreeBeforeSend(
  sessionId: string,
  messages: readonly ThreadMessage[],
): Promise<void> {
  const persist = useSessionPersistStore.getState();
  const { messages: payload, activeLeafId } = toSavePayload(messages, persist.runtimeNodes);
  if (payload.length === 0) return;
  const attempt = async (baseRevision: number) =>
    client.saveSessionTree({ sessionId, messages: payload, activeLeafId, baseRevision });
  try {
    const result = await attempt(persist.revision);
    useSessionPersistStore.getState().setRevision(result.revision);
  } catch (err) {
    const ipcErr = client.toIpcError(err);
    if (ipcErr.code !== "E_SESSION_REV") throw err;
    // canonical 已推进（run 落盘/挂起恢复）：重载基线重试一次（§6.4）
    const tree = await client.loadSessionTree(sessionId);
    useSessionPersistStore.getState().setRevision(tree.revision);
    const result = await attempt(tree.revision);
    useSessionPersistStore.getState().setRevision(result.revision);
  }
}

/** 历史收养（启动/会话进入）：load → canonical 树（返回给调用方 import）。 */
export async function loadHistory(sessionId: string): Promise<SessionTreeData | null> {
  try {
    const tree = await client.loadSessionTree(sessionId);
    useSessionPersistStore.getState().setRevision(tree.revision);
    return tree;
  } catch (err) {
    const ipcErr = client.toIpcError(err);
    if (ipcErr.code === "E_NOT_FOUND") return null; // 新会话
    throw err;
  }
}
