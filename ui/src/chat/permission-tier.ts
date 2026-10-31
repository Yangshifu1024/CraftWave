/**
 * 会话权限档（§10.6 / FR-13-02，17 号票）：Shift+Tab 循环切换，档位仅
 * 内存态（重启回默认档 autoEdit，与 DEFAULT_RUN_CONFIG 一致；§10.6 的
 * confirm_each 初始口径随安全域票复核——票面记录）。
 *
 * 权限档指「本次运行中工具执行前要不要询问用户」的严格程度档位，
 * 从严到宽共四档：plan（只规划不动手）/ confirmEach（每个工具都确认）/
 * autoEdit（自动编辑默认档）/ fullAccess（完全信任）。
 * start_chat 发送时经 sessionAdapter 的动态 runConfig 读取（runtime.ts）。
 */

import { create } from "zustand";

import { t } from "@/i18n/core";
import type { PermissionTier } from "@/ipc/types";

/** 循环顺序（§8.5 矩阵从严到宽）。 */
const TIERS: PermissionTier[] = ["plan", "confirmEach", "autoEdit", "fullAccess"];

/** 档位的用户可读名（Composer 档位指示）：经字典查词（31 号票 i18n——
 *  模块级 t() 每次调用查当前语言，组件重渲染时取到切换后文案）。 */
export function tierLabel(tier: PermissionTier): string {
  switch (tier) {
    case "plan":
      return t("workspace.tierPlan");
    case "confirmEach":
      return t("workspace.tierConfirmEach");
    case "autoEdit":
      return t("workspace.tierAutoEdit");
    case "fullAccess":
      return t("workspace.tierFullAccess");
  }
}

/** 权限档 store 的状态与方法面。 */
export interface PermissionTierStore {
  /** 各会话当前权限档（sessionId → 档位；无记录 = 默认 autoEdit）。 */
  bySession: Record<string, PermissionTier>;
  /** 循环切换并返回新档位（Shift+Tab）。 */
  cycle: (sessionId: string) => PermissionTier;
  /** 直接选定档位（Composer 下拉）。 */
  setTier: (sessionId: string, tier: PermissionTier) => void;
}

/** zustand store：按会话分桶保存权限档（多 Tab 各自独立切换互不影响）。 */
export const usePermissionTierStore = create<PermissionTierStore>((set, get) => ({
  bySession: {},

  cycle: (sessionId) => {
    const current = get().bySession[sessionId] ?? "autoEdit";
    const next = TIERS[(TIERS.indexOf(current) + 1) % TIERS.length];
    set((s) => ({ bySession: { ...s.bySession, [sessionId]: next } }));
    return next;
  },

  setTier: (sessionId, tier) => {
    set((s) => ({ bySession: { ...s.bySession, [sessionId]: tier } }));
  },
}));

/** 读会话当前档位（无记录 = 默认 autoEdit）。 */
export function tierOf(sessionId: string): PermissionTier {
  return usePermissionTierStore.getState().bySession[sessionId] ?? "autoEdit";
}
