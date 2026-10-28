/**
 * 会话计划状态（B 组：Inspector 计划页数据面，参考样式改版追加）：
 * 订阅 plan:update 事件（plan 工具写、§3.5）按会话维护最近一份计划条目。
 * 内存态（重启清空）——历史恢复无计划数据面（plan 边车持久化随 M2）。
 */

import { create } from "zustand";
import type { PlanItem } from "@/ipc/types";

export type { PlanItem };

export interface PlanStateStore {
  /** sessionId → 最近一份计划条目（空数组 = 无计划）。 */
  bySession: Record<string, PlanItem[]>;
  /** plan:update 事件入口（整份替换）。 */
  applyUpdate: (sessionId: string, items: PlanItem[]) => void;
  /** 会话删除后清桶。 */
  forgetSession: (sessionId: string) => void;
}

export const usePlanStateStore = create<PlanStateStore>((set) => ({
  bySession: {},

  applyUpdate: (sessionId, items) => {
    set((s) => ({ bySession: { ...s.bySession, [sessionId]: items } }));
  },

  forgetSession: (sessionId) =>
    set((s) => {
      if (!(sessionId in s.bySession)) return s;
      const bySession = { ...s.bySession };
      delete bySession[sessionId];
      return { bySession };
    }),
}));
