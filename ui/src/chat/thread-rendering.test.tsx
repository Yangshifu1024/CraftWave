// @vitest-environment happy-dom
import {
  AssistantRuntimeProvider,
  type ChatModelAdapter,
  useLocalRuntime,
} from "@assistant-ui/react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { ChatThread } from "./Thread";

vi.mock("@/ipc/client", async (original) => ({
  ...(await original<typeof import("@/ipc/client")>()),
  getConfig: async () => ({ providers: [] }),
}));

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

function Harness({ adapter }: { adapter: ChatModelAdapter }) {
  const runtime = useLocalRuntime(adapter);
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <ChatThread />
    </AssistantRuntimeProvider>
  );
}

it("建议填入实际 composer，真实 runtime 的 reasoning 状态传到思考摘要", async () => {
  let finish = () => {};
  const gate = new Promise<void>((resolve) => {
    finish = resolve;
  });
  const startedAt = performance.now();
  const adapter: ChatModelAdapter = {
    async *run() {
      yield {
        content: [
          {
            type: "reasoning",
            text: "先读取配置\n检查调用链",
            status: { type: "running" },
            timing: { startedAt },
          },
        ],
      };
      await gate;
      yield {
        content: [
          {
            type: "reasoning",
            text: "先读取配置\n检查调用链",
            status: { type: "complete" },
            timing: { startedAt, completedAt: startedAt + 2000 },
          },
          { type: "text", text: "已完成" },
        ],
      };
    },
  };
  render(<Harness adapter={adapter} />);
  fireEvent.click(screen.getByRole("button", { name: "梳理当前项目的目录结构与关键模块" }));
  const input = screen.getByRole("textbox") as HTMLTextAreaElement;
  expect(input.value).toBe("梳理当前项目的目录结构与关键模块");
  fireEvent.click(screen.getByRole("button", { name: "发送" }));
  await waitFor(() =>
    expect(screen.getByTestId("reasoning-preview").textContent).toBe("检查调用链"),
  );
  finish();
  await waitFor(() => expect(screen.getByText("思考过程 · 2s")).toBeTruthy());
  expect(screen.queryByTestId("reasoning-preview")).toBeNull();
});

it("空状态跨时段自动更新问候语", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 9, 29, 8, 59, 59));
  render(<Harness adapter={{ async *run() {} }} />);
  expect(screen.getByText("早上好")).toBeTruthy();
  act(() => vi.advanceTimersByTime(1000));
  expect(screen.getByText("上午好")).toBeTruthy();
});
