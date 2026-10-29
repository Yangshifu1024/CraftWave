// @vitest-environment happy-dom
/**
 * AskPanel 交互测试（§3.2 两形态，14 号票）：问卷作答 / 审批允许拒绝 →
 * resolveAsk 载荷形状。client.resolveAsk 注入 fake。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AskPayload } from "@/ipc/types";
import { useAskStore } from "@/stores/ask";
import { AskPanel } from "./AskPanel";
import { SessionScopeContext } from "./session-scope";

/** resolveAsk 的 mock：记录面板提交的载荷（真实 IPC 被 vi.mock 替换）。 */
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

/** 问卷载荷样例：一题选项单选 + 一题自由文本。 */
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

/** 审批载荷样例：命令行 + 围栏原因 + 允许/拒绝两选项。 */
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

/** 向 ask store 放入一个未决 ask（模拟后端 ask:opened 事件的效果）。 */
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

describe("AskPanel：mcpTrust 形态（08 号票）", () => {
  /** MCP 启动信任载荷样例（后端 ask:opened 的 sessionId="" 形态）。 */
  const trustPayload: AskPayload = {
    kind: "mcpTrust",
    server: "filesystem",
    transport: "stdio",
    fingerprint: "a1b2c3d4e5f60718",
    commandOrUrl: "npx -y @modelcontextprotocol/server-filesystem /tmp",
    options: [
      { id: "allow_once", label: "仅本次连接" },
      { id: "remember", label: "记住此版本" },
      { id: "deny", label: "拒绝" },
    ],
  };

  it('应用级桶（sessionId=""）的 mcpTrust 询问在任意会话上下文渲染', () => {
    // 信任询问不属任何会话：进 "" 桶；AskPanel 在 s1 作用域下也要能看到
    useAskStore.getState().open("", "trust-1", trustPayload);
    renderPanel();
    const panel = screen.getByTestId("ask-panel");
    expect(panel.getAttribute("data-kind")).toBe("mcpTrust");
    expect(panel.textContent).toContain("filesystem");
    expect(panel.textContent).toContain("npx -y @modelcontextprotocol/server-filesystem /tmp");
    expect(screen.getByTestId("mcp-trust-fingerprint").textContent).toBe("a1b2c3d4e5f60718");
  });

  it("三选项 → resolveAsk(approval) 携 allow_once/remember/deny", () => {
    useAskStore.getState().open("", "trust-1", trustPayload);
    renderPanel();
    fireEvent.click(screen.getByTestId("ask-option-remember"));
    expect(resolveAskMock).toHaveBeenCalledWith("trust-1", {
      kind: "approval",
      optionId: "remember",
    });

    resolveAskMock.mockClear();
    fireEvent.click(screen.getByTestId("ask-option-allow_once"));
    expect(resolveAskMock).toHaveBeenCalledWith("trust-1", {
      kind: "approval",
      optionId: "allow_once",
    });

    resolveAskMock.mockClear();
    fireEvent.click(screen.getByTestId("ask-option-deny"));
    expect(resolveAskMock).toHaveBeenCalledWith("trust-1", {
      kind: "approval",
      optionId: "deny",
    });
  });

  it("会话桶为空且无应用级询问时不渲染", () => {
    const { container } = renderPanel();
    expect(container.childElementCount).toBe(0);
  });

  it("应答失效（E_NOT_FOUND）时按 askId 关闭弹窗，不残留面板", async () => {
    useAskStore.getState().open("", "trust-1", trustPayload);
    resolveAskMock.mockRejectedValueOnce({ code: "E_NOT_FOUND", message: "槽不存在" });
    renderPanel();
    fireEvent.click(screen.getByTestId("ask-option-deny"));
    await waitFor(() => expect(useAskStore.getState().bySession[""]).toBeUndefined());
    expect(screen.queryByTestId("ask-panel")).toBeNull();
  });
});
