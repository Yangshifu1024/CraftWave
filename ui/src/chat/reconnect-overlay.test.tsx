// @vitest-environment happy-dom
/**
 * 重连观察态覆盖层组件测试（§5.8，05 号票）：观察态渲染（重放内容/工具行）、
 * runEnded 收口提示、取消按钮触发 cancel_run、覆盖层存在即输入被隔离、
 * 非观察态不渲染。IPC/依赖全部注入假实现。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { FrameSnapshot } from "./frames";
import { ReconnectOverlay } from "./ReconnectOverlay";
import { cancelObservedRun, useRunReconnectStore } from "./run-reconnect";

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
  it("观察态渲染重放内容：顶部细条 + 思考行 + 正文 + 工具行 + 处理中", () => {
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
    render(<ReconnectOverlay sessionId="s1" />);
    expect(screen.getByTestId("reconnect-overlay")).toBeDefined();
    expect(screen.getByTestId("reconnect-banner").textContent).toContain("已重连到运行中的任务");
    expect(screen.getByTestId("reconnect-reasoning").textContent).toContain("先看看目录");
    expect(screen.getByTestId("reconnect-text").textContent).toContain("我来帮你梳理");
    const toolRow = screen.getByTestId("reconnect-tool");
    expect(toolRow.textContent).toContain("read");
    expect(toolRow.textContent).toContain("[read] 完成");
    expect(screen.getByTestId("reconnect-cancel")).toBeDefined();
  });

  it("runEnded 后显示收口提示（树重载由收口信号驱动，覆盖层随后消失）", () => {
    enterObserving("s2", {
      segments: [],
      toolCalls: [],
      started: true,
      ended: { reason: "done", durationMs: 10, toolCount: 0 },
      run: { runId: "r1", model: "gpt-mock" },
      usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      rate: null,
      compaction: null,
      thinking: null,
    });
    render(<ReconnectOverlay sessionId="s2" />);
    expect(screen.getByText("任务已结束，正在恢复会话…")).toBeDefined();
  });

  it("非观察态不渲染任何内容", () => {
    render(<ReconnectOverlay sessionId="s3" />);
    expect(screen.queryByTestId("reconnect-overlay")).toBeNull();
  });

  it("取消按钮触发 cancel_run（置位即返回，终态以 runEnded 帧为权威）", async () => {
    const cancelRun = vi.fn(async () => true);
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
    // cancelObservedRun 经依赖注入：临时替换 client.cancelRun 不可行——
    // 该函数内部走 defaultReconnectDeps()，此处直接验证按钮 → cancelObservedRun
    // 的调用链（cancelRun 的 IPC 出口已有 Rust e2e 覆盖），用 spy 包 store 无关面。
    const spy = vi
      .spyOn(await import("./run-reconnect"), "cancelObservedRun")
      .mockImplementation(async () => true);
    void cancelRun;
    render(<ReconnectOverlay sessionId="s4" />);
    fireEvent.click(screen.getByTestId("reconnect-cancel"));
    await waitFor(() => {
      expect(spy).toHaveBeenCalledWith("s4");
    });
  });

  it("覆盖层覆盖会话面（输入被隔离）：overlay 以 absolute inset-0 覆盖存在", () => {
    enterObserving("s5", {
      segments: [],
      toolCalls: [],
      started: true,
      ended: null,
      run: null,
      usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      rate: null,
      compaction: null,
      thinking: null,
    });
    render(
      <div style={{ position: "relative", width: 400, height: 400 }}>
        <button type="button">会话输入按钮（被覆盖）</button>
        <ReconnectOverlay sessionId="s5" />
      </div>,
    );
    const overlay = screen.getByTestId("reconnect-overlay");
    // 覆盖层铺满宿主面（absolute inset-0：happy-dom 不解析 Tailwind，按
    // 类名断言；Composer 禁用由覆盖隔离实现）
    expect(overlay.className).toContain("absolute");
    expect(overlay.className).toContain("inset-0");
    expect(overlay.textContent).not.toContain("被覆盖");
  });

  it("cancelObservedRun 依赖注入生效（接线面冒烟）", async () => {
    const cancelRun = vi.fn(async () => true);
    await cancelObservedRun("s6", { cancelRun });
    expect(cancelRun).toHaveBeenCalledWith("s6");
  });
});
