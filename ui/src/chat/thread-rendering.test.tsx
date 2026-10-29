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
  // 运行中过程区展开（ZCode 流程）：思考摘要行 + preview 流式跟随
  // （真实 runtime 的流式推进受机器负载影响，waitFor 放宽到 3s）
  await waitFor(
    () => expect(screen.getByTestId("reasoning-preview").textContent).toBe("检查调用链"),
    { timeout: 3000 },
  );
  finish();
  await waitFor(() => expect(screen.getByText("思考")).toBeTruthy(), { timeout: 3000 });
  expect(screen.getByText("持续了 2 秒")).toBeTruthy();
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

it("错误后可继续发送，界面无再次生成入口，用户原文不被解释成 Markdown", async () => {
  const run = vi.fn(async function* () {
    yield {
      content: [{ type: "text" as const, text: "" }],
      status: {
        type: "incomplete" as const,
        reason: "error" as const,
        error: "E_NETWORK: 连接超时",
      },
    };
  });
  render(<Harness adapter={{ run }} />);
  const input = screen.getByRole("textbox");
  fireEvent.change(input, { target: { value: "# 原样显示\n**不加粗**" } });
  fireEvent.click(screen.getByRole("button", { name: "发送" }));
  await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
  expect(screen.queryByRole("heading", { name: "原样显示" })).toBeNull();
  expect(screen.getByText(/# 原样显示/).textContent).toBe("# 原样显示\n**不加粗**");
  expect(screen.queryByRole("button", { name: /再次生成|重新生成/ })).toBeNull();
  expect(screen.getByRole("button", { name: "编辑重发" })).toBeTruthy();
  fireEvent.change(input, { target: { value: "继续检查" } });
  fireEvent.click(screen.getByRole("button", { name: "发送" }));
  await waitFor(() => expect(run).toHaveBeenCalledTimes(2));
});

it("运行中过程区展开实时流动，结束 3 秒后收起为摘要行（ZCode 流程）", async () => {
  // shouldAdvanceTime：gate promise 走真实微任务，折叠的 3s 定时器可手动推进
  vi.useFakeTimers({ shouldAdvanceTime: true });
  let finish = () => {};
  const gate = new Promise<void>((resolve) => {
    finish = resolve;
  });
  const adapter: ChatModelAdapter = {
    async *run() {
      yield {
        content: [
          { type: "reasoning", text: "查看目录", status: { type: "complete" } },
          { type: "text", text: "现在深入" },
        ],
      };
      await gate;
      yield {
        content: [
          { type: "reasoning", text: "查看目录", status: { type: "complete" } },
          { type: "text", text: "现在深入" },
          { type: "text", text: "这是最终答复" },
        ],
      };
    },
  };
  render(<Harness adapter={adapter} />);
  fireEvent.click(screen.getByRole("button", { name: "梳理当前项目的目录结构与关键模块" }));
  fireEvent.click(screen.getByRole("button", { name: "发送" }));
  // 运行中：过程区全量展开——思考摘要行（剧本 reasoning 已 complete →
  // 「思考 · 持续了几秒」）与叙述段常显，无折叠摘要行
  await waitFor(() => expect(screen.getByText("现在深入")).toBeTruthy());
  expect(screen.getByText("思考")).toBeTruthy();
  expect(screen.getByText("持续了几秒")).toBeTruthy();
  expect(screen.queryByTestId("run-summary")).toBeNull();
  finish();
  await waitFor(() => expect(screen.getByText("这是最终答复")).toBeTruthy());
  // 完成态思考行：「思考 · 持续了几秒」（剧本无 timing 不虚构时长）
  expect(screen.getByText("思考")).toBeTruthy();
  expect(screen.getByText("持续了几秒")).toBeTruthy();
  // 结束 3 秒后收起：过程进摘要行，只留轮尾最终答复常显
  act(() => vi.advanceTimersByTime(3100));
  expect(screen.queryByText("现在深入")).toBeNull();
  expect(screen.queryByText("思考")).toBeNull();
  expect(screen.getByText("这是最终答复")).toBeTruthy();
  expect(screen.getByTestId("run-summary")).toBeTruthy();
  // 摘要行是 ZCode「已工作 + 时长」口径
  expect(screen.getByText(/已工作/)).toBeTruthy();
  // 点开摘要行回看完整过程
  fireEvent.click(screen.getByTestId("run-summary"));
  expect(screen.getByText("现在深入")).toBeTruthy();
  expect(screen.getByText("思考")).toBeTruthy();
  expect(screen.getByText("这是最终答复")).toBeTruthy();
});
