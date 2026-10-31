// @vitest-environment happy-dom
/**
 * ReasoningPart 测试。精简版：流式计时/完成冻结与历史展开合并为代表例。
 */

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { ReasoningPart } from "./ReasoningPart";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it("思考块：流式摘要与计时冻结、完成时长不虚构、历史展开纯文本不解释 Markdown", () => {
  // 流式：收起 + 「正在思考」流光 + 单行摘要（ZCode 收起态不渲染秒数）；
  // 完成后「思考 · 持续了 3 秒」（Math.ceil 口径），摘要移除、时长冻结
  vi.useFakeTimers();
  const now = vi.spyOn(performance, "now").mockReturnValue(2000);
  const first = render(
    <ReasoningPart
      text={"第一行\r\n最新思考\r\n\n"}
      status={{ type: "running" }}
      timing={{ startedAt: 1000 }}
    />,
  );
  expect(screen.getByText("正在思考")).toBeTruthy();
  expect(screen.queryByText(/持续了/)).toBeNull();
  expect(screen.getByTestId("reasoning-preview").textContent).toBe("最新思考");
  now.mockReturnValue(4000);
  act(() => vi.advanceTimersByTime(1000));
  expect(screen.getByTestId("reasoning-preview").textContent).toBe("最新思考");
  first.rerender(
    <ReasoningPart
      text="第一行\n最新思考"
      status={{ type: "complete" }}
      timing={{ startedAt: 1000, completedAt: 3500 }}
    />,
  );
  expect(screen.getByText("思考")).toBeTruthy();
  expect(screen.getByText("持续了 3 秒")).toBeTruthy();
  expect(screen.queryByTestId("reasoning-preview")).toBeNull();
  now.mockReturnValue(9000);
  act(() => vi.advanceTimersByTime(5000));
  expect(screen.getByText("持续了 3 秒")).toBeTruthy();
  first.unmount();
  vi.useRealTimers();
  vi.restoreAllMocks();

  // 历史思考无时间时不虚构时长；展开纯文本不解释 Markdown
  const second = render(<ReasoningPart text="**原文**" />);
  expect(screen.getByText("思考")).toBeTruthy();
  const details = second.container.querySelector("details")!;
  details.open = true;
  fireEvent(details, new Event("toggle"));
  expect(details.open).toBe(true);
  expect(second.container.querySelector("p")?.textContent).toBe("**原文**");
  expect(second.container.querySelector("strong")).toBeNull();
  second.unmount();
});
