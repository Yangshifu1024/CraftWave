/**
 * 询问与建议提示词的全局状态（§10.1 状态分界）。
 *
 * zustand：React 生态的轻量全局状态管理库；一个 store（状态仓库）就是一份
 * 任意组件都能读写的全局状态。本文件管两个桶：
 *  - ask：未决的询问/审批（后端事件驱动——模型需要用户确认时弹面板）；
 *  - askHint：建议提示词（run:suggestions 事件驱动——run 结束后给出的下一步建议）。
 *
 * 事件桥（chat/event-bridge.ts 的 useBackendEvents）把 §3.5 五键后端事件分发
 * 进本 store 与 runMetrics（用量记账）；用户的应答经 ipc/client.resolveAsk 送回后端。
 *
 * 多会话分桶（16 号票）：未决 ask 与 hint 都按 sessionId（会话唯一标识）分桶
 * ——即各会话各存一格、互不干扰；AskPanel 只渲染当前会话那一桶，多个 Tab
 * 并行运行时审批互不遮挡。
 */

import { create } from "zustand";
import type { AskPayload } from "@/ipc/types";

/** 当前未决询问/审批（AskPanel 渲染源）。 */
export interface AskPanelState {
  askId: string;
  payload: AskPayload;
}

/** ask 状态仓库的形状（方法说明见各字段行内注释）。 */
export interface AskStore {
  /** sessionId → 未决 ask（AskPanel 渲染本会话桶）。 */
  bySession: Record<string, AskPanelState>;
  /** sessionId → 建议提示词（run:suggestions；§10.4 ghost 承载，接受后清除）。 */
  hints: Record<string, string>;
  /** 收到 ask:opened 事件时登记一个未决询问（后开覆盖先开）。 */
  open: (sessionId: string, askId: string, payload: AskPayload) => void;
  /** 收到 ask:closed 事件时按 askId 精确清除（askId 不匹配则不动，防误伤新询问）。 */
  close: (sessionId: string, askId: string) => void;
  /** 写入本会话的建议提示词（run:suggestions）。 */
  setHint: (sessionId: string, text: string) => void;
  /** 清除本会话的建议提示词（用户已接受或会话切换时）。 */
  clearHint: (sessionId: string) => void;
  /** 会话被删除时清掉它的全部痕迹（ask 与 hint 两桶）。 */
  forgetSession: (sessionId: string) => void;
}

/** ask 状态仓库实例：组件用 useAskStore(selector) 订阅渲染，
 *  事件桥等非 React 场景用 useAskStore.getState().xxx() 命令式读写。 */
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
 * ghost 提示匹配（§10.4）：ghost 指输入框里以灰色预填的建议文本。
 * 输入为空时整句可用；已输入内容恰是建议的前缀时，返回建议的剩余尾部（补齐用）。
 * 返回 null = 无可用 ghost（不匹配，或用户已完整输入了建议本身）。
 */
export function matchGhost(hint: string | null, input: string): string | null {
  if (!hint) return null;
  if (input.length === 0) return hint;
  if (hint.startsWith(input) && input.length < hint.length) {
    return hint.slice(input.length);
  }
  return null;
}
