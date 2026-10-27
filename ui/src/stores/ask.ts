/**
 * ask / askHint store（§10.1 状态分界：ask/审批 zustand `ask`（事件驱动）+
 * 建议提示词 `askHint`（run:suggestions 驱动））。
 *
 * 事件桥（App.tsx 的 useEventBridge）把 §3.5 五键事件分发到本 store 与
 * runMetrics；命令应答走 ipc/client.resolveAsk。
 */

import { create } from "zustand";
import type { AskPayload } from "@/ipc/types";

/** 当前未决询问/审批（AskPanel 渲染源）。 */
export interface AskPanelState {
  askId: string;
  payload: AskPayload;
}

export interface AskStore {
  /** 当前未决 ask（null = 无，AskPanel 收起）。 */
  current: AskPanelState | null;
  /** 建议提示词（run:suggestions；§10.4 ghost 承载，接受后清除）。 */
  hint: string | null;
  open: (askId: string, payload: AskPayload) => void;
  close: (askId: string) => void;
  setHint: (text: string) => void;
  clearHint: () => void;
}

export const useAskStore = create<AskStore>((set, get) => ({
  current: null,
  hint: null,

  open: (askId, payload) => {
    // 同一时刻至多一个未决 ask（后端串行保证）；后到覆盖防御
    set({ current: { askId, payload } });
  },

  close: (askId) => {
    if (get().current?.askId === askId) {
      set({ current: null });
    }
  },

  setHint: (text) => set({ hint: text }),

  clearHint: () => set({ hint: null }),
}));

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
