// @vitest-environment happy-dom
/**
 * 滚动渐隐容器单测：mock 滚动量测（jsdom/happy-dom 无真实布局），验证
 * mask 四态机——无内容可滚为 none、贴顶为 bottom（下方还有内容渐隐）、
 * 中间为 both、贴底为 top，滚动事件与内容尺寸变化都会重新判定。
 */

import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ScrollFadeViewport } from "./ScrollFadeViewport";

afterEach(cleanup);

/** 以给定滚动几何接管量测并触发一次 scroll 重判定。 */
function scrollAt(
  viewport: HTMLElement,
  scrollTop: number,
  scrollHeight = 1000,
  clientHeight = 400,
) {
  Object.defineProperty(viewport, "scrollHeight", { configurable: true, value: scrollHeight });
  Object.defineProperty(viewport, "clientHeight", { configurable: true, value: clientHeight });
  Object.defineProperty(viewport, "scrollTop", { configurable: true, value: scrollTop });
  fireEvent.scroll(viewport);
}

describe("ScrollFadeViewport", () => {
  it("内容不超高时为 none（不挂任何 mask）", () => {
    const { container } = render(
      <ScrollFadeViewport>
        <p>一屏内的内容</p>
      </ScrollFadeViewport>,
    );
    const viewport = container.firstElementChild as HTMLElement;
    scrollAt(viewport, 0, 400, 400);
    expect(viewport.dataset.scrollMask).toBe("none");
    expect(viewport.className).not.toContain("[mask-image");
  });

  it("贴顶时底部 24px 渐隐；滚到中间变双侧；贴底变顶部渐隐", () => {
    const { container } = render(
      <ScrollFadeViewport>
        <p>长列表</p>
      </ScrollFadeViewport>,
    );
    const viewport = container.firstElementChild as HTMLElement;

    scrollAt(viewport, 0);
    expect(viewport.dataset.scrollMask).toBe("bottom");
    expect(viewport.className).toContain("black_calc(100%_-_24px)");

    scrollAt(viewport, 300);
    expect(viewport.dataset.scrollMask).toBe("both");

    // 滚到底（scrollTop = scrollHeight - clientHeight = 600）只渐隐顶部
    scrollAt(viewport, 600);
    expect(viewport.dataset.scrollMask).toBe("top");
    expect(viewport.className).toContain("transparent_0,black_24px,black_100%");
  });
});
