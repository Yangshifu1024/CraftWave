/**
 * 会话计划状态（Inspector 计划页数据面，全量计划态——基线标记与待批准
 * 范围展示）：订阅 plan:update 事件
 * （plan 工具写、approve_plan 冻结后同键补发）按会话维护最近一份
 * 全量计划态。内存态（重启清空）——历史恢复无计划数据面（plan 边车持久
 * 化随 M2）。
 */

import { create } from "zustand";
import type { PlanItem } from "@/ipc/types";

export type { PlanItem };

/** 单会话全量计划态（plan:update 载荷的同构快照）。 */
export interface SessionPlanState {
  /** 计划条目（空数组 = 无计划）。 */
  items: PlanItem[];
  /** 模型声明的写范围（具体文件/目录；批准时冻结为基线）。 */
  paths: string[];
  /** 是否已批准（批准快照存在）。 */
  approved: boolean;
  /** 用户批准时冻结的文件/目录范围（范围门判定基线的展示面）。 */
  baselinePaths: string[];
}

/** 空计划态（selector 缺桶时的稳定引用）。 */
export const EMPTY_PLAN_STATE: SessionPlanState = {
  items: [],
  paths: [],
  approved: false,
  baselinePaths: [],
};

export interface PlanStateStore {
  /** sessionId → 最近一份全量计划态。 */
  bySession: Record<string, SessionPlanState>;
  /** plan:update 事件入口（整份替换）。 */
  applyUpdate: (
    sessionId: string,
    state: {
      items: PlanItem[];
      paths: string[];
      approved: boolean;
      baselinePaths: string[];
    },
  ) => void;
  /** 会话删除后清桶。 */
  forgetSession: (sessionId: string) => void;
}

export const usePlanStateStore = create<PlanStateStore>((set) => ({
  bySession: {},

  applyUpdate: (sessionId, state) => {
    set((s) => ({
      bySession: { ...s.bySession, [sessionId]: { ...state } },
    }));
  },

  forgetSession: (sessionId) =>
    set((s) => {
      if (!(sessionId in s.bySession)) return s;
      const bySession = { ...s.bySession };
      delete bySession[sessionId];
      return { bySession };
    }),
}));
