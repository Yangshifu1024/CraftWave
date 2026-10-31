// @vitest-environment happy-dom
/**
 * 滚动锚点测试（29 号票，FR-01-11 收口）：锚点纯几何（computeAnchor/
 * resolveScrollTop）、ui-state 载入清洗（sanitizeScrollAnchors）、
 * DOM 落位（collectAnchorElements/applyAnchor——锚点失效回底部）。
 */

import { describe, expect, it } from "vitest";
import {
  type AnchorElement,
  applyAnchor,
  collectAnchorElements,
  computeAnchor,
  resolveScrollTop,
  sanitizeScrollAnchors,
} from "./scroll-anchor";

/** 消息序列样例：三条消息，顶部偏移 0 / 500 / 1200。 */
const ELEMENTS: AnchorElement[] = [
  { messageId: "m1", top: 0 },
  { messageId: "m2", top: 500 },
  { messageId: "m3", top: 1200 },
];

describe("scroll-anchor（29 号票）", () => {
  it("纯几何与清洗：视口顶上方最后一条 + 相对偏移、恢复几何钳 0、ui-state 载入剔除形状不符", () => {
    // ---- computeAnchor（记账几何）：表驱动口径 ----
    const cases: [number, string, number][] = [
      [0, "m1", 0], // 最顶
      [200, "m1", 200], // m1 区间内
      [500, "m2", 0], // 恰好压线 m2 顶
      [720, "m2", 220], // m2 区间内
      [5000, "m3", 3800], // 超出内容底（rubber-band 范围）：锚最后一条
    ];
    for (const [scrollTop, leafId, offset] of cases) {
      expect(computeAnchor(ELEMENTS, scrollTop), `scrollTop=${scrollTop}`).toEqual({
        leafId,
        offset,
      });
    }
    // 空消息序列返回 null（空会话不记账）
    expect(computeAnchor([], 100)).toBeNull();

    // ---- resolveScrollTop（恢复几何）：锚点命中 → 目标 scrollTop；越顶钳 0 ----
    expect(resolveScrollTop(ELEMENTS, { leafId: "m2", offset: 220 })).toBe(720);
    expect(resolveScrollTop(ELEMENTS, { leafId: "m1", offset: 99999 })).toBe(99999);
    expect(resolveScrollTop([{ messageId: "m1", top: 500 }], { leafId: "m1", offset: -800 })).toBe(
      0,
    );
    // leafId 失效（消息已不存在）→ null（调用方回底部）
    expect(resolveScrollTop(ELEMENTS, { leafId: "ghost", offset: 10 })).toBeNull();

    // ---- sanitizeScrollAnchors（ui-state 载入清洗）：合法保留，形状不符剔除 ----
    expect(
      sanitizeScrollAnchors({
        s1: { leafId: "m1", offset: 12 },
        bad1: { leafId: 3, offset: 0 },
        bad2: { leafId: "m1" },
        bad3: "x",
        bad4: null,
      }),
    ).toEqual({ s1: { leafId: "m1", offset: 12 } });
    expect(sanitizeScrollAnchors("not-object")).toEqual({});
    expect(sanitizeScrollAnchors(null)).toEqual({});
    expect(sanitizeScrollAnchors([1, 2])).toEqual({});
  });

  it("DOM 面：采集换算 + 记账 + 落位 + 失效回底部全链（happy-dom）", () => {
    /**
     * 搭一个滚动容器：3 条 data-message-id 消息。happy-dom 无布局引擎
     * （getBoundingClientRect 恒 0），给消息元素覆写 rect 方法模拟布局——
     * 消息内容顶分别在视口坐标 100/600/1300（容器 rect 恒 0），据此验证
     * collectAnchorElements 的换算公式（elRect.top - 容器 rect.top +
     * scrollTop）与 applyAnchor 的落位/回落链路。
     */
    document.body.innerHTML = "";
    const viewport = document.createElement("div");
    viewport.className = "cw-scroll-viewport";
    viewport.scrollTop = 300;
    const tops: Record<string, number> = { m1: 100, m2: 600, m3: 1300 };
    for (const id of ["m1", "m2", "m3"]) {
      const message = document.createElement("div");
      message.dataset.messageId = id;
      message.getBoundingClientRect = () => ({ top: tops[id] }) as DOMRect;
      viewport.appendChild(message);
    }
    viewport.getBoundingClientRect = () => ({ top: 0 }) as DOMRect;
    const container = document.createElement("div");
    container.appendChild(viewport);
    document.body.appendChild(container);

    // 采集换算：top = 消息 rect.top - 容器 rect.top + scrollTop：100-0+300=400 / 900 / 1600
    const elements = collectAnchorElements(viewport);
    expect(elements.map((e) => e.messageId)).toEqual(["m1", "m2", "m3"]);
    expect(elements.map((e) => e.top)).toEqual([400, 900, 1600]);
    // 视口顶 300：400>300 → 锚第一条，offset 0
    expect(computeAnchor(elements, viewport.scrollTop)).toEqual({ leafId: "m1", offset: 0 });
    // 落位：锚 m2 offset 50 → m2.top + offset = 950
    expect(applyAnchor(viewport, { leafId: "m2", offset: 50 })).toBe(true);
    expect(viewport.scrollTop).toBe(950);
    // leafId 失效 → 回底部（scrollHeight）；无锚点 → 回底部
    expect(applyAnchor(viewport, { leafId: "ghost", offset: 0 })).toBe(false);
    expect(viewport.scrollTop).toBe(viewport.scrollHeight);
    expect(applyAnchor(viewport, undefined)).toBe(false);
    expect(viewport.scrollTop).toBe(viewport.scrollHeight);
    void container;
  });
});
