/**
 * 首启引导状态纯函数单测：ui-state 自管 JSON 的清洗（不可信
 * 形状一律回退）、自动弹/横幅/续走三类判定与推进记账语义（前进、单步
 * 跳过、整包关闭、末步收口）。原逐断言用例已表驱动合并，覆盖语义不减。
 */

import { describe, expect, it } from "vitest";
import {
  afterCloseAll,
  afterComplete,
  afterStep,
  clampStep,
  ONBOARDING_TOTAL_STEPS,
  sanitizeOnboarding,
  shouldAutoOpen,
  shouldShowResumeBanner,
} from "./onboarding-state";

describe("onboarding-state 纯函数", () => {
  it("sanitizeOnboarding（ui-state 自管 JSON 不可信形状清洗）：坏形状回 null、正常 round-trip、step 修复与 skippedAt 丢弃", () => {
    // 表：[输入, 期望]（期望 null = 视作全新数据目录，触发首启自动弹）
    const badToNull: unknown[] = [
      undefined,
      null,
      "x",
      42,
      { step: 2 }, // 缺 completed：形状不完整整条丢弃，不猜默认
      {},
    ];
    for (const bad of badToNull) {
      expect(sanitizeOnboarding(bad as never)).toBeNull();
    }
    // 正常记录 round-trip；skippedAt 缺省可省
    expect(sanitizeOnboarding({ completed: false, step: 2 })).toEqual({
      completed: false,
      step: 2,
    });
    expect(sanitizeOnboarding({ completed: true, step: 3, skippedAt: 1700000000000 })).toEqual({
      completed: true,
      step: 3,
      skippedAt: 1700000000000,
    });
    // step 修复：非数字回 0、越界夹取、小数截断；skippedAt 非数字丢弃
    expect(sanitizeOnboarding({ completed: false, step: "2" })).toEqual({
      completed: false,
      step: 0,
    });
    expect(sanitizeOnboarding({ completed: false, step: 99 })).toEqual({
      completed: false,
      step: ONBOARDING_TOTAL_STEPS - 1,
    });
    expect(sanitizeOnboarding({ completed: false, step: -3 })).toEqual({
      completed: false,
      step: 0,
    });
    expect(sanitizeOnboarding({ completed: false, step: 1.7 })).toEqual({
      completed: false,
      step: 1,
    });
    expect(sanitizeOnboarding({ completed: false, step: 1, skippedAt: "x" })).toEqual({
      completed: false,
      step: 1,
    });
  });

  it("触发判定（无记录自动弹；completed 后不再弹且横幅消失）：shouldAutoOpen 与 shouldShowResumeBanner 三态口径", () => {
    const states = [null, { completed: false, step: 0 }, { completed: true, step: 3 }] as const;
    for (const state of states) {
      expect(shouldAutoOpen(state)).toBe(state === null);
      expect(shouldShowResumeBanner(state)).toBe(state !== null && state.completed === false);
    }
  });

  it("推进记账：afterStep 前进/单步跳过/末步整包收口、afterComplete 收口、afterCloseAll 保步与 skippedAt 保留", () => {
    // 正常前进：step+1、不打跳过时间戳、未收口
    expect(afterStep({ completed: false, step: 0 }, false, 1000)).toEqual({
      completed: false,
      step: 1,
    });
    // 单步跳过：step+1 且带 skippedAt 时间戳
    expect(afterStep({ completed: false, step: 0 }, true, 1000)).toEqual({
      completed: false,
      step: 1,
      skippedAt: 1000,
    });
    // 末步前进/跳过均整包收口 completed=true（其后无续走目标）
    expect(afterStep({ completed: false, step: ONBOARDING_TOTAL_STEPS - 1 }, false, 1000)).toEqual({
      completed: true,
      step: ONBOARDING_TOTAL_STEPS - 1,
    });
    expect(afterStep({ completed: false, step: ONBOARDING_TOTAL_STEPS - 1 }, true, 1000)).toEqual({
      completed: true,
      step: ONBOARDING_TOTAL_STEPS - 1,
      skippedAt: 1000,
    });
    // afterComplete：completed 收口（横幅随之消失、不再自动弹）
    expect(afterComplete()).toEqual({ completed: true, step: ONBOARDING_TOTAL_STEPS - 1 });
    // afterCloseAll（顶部 × 整包关闭）：completed=false、step 保持当前步；
    // prior 的 skippedAt 保留、无 prior 不带；越界步序夹取（组件态可能瞬时
    // 越界，落盘前兜底）
    expect(afterCloseAll(2, null)).toEqual({ completed: false, step: 2 });
    expect(afterCloseAll(1, { completed: false, step: 1, skippedAt: 7 })).toEqual({
      completed: false,
      step: 1,
      skippedAt: 7,
    });
    expect(afterCloseAll(9, null)).toEqual({
      completed: false,
      step: ONBOARDING_TOTAL_STEPS - 1,
    });
  });

  it("clampStep：负数回 0、超界夹到末步、非有限数回 0、小数截断、正常值原样", () => {
    expect(clampStep(-1)).toBe(0);
    expect(clampStep(ONBOARDING_TOTAL_STEPS + 5)).toBe(ONBOARDING_TOTAL_STEPS - 1);
    expect(clampStep(Number.NaN)).toBe(0);
    expect(clampStep(2.9)).toBe(2);
    expect(clampStep(1)).toBe(1);
  });
});
