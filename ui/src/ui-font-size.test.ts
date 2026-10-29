// @vitest-environment happy-dom
import { afterEach, expect, it } from "vitest";
import { applyUiFontSize } from "./ui-font-size";

afterEach(() => document.documentElement.removeAttribute("style"));

it("字号只更新基准变量并限制范围，旧配置和异常值使用默认值", () => {
  for (const [value, expected] of [
    [undefined, 14],
    [18, 18],
    [0, 12],
    [99, 18],
    [15.6, 16],
    [Number.NaN, 14],
  ]) {
    applyUiFontSize(value);
    expect(document.documentElement.style.getPropertyValue("--ui-font-size")).toBe(`${expected}px`);
    expect(document.documentElement.style.fontSize).toBe("");
  }
});
