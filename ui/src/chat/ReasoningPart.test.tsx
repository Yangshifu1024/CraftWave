// @vitest-environment happy-dom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { ReasoningPart } from "./ReasoningPart";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it("流式思考显示最后一个非空行和计时，结束后冻结且移除摘要", () => {
  vi.useFakeTimers();
  const now = vi.spyOn(performance, "now").mockReturnValue(2000);
  const { rerender } = render(
    <ReasoningPart
      text={"第一行\r\n最新思考\r\n\n"}
      status={{ type: "running" }}
      timing={{ startedAt: 1000 }}
    />,
  );
  expect(screen.getByText("思考中 · 1s")).toBeTruthy();
  expect(screen.getByTestId("reasoning-preview").textContent).toBe("最新思考");
  now.mockReturnValue(4000);
  act(() => vi.advanceTimersByTime(1000));
  expect(screen.getByText("思考中 · 3s")).toBeTruthy();
  rerender(
    <ReasoningPart
      text="第一行\n最新思考"
      status={{ type: "complete" }}
      timing={{ startedAt: 1000, completedAt: 3500 }}
    />,
  );
  expect(screen.getByText("思考过程 · 3s")).toBeTruthy();
  expect(screen.queryByTestId("reasoning-preview")).toBeNull();
  now.mockReturnValue(9000);
  act(() => vi.advanceTimersByTime(5000));
  expect(screen.getByText("思考过程 · 3s")).toBeTruthy();
});

it("历史思考无时间时不虚构时长；展开纯文本不解释 Markdown", () => {
  const { container } = render(<ReasoningPart text="**原文**" />);
  expect(screen.getByText("思考过程")).toBeTruthy();
  const details = container.querySelector("details")!;
  details.open = true;
  fireEvent(details, new Event("toggle"));
  expect(details.open).toBe(true);
  expect(container.querySelector("p")?.textContent).toBe("**原文**");
  expect(container.querySelector("strong")).toBeNull();
});
