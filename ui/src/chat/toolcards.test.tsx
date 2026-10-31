// @vitest-environment happy-dom
/**
 * 正式工具卡单测：args/参数流式/result 直写驱动的渲染面。
 * useToolArgsStatus 需要 runtime 上下文——按卡片的真实依赖面 mock（S1 实证
 * 返回形状：{ status, argsStatus }）。
 * 精简版：每种卡留代表 1 例；同形状摘要断言循环表驱动；同卡多状态分段断言。
 */

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { ReactElement } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { usePlanStateStore } from "@/chat/plan-state";
import { SessionScopeContext } from "@/chat/session-scope";
import {
  AskCard,
  CommandCard,
  EditCard,
  GrepCard,
  HttpRequestCard,
  ListFilesCard,
  McpFallbackCard,
  PlanCard,
  ReadCard,
  toolCardExpandedMemory,
  WebFetchCard,
  WebSearchCard,
  WriteCard,
} from "./ToolCards";

const toolStatus = vi.hoisted(() => ({ status: "running", argsStatus: "complete" }));

vi.mock("@assistant-ui/react", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@assistant-ui/react")>();
  return {
    ...actual,
    useToolArgsStatus: () => toolStatus,
  };
});

/** 边车数据面（web 两卡终态按 callId 拉边车）：用例内改写返回值。 */
const loadToolOutcomes = vi.hoisted(() => vi.fn(async () => ({}) as Record<string, unknown>));
vi.mock("@/ipc/client", () => ({
  loadToolOutcomes: loadToolOutcomes,
}));

afterEach(() => {
  cleanup();
  // 展开记忆是模块级 Map（跨用例持久），不清会污染后续用例的初值
  toolCardExpandedMemory.clear();
  toolStatus.status = "running";
  loadToolOutcomes.mockClear();
});

/** 卡片 props 的类型来源（以 ReadCard 的首参推导）。 */
type PartProps = Parameters<typeof ReadCard>[0];

/** 构造一份卡片 props：默认工具 read、参数已完整，extra 覆盖 result 等。 */
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

/** 构造 result/isError 覆盖项（模拟 result 直写的完成/错误终态）。 */
function resultOf(summary: string, isError = false): { result: unknown; isError: boolean } {
  return { result: { ok: !isError, summary }, isError };
}

describe("工具卡渲染（每种卡代表 1 例）", () => {
  /** 摘要行卡代表（渲染即见摘要文本，同形状循环断言）。 */
  const summaryCases: { name: string; node: ReactElement; expects: string[] }[] = [
    {
      name: "read：路径拆分（文件名 + 目录）",
      node: <ReadCard {...props({ path: "src/main.rs", offset: 10, limit: 50 })} />,
      expects: ["main.rs", "src/"],
    },
    {
      name: "write：+N 徽标按 content 行数",
      node: <WriteCard {...props({ path: "new.txt", content: "a\nb\nc" })} />,
      expects: ["new.txt", "+3"],
    },
    {
      name: "edit：+N/−N 统计按 newText/oldText 行数（ZCode 纯彩字口径）",
      node: <EditCard {...props({ path: "a.rs", oldText: "旧一\n旧二", newText: "新一" })} />,
      expects: ["a.rs", "+1", "-2"],
    },
    {
      name: "grep：模式与过滤",
      node: <GrepCard {...props({ pattern: "fn main", glob: "*.rs" })} />,
      expects: ["/fn main/", "*.rs"],
    },
    {
      name: "list_files：缺省项目根",
      node: <ListFilesCard {...props({})} />,
      expects: ["项目根"],
    },
    {
      name: "ask：极简状态卡（问题本体在 AskPanel）",
      node: <AskCard {...props({ title: "选方案" })} />,
      expects: ["选方案"],
    },
  ];

  it("摘要行卡：六种卡渲染即见摘要文本，运行态 data-phase=running", () => {
    for (const { node, expects } of summaryCases) {
      render(node);
      const card = screen.getByTestId("tool-card-c1");
      for (const text of expects) {
        expect(card.textContent).toContain(text);
      }
      expect(card.getAttribute("data-phase")).toBe("running");
      cleanup();
    }
  });

  it("command 卡状态机：取消/等待确认不显进度；完成摘要入详情；失败默认展开", () => {
    // 取消（incomplete）：无运行进度，展开可读完整调用参数
    toolStatus.status = "incomplete";
    const command = "echo 第一行\necho 第二行";
    const view = render(<CommandCard {...props({ command })} />);
    const card = screen.getByTestId("tool-card-c1");
    expect(card.textContent).toContain("已中断");
    expect(card.querySelector('[data-testid="tool-spinner"]')).toBeNull();
    fireEvent.click(card.querySelector("button")!);
    expect(card.querySelector("pre")?.textContent).toBe(JSON.stringify({ command }));

    // 等待确认（requires-action）：同样无进度条
    toolStatus.status = "requires-action";
    view.rerender(<CommandCard {...props({ command })} />);
    expect(card.textContent).toContain("等待确认");
    expect(card.querySelector(".cw-progress")).toBeNull();
    view.unmount();

    // 完成：摘要入详情（默认收起，展开后可见）
    toolStatus.status = "running";
    toolCardExpandedMemory.clear();
    const done = render(
      <CommandCard {...props({ command: "ls" }, resultOf("命令完成（2 行输出）"))} />,
    );
    const doneCard = screen.getByTestId("tool-card-c1");
    expect(doneCard.getAttribute("data-phase")).toBe("done");
    fireEvent.click(doneCard.querySelector("button")!);
    expect(doneCard.textContent).toContain("命令完成（2 行输出）");
    done.unmount();

    // 失败：错误态默认展开，失败原因无需点击即可见
    render(
      <CommandCard
        {...props({ command: "rm -rf /" }, resultOf("E_FENCE_BLOCKED：围栏拦截", true))}
      />,
    );
    const failedCard = screen.getByTestId("tool-card-c1");
    expect(failedCard.getAttribute("data-phase")).toBe("error");
    expect(failedCard.textContent).toContain("E_FENCE_BLOCKED");
  });

  it("plan 卡：摘要完成数 + 展开条目；已批准基线与范围声明（plan-state 同源）", () => {
    // 预置所属会话的批准态：基线 2 条
    usePlanStateStore.getState().applyUpdate("sess-p", {
      items: [],
      paths: ["src/a.rs"],
      approved: true,
      baselinePaths: ["src/a.rs", "tests/"],
    });
    render(
      <SessionScopeContext.Provider value="sess-p">
        <PlanCard
          {...props({
            action: "set",
            items: [
              { id: "p1", text: "读配置", status: "in_progress" },
              { id: "p2", text: "写总结", status: "pending" },
            ],
            paths: ["src/a.rs", "tests/"],
          })}
        />
      </SessionScopeContext.Provider>,
    );
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("0/2");
    fireEvent.click(screen.getByTestId("tool-card-c1").querySelector("button")!);
    const card = screen.getByTestId("tool-card-c1");
    expect(card.textContent).toContain("读配置");
    expect(card.textContent).toContain("进行中");
    expect(card.textContent).toContain("已批准 · 基线 2 条");
    expect(card.textContent).toContain("范围：src/a.rs、tests/");
    usePlanStateStore.setState({ bySession: {} });
  });

  it("web 工具卡：web_fetch 拉边车渲染标题/正文预览；http_request 徽标与 spill 路径", async () => {
    // web_fetch：终态拉边车（默认收起，展开后正文预览前 50 行）
    const content = Array.from({ length: 60 }, (_, i) => `第 ${i + 1} 行`).join("\n");
    loadToolOutcomes.mockResolvedValue({
      c1: {
        ok: true,
        uiSummary: "已抓取",
        data: { url: "https://example.com", title: "示例标题", content, length: 600 },
      },
    });
    const fetched = render(
      <WebFetchCard {...props({ url: "https://example.com" }, resultOf("已抓取"))} />,
    );
    const fetchCard = screen.getByTestId("tool-card-c1");
    expect(fetchCard.textContent).toContain("https://example.com");
    fireEvent.click(fetchCard.querySelector("button")!);
    await vi.waitFor(() => expect(fetchCard.textContent).toContain("示例标题"));
    expect(fetchCard.textContent).toContain("正文 600 字符");
    const preview = fetchCard.querySelector("details div");
    expect(preview?.textContent).toContain("第 1 行");
    expect(preview?.textContent).toContain("预览前 50 行");
    expect(preview?.textContent).not.toContain("第 60 行");
    fetched.unmount();

    // http_request：method 徽标大写归一 + 状态码徽标 + 边车响应头/body 折叠与 spill 路径
    toolCardExpandedMemory.clear();
    loadToolOutcomes.mockResolvedValue({
      c1: {
        ok: true,
        uiSummary: "HTTP 200",
        data: {
          status: 200,
          headers: { "content-type": "text/html", "x-probe": "1" },
          body: "响应体文本",
          bodyTruncated: true,
          spillPath: "/data/logs/spill/call-test.txt",
        },
      },
    });
    render(
      <HttpRequestCard
        {...props(
          { method: "post", url: "https://api.example.com/v1/submit" },
          resultOf("HTTP 200"),
        )}
      />,
    );
    const httpCard = screen.getByTestId("tool-card-c1");
    // method 徽标在详情区（默认收起，先展开再断言）；状态码徽标在摘要行
    fireEvent.click(httpCard.querySelector("button")!);
    expect(httpCard.textContent).toContain("POST");
    await vi.waitFor(() => {
      expect(httpCard.textContent).toContain("200");
      expect(httpCard.textContent).toContain("content-type");
    });
    expect(httpCard.textContent).toContain("响应体文本");
    expect(httpCard.textContent).toContain("尾部，全量已落盘");
    expect(httpCard.textContent).toContain("/data/logs/spill/call-test.txt");
  });

  it("web_search 卡：来源通道徽标（本地/抓取/云）+ 结果列表/空结果", async () => {
    const cases: {
      channel: string;
      badge: string;
      results: { title: string; url: string; snippet: string }[];
    }[] = [
      {
        channel: "searxng",
        badge: "本地",
        results: [
          {
            title: "Tokio 教程",
            url: "https://tokio.rs/tokio/tutorial",
            snippet: "异步 Rust 入门",
          },
        ],
      },
      { channel: "ddg", badge: "抓取", results: [] },
      {
        channel: "tavily",
        badge: "云",
        results: [{ title: "T", url: "https://t.dev", snippet: "s" }],
      },
    ];
    for (const { channel, badge, results } of cases) {
      toolCardExpandedMemory.clear();
      loadToolOutcomes.mockResolvedValue({
        c1: {
          ok: true,
          uiSummary: `已搜索（${badge}通道，${results.length} 条结果）`,
          data: { query: "rust async", channel, count: results.length, results },
        },
      });
      render(<WebSearchCard {...props({ query: "rust async" }, resultOf("已搜索"))} />);
      const card = screen.getByTestId("tool-card-c1");
      // 徽标在摘要行，展开前可见
      await vi.waitFor(() =>
        expect(screen.getByTestId("web-search-channel").textContent).toBe(badge),
      );
      fireEvent.click(card.querySelector("button")!);
      if (results.length === 0) {
        await vi.waitFor(() => expect(card.textContent).toContain("无结果"));
      } else {
        await vi.waitFor(() => expect(card.textContent).toContain(`${results.length} 条结果`));
        const list = screen.getByTestId("web-search-results");
        expect(list.querySelectorAll("li")).toHaveLength(results.length);
        expect(list.textContent).toContain(results[0]!.title);
        expect(list.textContent).toContain(results[0]!.snippet);
        if (results[0]!.url) {
          expect(list.querySelector("a")!.getAttribute("href")).toBe(results[0]!.url);
        }
      }
      cleanup();
    }
  });

  it("MCP 兜底卡：工具名摘要 + 展开参数 JSON；错误态默认展开显示失败原因", () => {
    // MCP 动态工具的归一名运行时才定——兜底卡直接以 part 的 toolName 为摘要
    render(
      <McpFallbackCard
        {...props({ query: "hi" }, { toolName: "server1__echo" } as Partial<PartProps>)}
      />,
    );
    const card = screen.getByTestId("tool-card-c1");
    expect(card.textContent).toContain("MCP 工具");
    expect(card.textContent).toContain("server1__echo");
    fireEvent.click(card.querySelector("button")!);
    expect(card.querySelector("pre")?.textContent).toBe(JSON.stringify({ query: "hi" }));
    cleanup();

    render(
      <McpFallbackCard
        {...props({}, {
          toolName: "fs__write_file",
          ...resultOf("E_MCP_RESULT_UNKNOWN：结果未知（可能已执行）", true),
        } as Partial<PartProps>)}
      />,
    );
    const failed = screen.getByTestId("tool-card-c1");
    expect(failed.getAttribute("data-phase")).toBe("error");
    // 错误态默认展开：失败原因无需点击即可见（结果未知的行动指引可见）
    expect(failed.textContent).toContain("E_MCP_RESULT_UNKNOWN");
  });
});
