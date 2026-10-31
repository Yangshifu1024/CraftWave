// @vitest-environment happy-dom
/**
 * Toast store 测试（29 号票，FR-13-17 顶部轻提示）：自动出队（fake timers
 * 定格 3s/6s 时长口径）、同屏堆叠上限挤最老、手动 dismiss、便捷入口。
 */

import { act } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_DURATION_BY_KIND, MAX_STACK, pushToast, useToastStore } from "./toast";

beforeEach(() => {
  vi.useFakeTimers();
  useToastStore.setState({ toasts: [] });
  return () => {
    vi.useRealTimers();
  };
});

describe("useToastStore（顶部轻提示）", () => {
  it("剧本：push 入队、3s/6s 自动出队节奏、duration 覆盖与手动 dismiss、堆叠上限挤最老、被挤条目残留定时器不复活", () => {
    // push 入队：成功/信息 3s、错误 6s 自动出队
    act(() => {
      pushToast("success", "已保存");
      pushToast("error", "保存失败", "E_IO: 磁盘满");
      pushToast("info", "提示");
    });
    expect(useToastStore.getState().toasts.map((t) => t.title)).toEqual([
      "已保存",
      "保存失败",
      "提示",
    ]);
    // 3s：成功/信息到期，错误（6s）仍在
    act(() => {
      vi.advanceTimersByTime(DEFAULT_DURATION_BY_KIND.success + 1);
    });
    expect(useToastStore.getState().toasts.map((t) => t.kind)).toEqual(["error"]);
    // 6s：错误也到期
    act(() => {
      vi.advanceTimersByTime(DEFAULT_DURATION_BY_KIND.error);
    });
    expect(useToastStore.getState().toasts).toEqual([]);

    // duration 可覆盖缺省时长（缺省 6s 已过但自定义 60s 未到）；dismiss 手动出队
    let customId = -1;
    act(() => {
      customId = useToastStore.getState().push({
        kind: "error",
        title: "自定义时长",
        detail: "详情",
        duration: 60_000,
      });
    });
    act(() => {
      vi.advanceTimersByTime(10_000);
    });
    expect(useToastStore.getState().toasts).toHaveLength(1);
    act(() => {
      useToastStore.getState().dismiss(customId);
    });
    expect(useToastStore.getState().toasts).toEqual([]);

    // 堆叠上限 MAX_STACK 条：超出挤掉最老的（最老两条被挤，保留最新 MAX_STACK 条）
    useToastStore.setState({ toasts: [] });
    act(() => {
      for (let i = 0; i < MAX_STACK + 2; i += 1) {
        pushToast("info", `提示 ${i}`);
      }
    });
    expect(useToastStore.getState().toasts.map((t) => t.title)).toEqual(
      Array.from({ length: MAX_STACK }, (_, i) => `提示 ${i + 2}`),
    );

    // 被挤掉条目的残留定时器到期不复活、不出错（幽灵 dismiss 不炸在屏条目）
    useToastStore.setState({ toasts: [] });
    act(() => {
      // 被挤条目给一个短时长：它的 timer 到期时条目已不在屏
      useToastStore.getState().push({ kind: "info", title: "会被挤掉", duration: 100 });
      pushToast("info", "1");
      pushToast("info", "2");
      pushToast("info", "3");
    });
    expect(useToastStore.getState().toasts.map((t) => t.title)).toEqual(["1", "2", "3"]);
    act(() => {
      // 挤掉条目的 timer（100ms）到期 → dismiss 一个不存在的 id
      vi.advanceTimersByTime(101);
    });
    // 在屏条目不受影响（缺省 3s 未到），也不因幽灵 dismiss 出错
    expect(useToastStore.getState().toasts.map((t) => t.title)).toEqual(["1", "2", "3"]);
  });
});
