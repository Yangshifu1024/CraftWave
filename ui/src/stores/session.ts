/**
 * 会话持久化的前端侧（15 号票，§6.3/§6.4 双写协议的发送方）。
 *
 * 双写协议：同一棵消息树由两端共同落盘——前端负责 save 用户操作造成的
 * 拓扑变化（编辑、分支切换），Rust 侧 RunRecorder 按帧落盘 run 产物
 * （canonical，即权威记录）。两边通过 revision（版本号，乐观并发）对齐：
 * save 必须带上次拿到的版本，版本不符会被拒绝（E_SESSION_REV）。
 *
 * 协议要点（票 15 Answer 详述）：
 * - **canonical 条目 Rust 权威**：run 产物由 Rust RunRecorder 按帧落盘
 *   （turn 级条目，id 规则 `{runId}:{turn}` / `{runId}:{turn}.tools` /
 *   `{runId}:marker`）；前端树上本次运行的运行态节点（assistant-ui 单 run
 *   单消息，id 由 runtime 临时分配）不进 save 载荷——剔除并把引用改写为
 *   `{runId}:marker`（canonical 链尾）。
 * - **发送流程**：adapter run() 开头先 save（活动路径线性链 + 新 user 拓扑
 *   正确落盘）再 start_chat；分支操作（编辑/切换）的拓扑也在下一次
 *   save 自然同步。
 * - **E_SESSION_REV 自愈**：canonical 落盘推进了 revision 而本地基线过期时，
 *   load 拿新 revision 重试一次（§6.4 重载重试）。
 */

import type { ThreadMessage } from "@assistant-ui/react";
import { create } from "zustand";
import { toWire } from "@/chat/runtime";
import * as client from "@/ipc/client";
import type { MessageEntry, SessionTreeData } from "@/ipc/types";
import { useSessionsStore } from "./sessions";

/** 每会话一份的运行态记账（16 号票多会话分桶）。 */
export interface SessionPersistScope {
  /** 乐观并发基线（save_session_tree.baseRevision；0 = 尚未同步过）。 */
  revision: number;
  /** 本 app 生命周期内 adapter 产出的 assistant 消息（不进 save 载荷）。 */
  runtimeNodes: Map<string, string>;
}

/** 会话持久化状态仓库的形状：按 sessionId 分桶的记账（16 号票多会话）。 */
export interface SessionPersistState {
  /** sessionId → 记账（缺省 = { revision: 0, 空 Map }）。 */
  scopes: Record<string, SessionPersistScope>;
  /** save 成功后更新该会话的版本基线（下次 save 携带）。 */
  setRevision: (sessionId: string, revision: number) => void;
  /** 登记一个运行态消息（messageId → runId），save 时据此剔除并改写引用。 */
  noteRuntimeNode: (sessionId: string, messageId: string, runId: string) => void;
  /** 会话关闭/删除时丢弃其记账。 */
  forgetScope: (sessionId: string) => void;
}

/** 空记账：revision 0 表示从未同步过，runtimeNodes 为空表示没有运行态节点。 */
const emptyScope = (): SessionPersistScope => ({
  revision: 0,
  runtimeNodes: new Map(),
});

/** 会话持久化状态仓库实例（zustand store；用法见 stores/ask.ts 文件头说明）。 */
export const useSessionPersistStore = create<SessionPersistState>((set) => ({
  scopes: {},
  setRevision: (sessionId, revision) =>
    set((s) => {
      const scope = s.scopes[sessionId] ?? emptyScope();
      return { scopes: { ...s.scopes, [sessionId]: { ...scope, revision } } };
    }),
  noteRuntimeNode: (sessionId, messageId, runId) =>
    set((s) => {
      const scope = s.scopes[sessionId] ?? emptyScope();
      const runtimeNodes = new Map(scope.runtimeNodes).set(messageId, runId);
      return { scopes: { ...s.scopes, [sessionId]: { ...scope, runtimeNodes } } };
    }),
  forgetScope: (sessionId) =>
    set((s) => {
      if (!(sessionId in s.scopes)) return s;
      const scopes = { ...s.scopes };
      delete scopes[sessionId];
      return { scopes };
    }),
}));

/** 取会话记账（只读快照；store 外使用）。 */
export function getPersistScope(
  state: SessionPersistState,
  sessionId: string,
): SessionPersistScope {
  return state.scopes[sessionId] ?? emptyScope();
}

/** 运行态消息在 canonical 落盘里的等价 id：同一 run 的链尾标记
 *  （{runId}:marker）——树上引用运行态节点的地方，save 时改指向它。 */
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
 *
 * 首次可见刷新：save 成功推进索引 messageCount 后，若该会话此前不在左栏
 * 列表（新建未发送的空会话），补一次 refresh 让它即时出现（FR-01-01：
 * 会话发送后才进列表）。
 */
export async function syncTreeBeforeSend(
  sessionId: string,
  messages: readonly ThreadMessage[],
): Promise<void> {
  const store = useSessionPersistStore.getState();
  const { revision, runtimeNodes } = getPersistScope(store, sessionId);
  const { messages: payload, activeLeafId } = toSavePayload(messages, runtimeNodes);
  if (payload.length === 0) return;
  const attempt = async (baseRevision: number) =>
    client.saveSessionTree({ sessionId, messages: payload, activeLeafId, baseRevision });
  const refreshIfFirstVisible = () => {
    const known = useSessionsStore.getState().sessions.some((m) => m.id === sessionId);
    if (known) return;
    void useSessionsStore
      .getState()
      .refresh()
      .catch((err) => {
        console.warn("[sessions] 发送后刷新列表失败（下次操作自愈）", err);
      });
  };
  try {
    const result = await attempt(revision);
    useSessionPersistStore.getState().setRevision(sessionId, result.revision);
    refreshIfFirstVisible();
  } catch (err) {
    const ipcErr = client.toIpcError(err);
    if (ipcErr.code !== "E_SESSION_REV") throw err;
    // canonical 已推进（run 落盘/挂起恢复）：重载基线重试一次（§6.4）
    const tree = await client.loadSessionTree(sessionId);
    useSessionPersistStore.getState().setRevision(sessionId, tree.revision);
    const result = await attempt(tree.revision);
    useSessionPersistStore.getState().setRevision(sessionId, result.revision);
    refreshIfFirstVisible();
  }
}

/** 历史收养（启动/会话进入）：load → canonical 树（返回给调用方 import）。 */
export async function loadHistory(sessionId: string): Promise<SessionTreeData | null> {
  try {
    const tree = await client.loadSessionTree(sessionId);
    useSessionPersistStore.getState().setRevision(sessionId, tree.revision);
    return tree;
  } catch (err) {
    const ipcErr = client.toIpcError(err);
    if (ipcErr.code === "E_NOT_FOUND") return null; // 新会话
    throw err;
  }
}
