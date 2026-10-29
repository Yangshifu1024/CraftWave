// @vitest-environment happy-dom
/**
 * 走马灯组件溢出判定单测：mock 布局量测（jsdom/happy-dom 无真实排版），
 * 验证「以单份标题真实宽度判定溢出」的状态机——溢出时挂渐隐 mask 与
 * marquee 类并渲染副本，未溢出时全部不挂（短标题不带渐隐样式）。
 * Web Animations 动画本体依赖真实浏览器，不在单测面内。
 */

import { cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { TaskTitleOverflowText } from "./TaskTitleOverflowText";

/** 以指定内容宽度接管布局量测：仅 original 副本报告宽度（组件以单份
 *  真实排版宽度判定），其余元素按容器宽 200 处理。 */
function mockLayout(contentWidth: number) {
  Object.defineProperty(HTMLElement.prototype, "scrollWidth", {
    configurable: true,
    get() {
      return (this as HTMLElement).dataset?.taskTitleCopy === "original" ? contentWidth : 0;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", {
    configurable: true,
    get() {
      return 200;
    },
  });
}

beforeEach(() => {
  mockLayout(0);
});

afterEach(() => {
  cleanup();
  Reflect.deleteProperty(HTMLElement.prototype, "scrollWidth");
  Reflect.deleteProperty(HTMLElement.prototype, "clientWidth");
});

describe("TaskTitleOverflowText", () => {
  it("标题超宽时进入溢出态：挂双侧 mask 与 marquee 类并渲染走马灯副本", () => {
    mockLayout(500);
    const { container } = render(
      <TaskTitleOverflowText as="span">很长很长的会话标题</TaskTitleOverflowText>,
    );

    const root = container.firstElementChild as HTMLElement;
    expect(root.className).toContain("task-title-marquee");
    expect(root.className).toContain(
      "[mask-image:linear-gradient(to_right,black_calc(100%_-_1.5rem),transparent)]",
    );

    const track = container.querySelector<HTMLElement>("[data-task-title-marquee-track]");
    expect(track).not.toBeNull();
    // 副本间距 24px（gap-6）
    expect(track!.className).toContain("gap-6");
    const duplicate = container.querySelector('[data-task-title-copy="duplicate"]');
    expect(duplicate).not.toBeNull();
    expect(duplicate!.getAttribute("aria-hidden")).toBe("true");
  });

  it("短标题不进入溢出态：不挂 mask、不渲染副本（完整可见不带渐隐）", () => {
    mockLayout(100);
    const { container } = render(<TaskTitleOverflowText as="span">短标题</TaskTitleOverflowText>);

    const root = container.firstElementChild as HTMLElement;
    expect(root.className).not.toContain("task-title-marquee");
    expect(root.className).not.toContain("[mask-image");
    expect(container.querySelector('[data-task-title-copy="duplicate"]')).toBeNull();
  });
});
