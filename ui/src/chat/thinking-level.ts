/**
 * 会话 thinking 档（FR-02-08，18 号票）：Composer 快捷钮按会话切换模型
 * 思考力度（off/minimal/low/medium/high 五档），内存态 bySession 分桶
 * （仿 chat/permission-tier.ts 模式）；变更经 session-prefs 的防抖落盘
 * （set_session_prefs，跨重启还原，FR-14-05）。
 *
 * 名词解释：「thinking 档」指用户为某会话选择的"模型思考力度"档位——
 * Rust 侧经 §11.4 映射表翻译为请求参数（OpenAI 系 reasoning_effort /
 * Anthropic thinking 预算）。`undefined`（无记录）= 未显式设置：按模型
 * 配置里的 per-model reasoningEffort 回落（§11.4 优先级链第二级），
 * 与显式 off（发 "none" 关闭思考）语义不同。
 */

import { create } from "zustand";

import { t } from "@/i18n/core";
import type { ThinkingLevel } from "@/ipc/types";
import { persistSessionPrefs } from "./session-prefs";

/** 档位循环/下拉顺序（票面定案：off → minimal → low → medium → high）。 */
export const THINKING_LEVELS: ThinkingLevel[] = ["off", "minimal", "low", "medium", "high"];

/** 档位的用户可读名（Composer 快捷钮触发器与菜单行首）：经字典查词
 *  （31 号票 i18n，模块级 t() 同 tierLabel 口径）。 */
export function thinkingLabel(level: ThinkingLevel): string {
  switch (level) {
    case "off":
      return t("workspace.thinkingOff");
    case "minimal":
      return t("workspace.thinkingMinimal");
    case "low":
      return t("workspace.thinkingLow");
    case "medium":
      return t("workspace.thinkingMedium");
    case "high":
      return t("workspace.thinkingHigh");
  }
}

/** 档位说明（下拉菜单第二行）：OpenAI 系口径的 effort 词 + Anthropic 比例
 *  预算（§11.4 对照表）；经字典查词（31 号票 i18n）。 */
export function thinkingDescription(level: ThinkingLevel): string {
  switch (level) {
    case "off":
      return t("workspace.thinkingOffDesc");
    case "minimal":
      return t("workspace.thinkingMinimalDesc");
    case "low":
      return t("workspace.thinkingLowDesc");
    case "medium":
      return t("workspace.thinkingMediumDesc");
    case "high":
      return t("workspace.thinkingHighDesc");
  }
}

/** thinking store 的状态与方法面（bySession 分桶，多 Tab 互不影响）。 */
export interface ThinkingStore {
  /** 各会话当前 thinking 档（sessionId → 档位；无记录 = 未显式设置）。 */
  bySession: Record<string, ThinkingLevel>;
  /** 直接选定档位（Composer 下拉；落盘防抖由 setLevelPersisted 承担）。 */
  setLevel: (sessionId: string, level: ThinkingLevel | undefined) => void;
}

/** zustand store：按会话分桶保存 thinking 档（仅内存态；持久化见
 *  setLevelPersisted——store 本体保持纯内存，测试与恢复路径可独立使用）。 */
export const useThinkingStore = create<ThinkingStore>((set) => ({
  bySession: {},

  setLevel: (sessionId, level) => {
    set((s) => {
      const bySession = { ...s.bySession };
      if (level === undefined) delete bySession[sessionId];
      else bySession[sessionId] = level;
      return { bySession };
    });
  },
}));

/** 读会话当前 thinking 档（无记录 = undefined，按 per-model reasoningEffort
 *  回落）。runtime 惰性 runConfig 与 Composer 钮显示共用。 */
export function thinkingOf(sessionId: string): ThinkingLevel | undefined {
  return useThinkingStore.getState().bySession[sessionId] ?? undefined;
}

/** 选定档位并防抖落盘（Composer 快捷钮入口）：undefined = 清除显式设置
 *  （载荷 thinking: null），回 per-model reasoningEffort 回落。 */
export function setLevelPersisted(sessionId: string, level: ThinkingLevel | undefined): void {
  useThinkingStore.getState().setLevel(sessionId, level);
  persistSessionPrefs(sessionId, {
    thinking: level ?? null,
  });
}
