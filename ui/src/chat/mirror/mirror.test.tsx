// @vitest-environment happy-dom
/**
 * 【S1 spike 原型，随时可弃】镜像工具桥接集成测试。
 *
 * 跑真实管线：useLocalRuntime(mirrorAdapter) + AuiConfig(tools: Tools) + primitives Thread，
 * mock 帧源按剧本推进，断言：
 * 1. 「参数准备中」态出现且半截 JSON 原文可见；
 * 2. 状态机流转：参数准备中 → 运行中 → 完成 / 错误；
 * 3. 不重挂载：每个 toolCallId 的卡片 mount 恰好 1 次；
 * 4. 帧累积器单测（纯逻辑部分）。
 */

import { AssistantRuntimeProvider, AuiConfig, Tools, useLocalRuntime } from "@assistant-ui/react";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ChatThread } from "../Thread";
import { FrameBuffer, isArgsTextComplete, type MirrorFrame } from "./frames";
import { mountLog, resetMountLog } from "./MirrorCards";
import { mirrorAdapter } from "./mirrorRuntime";
import { MOCK_SCRIPT, scriptedFrameSource } from "./mockSource";
import { mirrorToolkit } from "./toolkit";

// happy-dom 缺的浏览器 API 补齐（assistant-ui 视口观察用）
globalThis.ResizeObserver ??= class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

// 与 MirrorDemo 相同的挂载方式（config 传 Tools 资源）
const testAuiConfig = AuiConfig({ tools: Tools({ toolkit: mirrorToolkit }) });

/** 测试壳：与 MirrorDemo 相同接线；发送走真实 composer（输入 + Send 按钮）。 */
function TestHarness() {
  const runtime = useLocalRuntime(mirrorAdapter);
  return (
    <AssistantRuntimeProvider runtime={runtime} config={testAuiConfig}>
      <ChatThread />
    </AssistantRuntimeProvider>
  );
}

/** 真实 UI 交互发送：填 composer → 点 Send。 */
function sendFromComposer(text: string) {
  fireEvent.change(screen.getByPlaceholderText("发一条消息试试骨架回声…"), {
    target: { value: text },
  });
  // Send 带 aria-label「发送」（17 号票起 Composer 区还有回底按钮等，按名取）
  fireEvent.click(screen.getByRole("button", { name: "发送" }));
}

describe("FrameBuffer 帧累积（纯逻辑）", () => {
  it("半截 JSON 增量累积进 argsText，result 到达前后状态正确", () => {
    const buf = new FrameBuffer();
    const frames: MirrorFrame[] = [
      { type: "tool-start", seq: 1, toolCallId: "t1", toolName: "read_workspace_file" },
      { type: "tool-progress", seq: 2, toolCallId: "t1", argsTextDelta: '{"path": "src/' },
      { type: "tool-progress", seq: 3, toolCallId: "t1", argsTextDelta: 'main.rs"}' },
      { type: "tool-result", seq: 4, toolCallId: "t1", result: { ok: true } },
    ];
    for (const f of frames) buf.push(f);
    const parts = buf.toParts();
    expect(parts).toHaveLength(1);
    const part = parts[0]!;
    expect(part.type).toBe("tool-call");
    if (part.type !== "tool-call") return;
    expect(part.argsText).toBe('{"path": "src/main.rs"}');
    expect(isArgsTextComplete(part.argsText)).toBe(true);
    expect(part.result).toEqual({ ok: true });
  });

  it("每次快照全量累计（替换语义），顺序按首次出现", () => {
    const buf = new FrameBuffer();
    buf.push({ type: "text-delta", seq: 1, delta: "a" });
    buf.push({ type: "tool-start", seq: 2, toolCallId: "t1", toolName: "x" });
    const snap1 = buf.toParts();
    expect(snap1.map((p) => p.type)).toEqual(["text", "tool-call"]);
    buf.push({ type: "text-delta", seq: 3, delta: "b" });
    const snap2 = buf.toParts();
    // 累计替换：第二次快照包含全部内容，且 tool part 引用同一累积对象
    expect(snap2[0]).toEqual({ type: "text", text: "ab" });
    expect(snap2[1]).toBe(snap1[1]);
  });
});

describe("剧本帧源", () => {
  it("MOCK_SCRIPT 含半截 JSON 增量且拼回完整", async () => {
    const acc = new Map<string, string>();
    for await (const frame of scriptedFrameSource(MOCK_SCRIPT)) {
      if (frame.type === "tool-progress") {
        acc.set(frame.toolCallId, (acc.get(frame.toolCallId) ?? "") + frame.argsTextDelta);
      }
    }
    expect(acc.get("call-1")).toBe('{"path": "src/main.rs", "startLine": 1}');
    expect(acc.get("call-2")).toBe('{"command": "cargo test"}');
  });
});

describe("镜像工具桥接（真实渲染管线）", () => {
  // vitest 非 globals 模式下 RTL 不自动清理，需手动卸载（否则两次 render 叠加查询撞车）
  afterEach(cleanup);
  beforeEach(() => {
    resetMountLog();
  });

  it("状态机流转：参数准备中(半截JSON可见) → 运行中 → 完成/错误；卡片不重挂载", async () => {
    render(<TestHarness />);
    sendFromComposer("开始剧本");

    // 1) 参数准备中：call-1 卡片先以 preparing 态出现，且半截 JSON 原文可见
    await waitFor(
      () => {
        const card = screen.getByTestId("tool-card-call-1");
        expect(card.getAttribute("data-phase")).toBe("preparing");
        expect(card.textContent).toContain('{"path": "src/');
        expect(screen.queryByText("参数准备中")).not.toBeNull();
      },
      { timeout: 3000 },
    );

    // 2) 参数齐 → 运行中（call-1 完成前会经过 running 态）
    await waitFor(
      () => {
        expect(screen.getByTestId("tool-card-call-1").getAttribute("data-phase")).toBe("running");
      },
      { timeout: 3000 },
    );

    // 3) 完成 + 错误终态（剧本播完）
    await waitFor(
      () => {
        expect(screen.getByTestId("tool-card-call-1").getAttribute("data-phase")).toBe("done");
        expect(screen.getByTestId("tool-card-call-2").getAttribute("data-phase")).toBe("error");
      },
      { timeout: 5000 },
    );
    expect(screen.getByTestId("tool-card-call-1").textContent).toContain("fn main() {}");
    expect(screen.getByTestId("tool-card-call-2").textContent).toContain("编译失败");
    // 收尾文本正常渲染（两条 text-delta 累积进同一个 text part，用子串匹配）
    await waitFor(
      () => {
        expect(screen.getByText(/读取完成，但命令执行失败了。/)).not.toBeNull();
      },
      { timeout: 3000 },
    );

    // 4) 不重挂载：每个 toolCallId 恰好 mount 1 次（mount 后从未 unmount/remount）
    const mounts = mountLog().filter((e) => e.kind === "mount");
    const unmounts = mountLog().filter((e) => e.kind === "unmount");
    expect(mounts.filter((e) => e.toolCallId === "call-1")).toHaveLength(1);
    expect(mounts.filter((e) => e.toolCallId === "call-2")).toHaveLength(1);
    expect(unmounts).toHaveLength(0);
  });

  it("运行中不重挂载：中途与终态各采样一次，mount 计数不变", async () => {
    render(<TestHarness />);
    sendFromComposer("开始剧本");

    // 等到 call-2 出现且处于 running（参数一条到齐、结果未回的窗口）
    await waitFor(
      () => {
        expect(screen.getByTestId("tool-card-call-2").getAttribute("data-phase")).toBe("running");
      },
      { timeout: 5000 },
    );
    const midMounts = mountLog().filter((e) => e.kind === "mount");

    // 剧本播完
    await waitFor(
      () => {
        expect(screen.getByTestId("tool-card-call-2").getAttribute("data-phase")).toBe("error");
      },
      { timeout: 5000 },
    );
    const endMounts = mountLog().filter((e) => e.kind === "mount");
    // 中途与终态 mount 数一致 → 数十次快照 yield 期间没有任何重挂载
    expect(endMounts).toEqual(midMounts);
    expect(endMounts.filter((e) => e.toolCallId === "call-2")).toHaveLength(1);
    expect(mountLog().filter((e) => e.kind === "unmount")).toHaveLength(0);
  });
});
