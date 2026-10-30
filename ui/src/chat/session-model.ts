/**
 * 会话模型选择（FR-13-05 Ctrl+L，18 号票完成持久化收尾）：每会话一份
 * 内存态覆盖——发送时经 sessionAdapter 的惰性 runConfig 带上 `modelId`，
 * Rust 侧按 §11.1 优先级解析（RunConfig.modelId > 全局 activeModelId）。
 *
 * 18 号票追加：选择即经 session-prefs 的防抖落盘（set_session_prefs，
 * FR-02-07/FR-14-05——跨重启还原）；Ctrl+L 唤起的打开信号也收在本模块
 * （openFor 单字段 store，ModelPicker 订阅自己的会话 id）。
 */

import { create } from "zustand";
import { persistSessionPrefs } from "./session-prefs";

export interface SessionModelStore {
  /** sessionId → 选中的模型 id（无记录 = 跟随全局 activeModelId）。 */
  bySession: Record<string, string>;
  /** 设置本会话的模型（undefined = 清除覆盖，回到全局默认）。仅内存态；
   *  持久化入口见 setModelPersisted（恢复链路走本方法避免写回环）。 */
  setModel: (sessionId: string, modelId: string | undefined) => void;
  /** Ctrl+L 打开信号的承载（sessionId → 请求打开；null = 无）。 */
  openRequest: string | null;
  /** 请求打开某会话的模型选择（Ctrl+L 监听派发）。 */
  requestOpen: (sessionId: string) => void;
  /** ModelPicker 响应后消费掉打开信号。 */
  consumeOpen: (sessionId: string) => void;
}

export const useSessionModelStore = create<SessionModelStore>((set) => ({
  bySession: {},
  openRequest: null,

  setModel: (sessionId, modelId) => {
    set((s) => {
      const bySession = { ...s.bySession };
      if (modelId === undefined) delete bySession[sessionId];
      else bySession[sessionId] = modelId;
      return { bySession };
    });
  },

  requestOpen: (sessionId) => set({ openRequest: sessionId }),
  consumeOpen: (sessionId) => set((s) => (s.openRequest === sessionId ? { openRequest: null } : s)),
}));

/** 读会话当前覆盖的模型 id（无覆盖 = undefined，走全局默认）。 */
export function modelOf(sessionId: string): string | undefined {
  return useSessionModelStore.getState().bySession[sessionId] ?? undefined;
}

/** 选定会话模型并防抖落盘（Ctrl+L/下拉选择入口）：undefined = 清除覆盖
 *  （载荷 modelId: null，回全局默认）。恢复链路走 setModel 纯方法。 */
export function setModelPersisted(sessionId: string, modelId: string | undefined): void {
  useSessionModelStore.getState().setModel(sessionId, modelId);
  persistSessionPrefs(sessionId, {
    modelId: modelId ?? null,
  });
}
