/**
 * 会话权限档（§10.6 / FR-13-02，17 号票）：Shift+Tab 循环切换，档位仅
 * 内存态（重启回默认档 autoEdit，与 DEFAULT_RUN_CONFIG 一致；§10.6 的
 * confirm_each 初始口径随安全域票复核——票面记录）。
 *
 * start_chat 发送时经 sessionAdapter 的动态 runConfig 读取（runtime.ts）。
 */

import { create } from "zustand";
import type { PermissionTier } from "@/ipc/types";

/** 循环顺序（§8.5 矩阵从严到宽）。 */
const TIERS: PermissionTier[] = ["plan", "confirmEach", "autoEdit", "fullAccess"];

/** 档位的用户可读名（Composer 档位指示）。 */
export const TIER_LABELS: Record<PermissionTier, string> = {
  plan: "计划",
  confirmEach: "逐项确认",
  autoEdit: "自动编辑",
  fullAccess: "完全信任",
};

export interface PermissionTierStore {
  bySession: Record<string, PermissionTier>;
  /** 循环切换并返回新档位（Shift+Tab）。 */
  cycle: (sessionId: string) => PermissionTier;
}

export const usePermissionTierStore = create<PermissionTierStore>((set, get) => ({
  bySession: {},

  cycle: (sessionId) => {
    const current = get().bySession[sessionId] ?? "autoEdit";
    const next = TIERS[(TIERS.indexOf(current) + 1) % TIERS.length];
    set((s) => ({ bySession: { ...s.bySession, [sessionId]: next } }));
    return next;
  },
}));

/** 读会话当前档位（无记录 = 默认 autoEdit）。 */
export function tierOf(sessionId: string): PermissionTier {
  return usePermissionTierStore.getState().bySession[sessionId] ?? "autoEdit";
}
