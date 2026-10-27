/**
 * ask / askHint store（§10.1 状态分界：ask/审批 zustand `ask`（事件驱动）+
 * 建议提示词 `askHint`（run:suggestions 驱动））。
 *
 * 事件桥（App.tsx 的 useBackendEvents）把 §3.5 五键事件分发到本 store 与
 * runMetrics；命令应答走 ipc/client.resolveAsk。
 *
 * 16 号票多会话：未决 ask 与 hint 按 sessionId 分桶——AskPanel 只渲染本会话
 * 桶（跨 Tab 并行 run 的审批互不遮挡）。
 */

import { create } from "zustand";
import type { AskPayload } from "@/ipc/types";

/** 当前未决询问/审批（AskPanel 渲染源）。 */
export interface AskPanelState {
  askId: string;
  payload: AskPayload;
}

export interface AskStore {
  /** sessionId → 未决 ask（AskPanel 渲染本会话桶）。 */
  bySession: Record<string, AskPanelState>;
  /** sessionId → 建议提示词（run:suggestions；§10.4 ghost 承载，接受后清除）。 */
  hints: Record<string, string>;
  open: (sessionId: string, askId: string, payload: AskPayload) => void;
  close: (sessionId: string, askId: string) => void;
  setHint: (sessionId: string, text: string) => void;
  clearHint: (sessionId: string) => void;
  forgetSession: (sessionId: string) => void;
}

export const useAskStore = create<AskStore>((set) => ({
  bySession: {},
  hints: {},

  open: (sessionId, askId, payload) => {
    // 同一会话同一时刻至多一个未决 ask（后端串行保证）；后到覆盖防御
    set((s) => ({ bySession: { ...s.bySession, [sessionId]: { askId, payload } } }));
  },

  close: (sessionId, askId) => {
    set((s) => {
      if (s.bySession[sessionId]?.askId !== askId) return s;
      const bySession = { ...s.bySession };
      delete bySession[sessionId];
      return { bySession };
    });
  },

  setHint: (sessionId, text) => set((s) => ({ hints: { ...s.hints, [sessionId]: text } })),

  clearHint: (sessionId) =>
    set((s) => {
      if (!(sessionId in s.hints)) return s;
      const hints = { ...s.hints };
      delete hints[sessionId];
      return { hints };
    }),

  forgetSession: (sessionId) =>
    set((s) => {
      if (!(sessionId in s.bySession) && !(sessionId in s.hints)) return s;
      const bySession = { ...s.bySession };
      const hints = { ...s.hints };
      delete bySession[sessionId];
      delete hints[sessionId];
      return { bySession, hints };
    }),
}));

/** 读本会话未决 ask（无则 null）。 */
export function currentAsk(state: AskStore, sessionId: string): AskPanelState | null {
  return state.bySession[sessionId] ?? null;
}

/** 读本会话 hint（无则 null）。 */
export function sessionHint(state: AskStore, sessionId: string): string | null {
  return state.hints[sessionId] ?? null;
}

/**
 * ghost 提示匹配（§10.4：输入为空时整句 / 前缀匹配时补尾部）。
 * 返回 null = 无可用 ghost（不匹配或已完整输入）。
 */
export function matchGhost(hint: string | null, input: string): string | null {
  if (!hint) return null;
  if (input.length === 0) return hint;
  if (hint.startsWith(input) && input.length < hint.length) {
    return hint.slice(input.length);
  }
  return null;
}
