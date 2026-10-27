// @vitest-environment happy-dom
/**
 * 正式工具卡单测（§10.3，14 号票）：args/参数流式/result 直写驱动的渲染面。
 * useToolArgsStatus 需要 runtime 上下文——按卡片的真实依赖面 mock（S1 实证
 * 返回形状：{ status, argsStatus }）。
 */

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  AskCard,
  CommandCard,
  cardPhase,
  EditCard,
  GrepCard,
  ListFilesCard,
  PlanCard,
  ReadCard,
  WriteCard,
} from "./ToolCards";

vi.mock("@assistant-ui/react", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@assistant-ui/react")>();
  return {
    ...actual,
    useToolArgsStatus: () => ({ status: "running", argsStatus: "complete" }),
  };
});

afterEach(cleanup);

type PartProps = Parameters<typeof ReadCard>[0];

function props(args: Record<string, unknown>, extra: Partial<PartProps> = {}): PartProps {
  return {
    toolCallId: "c1",
    toolName: "read",
    argsText: JSON.stringify(args),
    args: args as PartProps["args"],
    result: undefined,
    isError: false,
    ...extra,
  } as PartProps;
}

function resultOf(summary: string, isError = false): { result: unknown; isError: boolean } {
  return { result: { ok: !isError, summary }, isError };
}

describe("cardPhase 状态机（纯逻辑）", () => {
  it("result 到达即 done；参数未齐为 preparing", () => {
    expect(cardPhase("running", false, false)).toBe("preparing");
    expect(cardPhase("running", false, true)).toBe("running");
    expect(cardPhase("running", true, true)).toBe("done");
    expect(cardPhase("complete", true, true)).toBe("done");
  });
});

describe("工具卡渲染", () => {
  it("read：路径与行区间", () => {
    render(<ReadCard {...props({ path: "src/main.rs", offset: 10, limit: 50 })} />);
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("src/main.rs");
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("10 起");
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("50 行");
    expect(screen.getByTestId("tool-card-c1").getAttribute("data-phase")).toBe("running");
  });

  it("write：+N 徽标按 content 行数", () => {
    render(<WriteCard {...props({ path: "new.txt", content: "a\nb\nc" })} />);
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("new.txt");
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("+3");
  });

  it("edit：+N/−N 徽标按 newText/oldText 行数", () => {
    render(<EditCard {...props({ path: "a.rs", oldText: "旧一\n旧二", newText: "新一" })} />);
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("a.rs");
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("+1");
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("−2");
  });

  it("command：命令行 monospace 展示", () => {
    render(<CommandCard {...props({ command: "cargo test --lib" })} />);
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("cargo test --lib");
  });

  it("grep：模式与过滤", () => {
    render(<GrepCard {...props({ pattern: "fn main", glob: "*.rs" })} />);
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("/fn main/");
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("*.rs");
  });

  it("list_files：缺省项目根", () => {
    render(<ListFilesCard {...props({})} />);
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("项目根");
  });

  it("plan：条目与状态渲染 + 批准语义在摘要", () => {
    render(
      <PlanCard
        {...props({
          action: "set",
          items: [
            { id: "p1", text: "读配置", status: "in_progress" },
            { id: "p2", text: "写总结", status: "pending" },
          ],
        })}
      />,
    );
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("读配置");
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("进行中");
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("写总结");
  });

  it("ask：极简状态卡（问题本体在 AskPanel）", () => {
    render(<AskCard {...props({ title: "选方案" })} />);
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("选方案");
  });

  it("result 直写：完成态与摘要行 / isError 错误态", () => {
    const done = resultOf("命令完成（2 行输出）");
    render(<CommandCard {...props({ command: "ls" }, done)} />);
    const card = screen.getByTestId("tool-card-c1");
    expect(card.getAttribute("data-phase")).toBe("done");
    expect(card.textContent).toContain("命令完成（2 行输出）");

    cleanup();
    const failed = resultOf("E_FENCE_BLOCKED：围栏拦截", true);
    render(<CommandCard {...props({ command: "rm -rf /" }, failed)} />);
    expect(screen.getByTestId("tool-card-c1").getAttribute("data-phase")).toBe("error");
  });
});
