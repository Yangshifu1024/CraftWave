// @vitest-environment happy-dom
/**
 * 用量统计图表测试（20 号票）：多日多模型 fixture 的渲染面——柱状图根数、
 * 空态、排行条内容与占比序。精简版：两图合并为代表例，段内手动卸载。
 */

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { DayUsage, ModelUsage } from "@/ipc/types";
import { compactTokens, DailyBars, ModelBars } from "./usage-charts";

afterEach(cleanup);

const days: DayUsage[] = [
  { day: "2026-10-28", input: 100, output: 10, cacheRead: 0, cacheWrite: 0, total: 110 },
  { day: "2026-10-29", input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  { day: "2026-10-30", input: 500, output: 50, cacheRead: 30, cacheWrite: 0, total: 580 },
];

const models: ModelUsage[] = [
  {
    modelId: "m1",
    modelName: "GPT-4o",
    input: 600,
    output: 60,
    cacheRead: 30,
    cacheWrite: 0,
    total: 690,
    runs: 3,
  },
  {
    modelId: "m2",
    modelName: "Claude",
    input: 100,
    output: 10,
    cacheRead: 0,
    cacheWrite: 0,
    total: 110,
    runs: 1,
  },
];

describe("用量统计图表（20 号票）", () => {
  it("DailyBars：柱根数/截窗/空态/零值占位；ModelBars：排行内容与空态", () => {
    // 柱根数 = 天数
    const first = render(<DailyBars days={days} />);
    const svg = first.container.querySelector('[data-testid="stats-daily-bars"]')!;
    expect(svg.querySelectorAll("rect").length).toBe(3);
    first.unmount();

    // 31 天进、30 根柱（近 30 天窗口）
    const many: DayUsage[] = Array.from({ length: 31 }, (_, i) => ({
      day: `2026-09-${String(i + 1).padStart(2, "0")}`,
      input: i,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      total: i,
    }));
    const second = render(<DailyBars days={many} />);
    expect(
      second.container.querySelector('[data-testid="stats-daily-bars"]')!.querySelectorAll("rect")
        .length,
    ).toBe(30);
    second.unmount();

    // 空态与零值日（纯零值日：0.5 高度占位柱可见，不是空态）
    const empty = render(<DailyBars days={[]} />);
    expect(screen.getByTestId("stats-daily-empty")).toBeTruthy();
    empty.unmount();
    const zero: DayUsage[] = [
      { day: "2026-10-30", input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    ];
    const zeroView = render(<DailyBars days={zero} />);
    expect(zeroView.container.querySelector("rect")).toBeTruthy();
    zeroView.unmount();

    // ModelBars：第一行 = m1（690 tok · 3 次 · 缓存命中率 30/(600+30)≈5%）
    const filled = render(<ModelBars models={models} />);
    const list = screen.getByTestId("stats-model-bars")!;
    expect(list.querySelectorAll("li").length).toBe(2);
    expect(list.textContent).toContain("GPT-4o");
    expect(list.textContent).toContain(compactTokens(690));
    expect(list.textContent).toContain("3 次");
    expect(list.textContent).toContain("缓存 5%");
    filled.unmount();

    const modelsEmpty = render(<ModelBars models={[]} />);
    expect(screen.getByTestId("stats-models-empty")).toBeTruthy();
    modelsEmpty.unmount();
  });
});
