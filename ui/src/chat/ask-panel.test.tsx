// @vitest-environment happy-dom
/**
 * AskPanel 交互测试（§3.2 两形态，14 号票）：问卷作答 / 审批允许拒绝 →
 * resolveAsk 载荷形状。client.resolveAsk 注入 fake。
 */

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AskPayload } from "@/ipc/types";
import { useAskStore } from "@/stores/ask";
import { AskPanel } from "./AskPanel";
import { SessionScopeContext } from "./session-scope";

const resolveAskMock = vi.fn<(askId: string, answer: unknown) => Promise<void>>();
vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    resolveAsk: (askId: string, answer: unknown) => resolveAskMock(askId, answer),
  };
});

beforeEach(() => {
  resolveAskMock.mockReset();
  resolveAskMock.mockResolvedValue(undefined);
  useAskStore.setState({ bySession: {}, hints: {} });
});
afterEach(cleanup);

/** AskPanel 经 useSessionScope 取所属会话桶：统一以 "s1" 作用域渲染。 */
function renderPanel() {
  return render(
    <SessionScopeContext.Provider value="s1">
      <AskPanel />
    </SessionScopeContext.Provider>,
  );
}

const questionPayload: AskPayload = {
  kind: "question",
  title: "选方案",
  questions: [
    {
      id: "q1",
      text: "用哪个方案？",
      options: [
        { id: "a", label: "方案A" },
        { id: "b", label: "方案B" },
      ],
      allowFreeText: false,
    },
    {
      id: "q2",
      text: "补充说明？",
      options: undefined,
      allowFreeText: true,
    },
  ],
};

const approvalPayload: AskPayload = {
  kind: "approval",
  title: "执行命令",
  command: "echo hi > out.txt",
  fenceReason: "命令含写信号，需确认",
  options: [
    { id: "allow_once", label: "允许", recommended: true, mode: "allow_once" },
    { id: "deny", label: "拒绝", mode: "deny" },
  ],
};

function openPanel(payload: AskPayload) {
  useAskStore.getState().open("s1", "ask-1", payload);
}

describe("AskPanel：question 形态", () => {
  it("无未决 ask 时不渲染", () => {
    const { container } = renderPanel();
    expect(container.childElementCount).toBe(0);
    expect(screen.queryByTestId("ask-panel")).toBeNull();
  });

  it("选项点选 + 自由文本 → 提交 answers", () => {
    openPanel(questionPayload);
    renderPanel();
    expect(screen.getByTestId("ask-panel").getAttribute("data-kind")).toBe("question");

    const submit = screen.getByText("提交回答");
    expect((submit as HTMLButtonElement).disabled).toBe(true);

    fireEvent.click(screen.getByText("方案A"));
    fireEvent.change(screen.getByPlaceholderText("自由输入…"), {
      target: { value: "注意并发" },
    });
    expect((screen.getByText("提交回答") as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(screen.getByText("提交回答"));

    expect(resolveAskMock).toHaveBeenCalledWith("ask-1", {
      kind: "question",
      answers: { q1: "a", q2: "注意并发" },
    });
  });
});

describe("AskPanel：approval 形态", () => {
  it("命令 + 围栏原因 + diff 预览；允许/拒绝载荷", () => {
    openPanel({
      ...approvalPayload,
      diff: {
        path: "src/a.rs",
        hunks: [
          {
            header: "新文件",
            lines: [
              { kind: "add", text: "fn main() {}" },
              { kind: "del", text: "旧实现" },
            ],
          },
        ],
      },
    });
    renderPanel();
    expect(screen.getByTestId("ask-panel").getAttribute("data-kind")).toBe("approval");
    expect(screen.getByTestId("ask-panel").textContent).toContain("echo hi > out.txt");
    expect(screen.getByTestId("ask-panel").textContent).toContain("写信号");
    expect(screen.getByTestId("ask-diff").textContent).toContain("src/a.rs");
    expect(screen.getByTestId("ask-diff").textContent).toContain("+ fn main() {}");
    expect(screen.getByTestId("ask-diff").textContent).toContain("− 旧实现");

    fireEvent.click(screen.getByTestId("ask-option-allow_once"));
    expect(resolveAskMock).toHaveBeenCalledWith("ask-1", {
      kind: "approval",
      optionId: "allow_once",
    });

    resolveAskMock.mockClear();
    fireEvent.click(screen.getByTestId("ask-option-deny"));
    expect(resolveAskMock).toHaveBeenCalledWith("ask-1", {
      kind: "approval",
      optionId: "deny",
    });
  });
});
