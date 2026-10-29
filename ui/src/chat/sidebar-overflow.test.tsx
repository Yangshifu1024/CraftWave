// @vitest-environment happy-dom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { MarqueeTitle, useScrollFade } from "./SessionWorkspace";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

it("标题只测量原文，重复测量保持稳定，变宽后撤掉副本", () => {
  let viewportWidth = 100;
  const observers: ResizeObserverCallback[] = [];
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: ResizeObserverCallback) {
        observers.push(callback);
      }
      observe() {}
      disconnect() {}
    },
  );
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockImplementation(() => viewportWidth);
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
    this: HTMLElement,
  ) {
    return new DOMRect(0, 0, this.textContent === "很长的会话标题" ? 400 : 50, 20);
  });
  const { container } = render(<MarqueeTitle text="很长的会话标题" />);
  const title = container.firstElementChild!;
  expect(title.getAttribute("data-overflow")).toBe("true");
  const track = container.querySelector<HTMLElement>(".cw-marquee-track")!;
  expect(track.style.getPropertyValue("--marquee-shift")).toBe("-424px");
  act(() =>
    observers.forEach((callback) => {
      callback([], {} as ResizeObserver);
    }),
  );
  expect(track.style.getPropertyValue("--marquee-shift")).toBe("-424px");
  viewportWidth = 500;
  act(() =>
    observers.forEach((callback) => {
      callback([], {} as ResizeObserver);
    }),
  );
  expect(title.getAttribute("data-overflow")).toBe("false");
  expect(container.querySelector("[aria-hidden]")).toBeNull();
});

function FadeHarness({ text }: { text: string }) {
  const { ref, fade } = useScrollFade();
  return (
    <div ref={ref} data-testid="viewport" data-fade={fade}>
      <p>{text}</p>
    </div>
  );
}

it("列表内容追加即更新渐隐，无需等待用户滚动或窗口 resize", async () => {
  let height = 80;
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(100);
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockImplementation(() => height);
  const { rerender } = render(<FadeHarness text="一条会话" />);
  expect(screen.getByTestId("viewport").getAttribute("data-fade")).toBe("none");
  height = 200;
  rerender(<FadeHarness text="增加了多条会话" />);
  await waitFor(() =>
    expect(screen.getByTestId("viewport").getAttribute("data-fade")).toBe("bottom"),
  );
});
