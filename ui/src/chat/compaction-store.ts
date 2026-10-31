/**
 * 上下文压缩状态 store（FR-03-05，04 号票，zustand 按会话分桶）。
 *
 * 两类状态：
 * - compactions：压缩边界投影（§6.3 定稿：compaction 条目不进消息树，
 *   load_session_tree 的 compactions 投影面在此落地）——时间线顶部分隔
 *   提示（"已压缩早期上下文"）的渲染源；
 * - phase：压缩生命周期（事件桥三键驱动）——compacting=Composer 上方
 *   指示条"正在压缩上下文…"；compact_failed 到达即清 phase（29 号票收敛：
 *   失败反馈改走顶部 Toast，行内常驻提示撤除，04 号票挂账清偿）。
 */

import { create } from "zustand";
import type { StoredCompactionEntry } from "@/ipc/types";

/** 单会话的压缩状态桶。 */
export interface SessionCompaction {
  /** 历史压缩边界投影（追加序；最近一条在末位）。 */
  compactions: StoredCompactionEntry[];
  /** 压缩进行态（null = 空闲；失败不再有常驻态——反馈走 Toast）。 */
  phase: null | "compacting";
}

/** store 状态与方法面。 */
export interface CompactionStore {
  /** 按会话分桶。 */
  bySession: Record<string, SessionCompaction>;
  /** 历史收养 / run:compacted 收口后写入压缩边界投影。 */
  setCompactions: (sessionId: string, compactions: StoredCompactionEntry[]) => void;
  /** 事件桥三键写 phase。 */
  setPhase: (sessionId: string, phase: SessionCompaction["phase"]) => void;
  /** 会话删除后清桶。 */
  forgetSession: (sessionId: string) => void;
}

/** 空桶（模块级常量：selector 缺桶返回稳定引用，防渲染循环）。 */
const EMPTY: SessionCompaction = { compactions: [], phase: null };

export const useCompactionStore = create<CompactionStore>((set) => ({
  bySession: {},
  setCompactions: (sessionId, compactions) =>
    set((s) => ({
      bySession: {
        ...s.bySession,
        [sessionId]: { ...EMPTY, ...s.bySession[sessionId], compactions },
      },
    })),
  setPhase: (sessionId, phase) =>
    set((s) => ({
      bySession: {
        ...s.bySession,
        [sessionId]: { ...EMPTY, ...s.bySession[sessionId], phase },
      },
    })),
  forgetSession: (sessionId) =>
    set((s) => {
      if (!(sessionId in s.bySession)) return s;
      const bySession = { ...s.bySession };
      delete bySession[sessionId];
      return { bySession };
    }),
}));

/** 响应式取单会话桶（无记录返回零值快照）。 */
export function useSessionCompaction(sessionId: string): SessionCompaction {
  return useCompactionStore((s) => s.bySession[sessionId]) ?? EMPTY;
}
