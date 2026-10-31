// @vitest-environment happy-dom
/**
 * 重连观察态覆盖层组件测试（§5.8，05 号票）：观察态渲染（重放内容/工具行）、
 * 取消按钮触发 cancelObservedRun、覆盖层存在即输入被隔离、非观察态不渲染。
 * 精简版：渲染/隔离/非观察态与取消链合并为代表例。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { FrameSnapshot } from "./frames";
import { ReconnectOverlay } from "./ReconnectOverlay";
import { useRunReconnectStore } from "./run-reconnect";

/** 把 store 直接置为观察态（组件测试不经重连流程，聚焦渲染与交互）。 */
function enterObserving(sessionId: string, snapshot: FrameSnapshot) {
  useRunReconnectStore.getState().enter(sessionId);
  useRunReconnectStore.getState().noteFrame(sessionId, 99, snapshot);
}

afterEach(() => {
  cleanup();
  useRunReconnectStore.setState({ scopes: {}, reloadTick: {} });
  vi.restoreAllMocks();
});

describe("ReconnectOverlay（重连观察态覆盖层）", () => {
  it("观察态渲染重放面并隔离输入；取消按钮触发 cancelObservedRun；非观察态不渲染", async () => {
    // 观察态：顶部细条 + 思考行 + 正文 + 工具行 + 取消钮
    enterObserving("s1", {
      segments: [
        { kind: "reasoning", text: "先看看目录" },
        { kind: "text", text: "我来帮你梳理" },
        { kind: "tool", callId: "c1" },
      ],
      toolCalls: [
        {
          callId: "c1",
          name: "read",
          argsJson: "{}",
          result: { ok: true, uiSummary: "[read] 完成" },
        },
      ],
      started: true,
      ended: null,
      run: { runId: "r1", model: "gpt-mock" },
      usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      rate: null,
      compaction: null,
      thinking: null,
    });
    render(
      <div style={{ position: "relative", width: 400, height: 400 }}>
        <button type="button">会话输入按钮（被覆盖）</button>
        <ReconnectOverlay sessionId="s1" />
      </div>,
    );
    expect(screen.getByTestId("reconnect-overlay")).toBeDefined();
    expect(screen.getByTestId("reconnect-banner").textContent).toContain("已重连到运行中的任务");
    expect(screen.getByTestId("reconnect-reasoning").textContent).toContain("先看看目录");
    expect(screen.getByTestId("reconnect-text").textContent).toContain("我来帮你梳理");
    const toolRow = screen.getByTestId("reconnect-tool");
    expect(toolRow.textContent).toContain("read");
    expect(toolRow.textContent).toContain("[read] 完成");
    // 覆盖层铺满宿主面（absolute inset-0：happy-dom 不解析 Tailwind，按
    // 类名断言；Composer 禁用由覆盖隔离实现）
    const overlay = screen.getByTestId("reconnect-overlay");
    expect(overlay.className).toContain("absolute");
    expect(overlay.className).toContain("inset-0");
    expect(overlay.textContent).not.toContain("被覆盖");
    cleanup();

    // 取消按钮 → cancelObservedRun（置位即返回，终态以 runEnded 帧为权威；
    // cancelRun 的 IPC 出口已有 Rust e2e 覆盖，此处 spy 验证调用链）
    enterObserving("s4", {
      segments: [{ kind: "text", text: "x" }],
      toolCalls: [],
      started: true,
      ended: null,
      run: { runId: "r1", model: "m" },
      usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      rate: null,
      compaction: null,
      thinking: null,
    });
    const spy = vi
      .spyOn(await import("./run-reconnect"), "cancelObservedRun")
      .mockImplementation(async () => true);
    render(<ReconnectOverlay sessionId="s4" />);
    fireEvent.click(screen.getByTestId("reconnect-cancel"));
    await waitFor(() => {
      expect(spy).toHaveBeenCalledWith("s4");
    });
    cleanup();

    // 非观察态不渲染任何内容
    render(<ReconnectOverlay sessionId="s3" />);
    expect(screen.queryByTestId("reconnect-overlay")).toBeNull();
  });
});
