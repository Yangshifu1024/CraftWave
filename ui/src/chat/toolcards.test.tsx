// @vitest-environment happy-dom
/**
 * 正式工具卡单测（§10.3，14 号票）：args/参数流式/result 直写驱动的渲染面。
 * useToolArgsStatus 需要 runtime 上下文——按卡片的真实依赖面 mock（S1 实证
 * 返回形状：{ status, argsStatus }）。
 */

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { EMPTY_PLAN_STATE, usePlanStateStore } from "@/chat/plan-state";
import { SessionScopeContext } from "@/chat/session-scope";
import {
  AskCard,
  CommandCard,
  cardPhase,
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

/** 边车数据面（12 号票两卡终态按 callId 拉边车）：用例内改写返回值。 */
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

describe("cardPhase 状态机（纯逻辑）", () => {
  it("result 到达即 done；参数未齐为 preparing", () => {
    expect(cardPhase("running", false, false)).toBe("preparing");
    expect(cardPhase("running", false, true)).toBe("running");
    expect(cardPhase("running", true, true)).toBe("done");
    expect(cardPhase("complete", true, true)).toBe("done");
    expect(cardPhase("incomplete", false, true)).toBe("cancelled");
    expect(cardPhase("complete", false, true)).toBe("cancelled");
    expect(cardPhase("requires-action", false, true)).toBe("awaiting");
  });
});

it("取消和等待确认不再显示运行进度，展开后可读取完整调用参数", () => {
  toolStatus.status = "incomplete";
  const command = "echo 第一行\necho 第二行";
  const { rerender } = render(<CommandCard {...props({ command })} />);
  const card = screen.getByTestId("tool-card-c1");
  expect(card.textContent).toContain("已中断");
  expect(card.querySelector('[data-testid="tool-spinner"]')).toBeNull();
  fireEvent.click(card.querySelector("button")!);
  expect(card.querySelector("pre")?.textContent).toBe(JSON.stringify({ command }));
  toolStatus.status = "requires-action";
  rerender(<CommandCard {...props({ command })} />);
  expect(card.textContent).toContain("等待确认");
  expect(card.querySelector(".cw-progress")).toBeNull();
});

describe("工具卡渲染", () => {
  /** 参考样式改版后卡片默认收为单行摘要；点击摘要行展开详情再断言。 */
  function expandCard() {
    fireEvent.click(
      screen.getByTestId("tool-card-c1").querySelector("button") as HTMLButtonElement,
    );
  }

  it("read：摘要行路径拆分（蓝文件名 + 灰目录）+ 展开后行区间", () => {
    render(<ReadCard {...props({ path: "src/main.rs", offset: 10, limit: 50 })} />);
    // ZCode 工具行同款：文件名与目录拆开渲染（textContent 相连无分隔符）
    const card = screen.getByTestId("tool-card-c1");
    expect(card.textContent).toContain("main.rs");
    expect(card.textContent).toContain("src/");
    expect(card.getAttribute("data-phase")).toBe("running");
    expandCard();
    expect(card.textContent).toContain("10 起");
    expect(card.textContent).toContain("50 行");
  });

  it("write：+N 徽标按 content 行数", () => {
    render(<WriteCard {...props({ path: "new.txt", content: "a\nb\nc" })} />);
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("new.txt");
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("+3");
  });

  it("edit：+N/−N 统计按 newText/oldText 行数（ZCode 纯彩字口径）", () => {
    render(<EditCard {...props({ path: "a.rs", oldText: "旧一\n旧二", newText: "新一" })} />);
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("a.rs");
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("+1");
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("-2");
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

  it("plan：摘要完成数统计 + 展开后条目与状态", () => {
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
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("0/2");
    expandCard();
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("读配置");
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("进行中");
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("写总结");
  });

  it("plan：25 号票——范围声明展示与已批准基线标记（plan-state 同源）", () => {
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
            items: [{ id: "p1", text: "写 a", status: "in_progress" }],
            paths: ["src/a.rs", "tests/"],
          })}
        />
        ,
      </SessionScopeContext.Provider>,
    );
    expandCard();
    const card = screen.getByTestId("tool-card-c1");
    expect(card.textContent).toContain("已批准 · 基线 2 条");
    expect(card.textContent).toContain("范围：src/a.rs、tests/");
    // 缺桶（会话无计划态）：不展示基线标记
    cleanup();
    render(
      <SessionScopeContext.Provider value="sess-none">
        <PlanCard
          {...props({
            action: "set",
            items: [{ id: "p1", text: "写 a", status: "in_progress" }],
          })}
        />
      </SessionScopeContext.Provider>,
    );
    expect(screen.getByTestId("tool-card-c1").textContent).not.toContain("已批准");
    usePlanStateStore.setState({ bySession: {} });
  });

  it("plan：已批准但未声明范围时提示首写确认", () => {
    usePlanStateStore.getState().applyUpdate("sess-e", {
      ...EMPTY_PLAN_STATE,
      approved: true,
    });
    render(
      <SessionScopeContext.Provider value="sess-e">
        <PlanCard
          {...props({ action: "set", items: [{ id: "p1", text: "任务", status: "pending" }] })}
        />
      </SessionScopeContext.Provider>,
    );
    expandCard();
    expect(screen.getByTestId("plan-card-baseline").textContent).toContain("首次写入会请求确认");
    usePlanStateStore.setState({ bySession: {} });
  });

  it("ask：极简状态卡（问题本体在 AskPanel）", () => {
    render(<AskCard {...props({ title: "选方案" })} />);
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("选方案");
  });

  it("MCP 兜底卡（08 号票）：工具名摘要 + 展开后参数 JSON 折叠", () => {
    // MCP 动态工具的归一名运行时才定——兜底卡直接以 part 的 toolName 为摘要
    render(
      <McpFallbackCard
        {...props({ query: "hi" }, { toolName: "server1__echo" } as Partial<PartProps>)}
      />,
    );
    const card = screen.getByTestId("tool-card-c1");
    expect(card.textContent).toContain("MCP 工具");
    expect(card.textContent).toContain("server1__echo");
    // 展开详情：调用参数 JSON 原文（兜底卡无专属排版，参数折叠即详情主体）
    fireEvent.click(card.querySelector("button")!);
    expect(card.querySelector("pre")?.textContent).toBe(JSON.stringify({ query: "hi" }));
  });

  it("MCP 兜底卡：错误态默认展开并显示失败原因", () => {
    render(
      <McpFallbackCard
        {...props({}, {
          toolName: "fs__write_file",
          ...resultOf("E_MCP_RESULT_UNKNOWN：结果未知（可能已执行）", true),
        } as Partial<PartProps>)}
      />,
    );
    const card = screen.getByTestId("tool-card-c1");
    expect(card.getAttribute("data-phase")).toBe("error");
    // 错误态默认展开：失败原因无需点击即可见（Q14 结果未知的行动指引可见）
    expect(card.textContent).toContain("E_MCP_RESULT_UNKNOWN");
  });

  it("result 直写：完成态摘要在详情 / isError 错误态默认展开", () => {
    const done = resultOf("命令完成（2 行输出）");
    render(<CommandCard {...props({ command: "ls" }, done)} />);
    const card = screen.getByTestId("tool-card-c1");
    expect(card.getAttribute("data-phase")).toBe("done");
    expandCard();
    expect(card.textContent).toContain("命令完成（2 行输出）");

    cleanup();
    const failed = resultOf("E_FENCE_BLOCKED：围栏拦截", true);
    render(<CommandCard {...props({ command: "rm -rf /" }, failed)} />);
    expect(screen.getByTestId("tool-card-c1").getAttribute("data-phase")).toBe("error");
    // 错误态默认展开：失败原因无需点击即可见
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("E_FENCE_BLOCKED");
  });
});

describe("复刻回归", () => {
  it("运行态使用单色 SVG 和扫光标签；失败到达后自动展开", () => {
    const { rerender } = render(<CommandCard {...props({ command: "cargo test" })} />);
    const card = screen.getByTestId("tool-card-c1");
    expect(card.querySelector("svg.lucide-square-terminal")).toBeTruthy();
    expect(card.querySelector(".cw-shimmer-text")?.textContent).toBe("正在执行");
    expect(card.querySelector("button")?.getAttribute("aria-expanded")).toBe("false");
    rerender(
      <CommandCard {...props({ command: "cargo test" }, resultOf("E_COMMAND: 退出码 1", true))} />,
    );
    expect(card.querySelector("button")?.getAttribute("aria-expanded")).toBe("true");
    expect(card.textContent).toContain("E_COMMAND: 退出码 1");
    expect(card.querySelector(".cw-shimmer-text")).toBeNull();
  });

  it("用户已手动收起的失败卡不会被后续更新反复打开", () => {
    const { rerender } = render(<CommandCard {...props({ command: "ls" })} />);
    const toggle = screen.getByTestId("tool-card-c1").querySelector("button")!;
    fireEvent.click(toggle);
    fireEvent.click(toggle);
    rerender(<CommandCard {...props({ command: "ls" }, resultOf("失败", true))} />);
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
  });
});

describe("web 工具卡（12 号票）", () => {
  it("web_fetch：URL 摘要 + 终态拉边车渲染标题/正文预览（默认收起，前 50 行）", async () => {
    const content = Array.from({ length: 60 }, (_, i) => `第 ${i + 1} 行`).join("\n");
    loadToolOutcomes.mockResolvedValue({
      c1: {
        ok: true,
        uiSummary: "已抓取",
        data: { url: "https://example.com", title: "示例标题", content, length: 600 },
      },
    });
    render(<WebFetchCard {...props({ url: "https://example.com" }, resultOf("已抓取"))} />);
    const card = screen.getByTestId("tool-card-c1");
    expect(card.textContent).toContain("https://example.com");
    // 详情区默认收起（ToolCardShell：收起不渲染 children）——先展开再断言
    fireEvent.click(card.querySelector("button")!);
    // 边车异步到达：标题渲染 + 正文折叠预览存在
    await vi.waitFor(() => expect(card.textContent).toContain("示例标题"));
    expect(card.textContent).toContain("正文 600 字符");
    const preview = card.querySelector("details div");
    expect(preview?.textContent).toContain("第 1 行");
    // 前 50 行截断提示（60 行正文 → 预览含省略提示、不含第 60 行）
    expect(preview?.textContent).toContain("预览前 50 行");
    expect(preview?.textContent).not.toContain("第 60 行");
  });

  it("web_fetch：失败态（SSRF 拒绝）默认展开显示错误类别", () => {
    render(
      <WebFetchCard
        {...props(
          { url: "http://127.0.0.1:6379/" },
          resultOf("E_SSRF_BLOCKED：SSRF 防护拦截（回环地址）", true),
        )}
      />,
    );
    const card = screen.getByTestId("tool-card-c1");
    expect(card.getAttribute("data-phase")).toBe("error");
    expect(card.textContent).toContain("E_SSRF_BLOCKED");
    expect(card.textContent).toContain("回环地址");
  });

  it("http_request：method 徽标 + 状态码徽标 + 展开响应头/body 折叠与 spill 路径", async () => {
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
    const card = screen.getByTestId("tool-card-c1");
    // method 徽标大写归一 + 状态码徽标（2xx 绿；状态码徽标在摘要行，
    // method 徽标在详情区——详情区默认收起，先展开再断言）
    fireEvent.click(card.querySelector("button")!);
    expect(card.textContent).toContain("POST");
    await vi.waitFor(() => {
      expect(card.textContent).toContain("200");
    });
    // 展开详情：边车到达后响应头 JSON 折叠 + body 折叠 + spill 路径
    await vi.waitFor(() => {
      expect(card.textContent).toContain("content-type");
    });
    expect(card.textContent).toContain("响应体文本");
    expect(card.textContent).toContain("尾部，全量已落盘");
    expect(card.textContent).toContain("/data/logs/spill/call-test.txt");
  });

  it("http_request：5xx 状态码徽标着红色（错误分级配色）", async () => {
    loadToolOutcomes.mockResolvedValue({
      c1: { ok: true, uiSummary: "HTTP 503", data: { status: 503, headers: {}, body: "" } },
    });
    render(
      <HttpRequestCard
        {...props({ method: "GET", url: "https://api.example.com/x" }, resultOf("HTTP 503"))}
      />,
    );
    const card = screen.getByTestId("tool-card-c1");
    await vi.waitFor(() => {
      expect(card.textContent).toContain("503");
    });
  });
});

describe("web_search 工具卡（13 号票）", () => {
  it("来源通道徽标（本地/抓取/云）+ 结果列表（标题链接 + 摘要）", async () => {
    loadToolOutcomes.mockResolvedValue({
      c1: {
        ok: true,
        uiSummary: "已搜索「rust async」（本地通道，2 条结果）",
        data: {
          query: "rust async",
          channel: "searxng",
          count: 2,
          results: [
            {
              title: "Tokio 教程",
              url: "https://tokio.rs/tokio/tutorial",
              snippet: "异步 Rust 入门",
            },
            {
              title: "async book",
              url: "https://rust-lang.github.io/async-book/",
              snippet: "官方异步书",
            },
          ],
        },
      },
    });
    render(<WebSearchCard {...props({ query: "rust async" }, resultOf("已搜索"))} />);
    const card = screen.getByTestId("tool-card-c1");
    // 摘要行带查询词 + 来源通道徽标（本地）——徽标在摘要行，展开前可见
    expect(card.textContent).toContain("rust async");
    await vi.waitFor(() =>
      expect(screen.getByTestId("web-search-channel").textContent).toBe("本地"),
    );
    // 详情区默认收起：展开后结果列表出现（标题链接 + URL + 摘要）
    fireEvent.click(card.querySelector("button")!);
    await vi.waitFor(() => expect(card.textContent).toContain("2 条结果"));
    const list = screen.getByTestId("web-search-results");
    expect(list.querySelectorAll("li")).toHaveLength(2);
    expect(list.textContent).toContain("Tokio 教程");
    const link = list.querySelector("a")!;
    expect(link.getAttribute("href")).toBe("https://tokio.rs/tokio/tutorial");
    expect(list.textContent).toContain("异步 Rust 入门");
  });

  it("抓取通道徽标 + 空结果显示「无结果」", async () => {
    loadToolOutcomes.mockResolvedValue({
      c1: {
        ok: true,
        uiSummary: "已搜索（抓取通道，0 条结果）",
        data: { query: "xyzzy", channel: "ddg", count: 0, results: [] },
      },
    });
    render(<WebSearchCard {...props({ query: "xyzzy" }, resultOf("已搜索"))} />);
    const card = screen.getByTestId("tool-card-c1");
    await vi.waitFor(() =>
      expect(screen.getByTestId("web-search-channel").textContent).toBe("抓取"),
    );
    fireEvent.click(card.querySelector("button")!);
    await vi.waitFor(() => expect(card.textContent).toContain("无结果"));
  });

  it("云通道徽标（tavily → 云）", async () => {
    loadToolOutcomes.mockResolvedValue({
      c1: {
        ok: true,
        uiSummary: "已搜索（云通道，1 条结果）",
        data: {
          query: "q",
          channel: "tavily",
          count: 1,
          results: [{ title: "T", url: "https://t.dev", snippet: "s" }],
        },
      },
    });
    render(<WebSearchCard {...props({ query: "q" }, resultOf("已搜索"))} />);
    await vi.waitFor(() => expect(screen.getByTestId("web-search-channel").textContent).toBe("云"));
  });

  it("全败错误态默认展开：逐层通道尝试记录可见", async () => {
    loadToolOutcomes.mockResolvedValue({
      c1: {
        ok: false,
        uiSummary: "网络搜索失败（全部通道不可用）",
        detail: {
          attempts: [
            { channel: "searxng", label: "本地 SearXNG", tried: false },
            {
              channel: "ddg",
              label: "内置抓取（DuckDuckGo）",
              tried: true,
              error: "疑似改版",
            },
            {
              channel: "tavily",
              label: "云搜索（Tavily）",
              tried: true,
              error: "Tavily（Key 无效（401））：Invalid API key",
            },
          ],
        },
      },
    });
    render(
      <WebSearchCard
        {...props({ query: "rust" }, resultOf("E_SEARCH_UNAVAILABLE：全部通道失败", true))}
      />,
    );
    const card = screen.getByTestId("tool-card-c1");
    expect(card.getAttribute("data-phase")).toBe("error");
    expect(card.textContent).toContain("E_SEARCH_UNAVAILABLE");
    // 逐层尝试：等边车 detail 异步到达后展开「通道尝试记录」折叠块
    //（详情区第一个 details 是「调用参数」，按 summary 文本定位目标块）
    await vi.waitFor(() => {
      expect(
        [...card.querySelectorAll("details")].some((d) =>
          d.querySelector("summary")?.textContent?.includes("通道尝试记录"),
        ),
      ).toBe(true);
    });
    const details = [...card.querySelectorAll("details")].find((d) =>
      d.querySelector("summary")?.textContent?.includes("通道尝试记录"),
    )!;
    fireEvent.click(details.querySelector("summary")!);
    expect(details.textContent).toContain("本地 SearXNG");
    expect(details.textContent).toContain("未启用/未配置");
    expect(details.textContent).toContain("内置抓取（DuckDuckGo）：疑似改版");
    expect(details.textContent).toContain("Key 无效");
  });
});

it("历史工具结果是纯字符串时仍显示失败原因并可复制", async () => {
  const copy = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
  render(
    <CommandCard
      {...props({ command: "ls" }, { result: "E_COMMAND: 目录不存在", isError: true })}
    />,
  );
  expect(screen.getByText("E_COMMAND: 目录不存在")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "复制错误" }));
  expect(copy).toHaveBeenCalledWith("E_COMMAND: 目录不存在");
  copy.mockRestore();
});

describe("read 文档态（16 号票）", () => {
  /** pdf 文档卡的边车 data（read 文档管线结构化输出）。 */
  const pdfOutcome = {
    ok: true,
    uiSummary: "读取 docs/report.pdf（pdf：共 12 页，输出第 2–2 页）",
    data: {
      kind: "pdf",
      path: "docs/report.pdf",
      pageCount: 12,
      pages: [2, 2],
      content: "--- 第 2 页 ---\n正文内容",
      truncated: false,
    },
  };

  it("扩展名徽标（pdf）在摘要行 + 页数摘要 + 内容折叠预览", async () => {
    loadToolOutcomes.mockResolvedValue({ c1: pdfOutcome });
    render(<ReadCard {...props({ path: "docs/report.pdf", page: 2 }, resultOf("已读取"))} />);
    const card = screen.getByTestId("tool-card-c1");
    // 徽标与页码参数（摘要行，展开前可见）
    expect(screen.getByTestId("doc-format-badge").textContent).toBe("pdf");
    fireEvent.click(card.querySelector("button")!);
    await vi.waitFor(() => expect(card.textContent).toContain("共 12 页"));
    expect(card.textContent).toContain("第 2 页");
    // 内容折叠预览存在
    const preview = [...card.querySelectorAll("details")].find((d) =>
      d.querySelector("summary")?.textContent?.includes("内容预览"),
    );
    expect(preview?.textContent).toContain("--- 第 2 页 ---");
  });

  it("xlsx 徽标 + sheet 数摘要", async () => {
    loadToolOutcomes.mockResolvedValue({
      c1: {
        ok: true,
        uiSummary: "读取 docs/data.xlsx",
        data: {
          kind: "xlsx",
          path: "docs/data.xlsx",
          sheets: ["销售数据", "汇总", "说明"],
          selectedSheets: ["销售数据"],
          rowCount: 4,
          content: "## 销售数据\n| 产品 | 销售额 |",
          truncated: true,
        },
      },
    });
    render(<ReadCard {...props({ path: "docs/data.xlsx" }, resultOf("已读取"))} />);
    expect(screen.getByTestId("doc-format-badge").textContent).toBe("xlsx");
    fireEvent.click(screen.getByTestId("tool-card-c1").querySelector("button")!);
    const card = screen.getByTestId("tool-card-c1");
    await vi.waitFor(() => expect(card.textContent).toContain("3 个 sheet"));
    // truncated 提示行（提示模型继续，非按钮）
    expect(screen.getByTestId("doc-resume-hint").textContent).toContain("继续读取");
  });

  it("warnings 黄条（降级解析提示仅 UI）+ 预览前 80 行截断", async () => {
    const lines = Array.from({ length: 120 }, (_, i) => `第 ${i + 1} 行`).join("\n");
    loadToolOutcomes.mockResolvedValue({
      c1: {
        ok: true,
        uiSummary: "读取 docs/a.docx（docx：3 段、2 个表格）",
        detail: {
          warnings: ["降级解析：docx-rs 读取失败（zip 解析错误），已用 zip+quick-xml 兜底解析"],
        },
        data: {
          kind: "docx",
          path: "docs/a.docx",
          paragraphCount: 3,
          tableCount: 2,
          content: lines,
          truncated: false,
        },
      },
    });
    render(<ReadCard {...props({ path: "docs/a.docx" }, resultOf("已读取"))} />);
    fireEvent.click(screen.getByTestId("tool-card-c1").querySelector("button")!);
    const card = await vi.waitFor(() => {
      const warnings = screen.getByTestId("doc-warnings");
      expect(warnings.textContent).toContain("降级解析");
      return screen.getByTestId("tool-card-c1");
    });
    expect(card.textContent).toContain("3 段 · 2 个表格");
    // 预览前 80 行：第 80 行在内、第 81 行不在
    const preview = [...card.querySelectorAll("details")].find((d) =>
      d.querySelector("summary")?.textContent?.includes("内容预览"),
    );
    expect(preview?.textContent).toContain("第 80 行");
    expect(preview?.textContent).not.toContain("第 81 行");
  });

  it("普通文本文件不进入文档态（无徽标）", () => {
    render(<ReadCard {...props({ path: "src/main.rs" }, resultOf("已读取"))} />);
    expect(screen.getByTestId("tool-card-c1").textContent).toContain("main.rs");
    expect(screen.queryByTestId("doc-format-badge")).toBeNull();
  });
});
