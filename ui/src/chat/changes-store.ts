/**
 * 变更面板 store：变更面板
 * 的开合态与定位文件的共享状态，Inspector「变更」tab 与产物面板
 * （约定接口 openChangesPanel）的联动接缝。
 *
 * 约定接口（产物面板）：`openChangesPanel(sessionId, path?)`——
 * 产物页路径条目调用它打开面板并定位该文件的 diff；path 缺省只打开面板。
 */

import { create } from "zustand";

/** 变更面板状态：开合 + 归属会话 + 定位目标文件。 */
interface ChangesState {
  /** 面板是否打开（全局单例——同一时刻一个变更面板）。 */
  open: boolean;
  /** 打开面板时的会话 id（面板数据按它拉取 git 状态）。 */
  sessionId: string;
  /** 定位目标文件（面板打开后滚动/选中该文件的 diff；null = 不定位）。 */
  focusPath: string | null;
}

interface ChangesStore extends ChangesState {
  /** 打开变更面板（path? 定位该文件的 diff；不传只打开）。 */
  openPanel: (sessionId: string, path?: string) => void;
  /** 关闭面板（清定位目标）。 */
  closePanel: () => void;
}

/** 与产物面板的联动入口：产物页与 Inspector 变更 tab 共用。 */
export const useChangesStore = create<ChangesStore>((set) => ({
  open: false,
  sessionId: "",
  focusPath: null,
  openPanel: (sessionId, path) => set({ open: true, sessionId, focusPath: path ?? null }),
  closePanel: () => set({ open: false, focusPath: null }),
}));

/** 命令式联动入口（产物页路径条目的调用面；非组件上下文可用）。 */
export function openChangesPanel(sessionId: string, path?: string): void {
  useChangesStore.getState().openPanel(sessionId, path);
}
