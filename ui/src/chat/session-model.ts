/**
 * 会话模型选择（FR-13-05 Ctrl+L 的 M1 简化：Composer 下拉，17 号后追加）：
 * 每会话一份内存态覆盖——发送时经 sessionAdapter 的惰性 runConfig 带上
 * `modelId`，Rust 侧按 §11.1 优先级解析（RunConfig.modelId > 全局
 * activeModelId）。不落盘（会话偏好持久化随 session_prefs 域后续票）。
 */

import { create } from "zustand";

export interface SessionModelStore {
  /** sessionId → 选中的模型 id（无记录 = 跟随全局 activeModelId）。 */
  bySession: Record<string, string>;
  /** 设置本会话的模型（undefined = 清除覆盖，回到全局默认）。 */
  setModel: (sessionId: string, modelId: string | undefined) => void;
}

export const useSessionModelStore = create<SessionModelStore>((set) => ({
  bySession: {},

  setModel: (sessionId, modelId) => {
    set((s) => {
      const bySession = { ...s.bySession };
      if (modelId === undefined) delete bySession[sessionId];
      else bySession[sessionId] = modelId;
      return { bySession };
    });
  },
}));

/** 读会话当前覆盖的模型 id（无覆盖 = undefined，走全局默认）。 */
export function modelOf(sessionId: string): string | undefined {
  return useSessionModelStore.getState().bySession[sessionId] ?? undefined;
}
