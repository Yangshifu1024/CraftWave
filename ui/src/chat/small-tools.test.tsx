// @vitest-environment happy-dom
/**
 * 小工具包前端测试（四张工具卡的 UI 面）：
 * 1. 四张工具卡的渲染面（徽标/列表/结果行/预览折叠）；
 * 2. HtmlPreview 沙箱逃逸向量断言——iframe 的 sandbox 属性契约（只带
 *    allow-scripts，无 allow-same-origin/allow-top-navigation/allow-forms）
 *    与预览协议地址（静态契约面。跨框动态逃逸行为——预览页脚本访问
 *    parent/top/localStorage 抛 SecurityError——由浏览器沙箱规范在
 *    opaque origin 下保证，happy-dom 无法真实执行跨框脚本，故此处钉死
 *    装配面契约；预览页 CSP 的禁外联面由 Rust 侧 render_html_test 断言）；
 * 3. AskPanel 审批卡的「查看预览」入口。
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AskPayload } from "@/ipc/types";
import { useAskStore } from "@/stores/ask";
import { AskPanel } from "./AskPanel";
import { HtmlPreview, PREVIEW_SANDBOX_TOKENS } from "./HtmlPreview";
import { SessionScopeContext } from "./session-scope";
import {
  BatchReadCard,
  CalculateCard,
  GlobCard,
  RenderHtmlCard,
  toolCardExpandedMemory,
} from "./ToolCards";

const toolStatus = vi.hoisted(() => ({ status: "running", argsStatus: "complete" }));

vi.mock("@assistant-ui/react", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@assistant-ui/react")>();
  return {
    ...actual,
    useToolArgsStatus: () => toolStatus,
  };
});

/** 边车数据面（batch_read/glob/calculate 终态按 callId 拉边车）。 */
const loadToolOutcomes = vi.hoisted(() => vi.fn(async () => ({}) as Record<string, unknown>));
/** 预览注册面（HtmlPreview 挂载即注册）：返回固定 token/URL。 */
const registerPreview = vi.hoisted(() =>
  vi.fn(async (html: string) => ({
    token: `tok-${html.length}`,
    url: `preview://localhost/tok-${html.length}`,
  })),
);
const resolveAskMock = vi.hoisted(() => vi.fn<(askId: string, answer: unknown) => Promise<void>>());
vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    loadToolOutcomes: loadToolOutcomes,
    registerPreview: (html: string) => registerPreview(html),
    resolveAsk: (askId: string, answer: unknown) => resolveAskMock(askId, answer),
  };
});

beforeEach(() => {
  toolStatus.status = "running";
  loadToolOutcomes.mockReset();
  loadToolOutcomes.mockResolvedValue({});
  registerPreview.mockClear();
  resolveAskMock.mockReset();
  resolveAskMock.mockResolvedValue(undefined);
  useAskStore.setState({ bySession: {}, hints: {} });
});
afterEach(() => {
  cleanup();
  // 展开记忆是模块级 Map（跨用例持久）：共享 toolCallId 的用例会互相污染
  // 初始展开态（前一个用例展开 c1，后一个用例的 expandCard 点击反而收起）
  toolCardExpandedMemory.clear();
});

/** 卡片 props 的类型来源（以 RenderHtmlCard 的首参推导）。 */
type PartProps = Parameters<typeof RenderHtmlCard>[0];

/** 卡片 props 的构造（toolName 按用例传）。 */
function props(
  toolName: string,
  args: Record<string, unknown>,
  extra: { result?: unknown; isError?: boolean } = {},
): PartProps {
  return {
    toolCallId: "c1",
    toolName,
    argsText: JSON.stringify(args),
    args: args as PartProps["args"],
    result: undefined,
    isError: false,
    ...extra,
  } as PartProps;
}

/** 结果到达后边车返回的 data（loadToolOutcomes 的响应形状）。 */
function sidecarData(callId: string, data: unknown) {
  loadToolOutcomes.mockResolvedValue({ [callId]: { data, detail: null } });
}

/** 展开卡片详情（单行摘要点击）。 */
function expandCard() {
  fireEvent.click(screen.getByTestId("tool-card-c1").querySelector("button") as HTMLButtonElement);
}

/** 向 ask store 放入一个未决审批（模拟 ask:opened）。 */
function openApproval(payload: AskPayload) {
  useAskStore.getState().open("s1", "ask-1", payload);
}

const baseOptions = [
  { id: "allow_once", label: "允许", recommended: true, mode: "allow_once" as const },
  { id: "deny", label: "拒绝", mode: "deny" as const },
];

describe("small-tools", () => {
  it("四张工具卡：batch_read 徽标与逐文件列表、glob 命中计数与上限、calculate 结果行、render_html 折叠预览", async () => {
    // ---- batch_read：文件数徽标 + 逐文件成功/失败/截断列表 + 内容折叠 ----
    sidecarData("c1", {
      files: [
        { path: "a.txt", ok: true, content: "a1\n", truncated: false, nextOffset: null },
        {
          path: "missing.txt",
          ok: false,
          truncated: false,
          nextOffset: null,
          error: { code: "E_NOT_FOUND", message: "文件不存在：missing.txt" },
        },
        { path: "big.log", ok: true, content: "x".repeat(200), truncated: true, nextOffset: 1748 },
      ],
      truncatedFiles: ["big.log"],
    });
    render(
      <BatchReadCard
        {...props("batch_read", { paths: ["a.txt", "missing.txt", "big.log"] })}
        result={{ ok: true, summary: "批量读取" }}
      />,
    );
    // 徽标 = 文件数；摘要行：首个文件 + 计数
    expect(screen.getByTestId("batch-read-count").textContent).toBe("3");
    const batchCard = screen.getByTestId("tool-card-c1");
    expect(batchCard.textContent).toContain("等 3 个文件");
    expandCard();
    // 逐文件状态（终态异步拉边车）
    await waitFor(() => {
      expect(screen.getByTestId("batch-read-files").textContent).toContain("missing.txt");
    });
    const list = screen.getByTestId("batch-read-files").textContent!;
    expect(list).toContain("文件不存在：missing.txt");
    expect(list).toContain("续读 offset 1748");
    expect(batchCard.textContent).toContain("1 个文件内容截断");
    cleanup();
    toolCardExpandedMemory.clear();

    // ---- glob：命中计数 + 路径列表（上限展示 100 条）----
    const paths = Array.from({ length: 130 }, (_, i) => ({ path: `gen/f${i}.rs`, kind: "file" }));
    sidecarData("c1", { count: 130, truncated: true, paths });
    render(
      <GlobCard
        {...props("glob", { pattern: "**/*.rs", path: "gen" })}
        result={{ ok: true, summary: "130 项命中" }}
      />,
    );
    expandCard();
    await waitFor(() => {
      expect(screen.getByTestId("glob-paths").childElementCount).toBeGreaterThan(0);
    });
    const globCard = screen.getByTestId("tool-card-c1");
    expect(globCard.textContent).toContain("130 项命中");
    expect(globCard.textContent).toContain("上限 500 截断");
    // 上限展示 100 条 + 余量提示
    expect(screen.getByTestId("glob-paths").childElementCount).toBe(101);
    expect(globCard.textContent).toContain("其余 30 项");
    cleanup();
    toolCardExpandedMemory.clear();

    // ---- calculate：表达式摘要 + 结果 monospace 单行 ----
    sidecarData("c1", {
      expression: "0.1 + 0.2",
      result: 0.30000000000000004,
      formatted: "0.30000000000000004",
    });
    render(
      <CalculateCard
        {...props("calculate", { expression: "0.1 + 0.2" })}
        result={{ ok: true, summary: "0.1 + 0.2 = 0.30000000000000004" }}
      />,
    );
    expandCard();
    await waitFor(() => {
      expect(screen.getByTestId("calculate-result").textContent).toBe("= 0.30000000000000004");
    });
    cleanup();
    toolCardExpandedMemory.clear();

    // ---- render_html：字符数徽标 + 预览默认折叠，展开才注册并挂载沙箱 iframe ----
    const html = "<!DOCTYPE html><p>你好预览</p>";
    render(
      <RenderHtmlCard
        {...props("render_html", { html })}
        result={{ ok: true, summary: "HTML 产出就绪" }}
      />,
    );
    expect(screen.getByTestId("render-html-chars").textContent).toContain("字符");
    expandCard();
    // 默认折叠：iframe 未挂载、注册未发生
    expect(registerPreview).not.toHaveBeenCalled();
    expect(document.querySelector("iframe")).toBeNull();
    // 展开后：注册发生、iframe 挂载、地址 = 预览协议
    fireEvent.click(screen.getByTestId("render-html-toggle"));
    expect(screen.getByTestId("render-html-toggle").textContent).toContain("收起预览");
    await waitFor(() => {
      expect(document.querySelector("iframe")).not.toBeNull();
    });
    expect(registerPreview).toHaveBeenCalledWith(html);
    expect(document.querySelector("iframe")!.getAttribute("src")).toBe(
      `preview://localhost/tok-${html.length}`,
    );
  });

  it("HtmlPreview 沙箱契约（sandbox 只带 allow-scripts、注册失败错误行）+ AskPanel 审批卡「查看预览」入口", async () => {
    // ---- 沙箱逃逸向量（装配契约面）：无 allow-same-origin / top-navigation / forms ----
    // 逃逸向量 ①（同源访问）：无 allow-same-origin → opaque origin，
    // 预览页脚本对 parent/top 的 DOM/存储访问被浏览器判定跨源拒绝。
    // 逃逸向量 ②（顶层导航）：无 allow-top-navigation → 主窗不可被劫持。
    // 逃逸向量 ③（表单/自动提交导航）：无 allow-forms。
    render(<HtmlPreview html="<p>escaped?</p>" />);
    const frame = await screen.findByTestId("html-preview-frame");
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
    const tokens = (frame.getAttribute("sandbox") ?? "").split(/\s+/);
    expect(tokens).not.toContain("allow-same-origin");
    expect(tokens).not.toContain("allow-top-navigation");
    expect(tokens).not.toContain("allow-forms");
    expect(tokens).not.toContain("allow-popups");
    // 常量与属性一致（防线单一来源）；地址走预览协议（预览页 CSP 由协议响应自带）
    expect(PREVIEW_SANDBOX_TOKENS).toEqual(["allow-scripts"]);
    expect(frame.getAttribute("src")).toMatch(/^preview:\/\/localhost\//);
    cleanup();

    // ---- 注册失败显示错误行，不渲染 iframe ----
    registerPreview.mockRejectedValueOnce(new Error("HTML 超过 50000 字符上限"));
    render(<HtmlPreview html={"x".repeat(10)} />);
    await waitFor(() => {
      expect(screen.getByTestId("html-preview").textContent).toContain("预览注册失败");
    });
    expect(document.querySelector("iframe")).toBeNull();
    cleanup();

    // ---- 载荷附带 previewHtml 时显示入口，点击展开沙箱预览；点击允许提交应答 ----
    openApproval({
      kind: "approval",
      title: "写入新文件 index.html",
      diff: { path: "index.html", hunks: [] },
      previewHtml: "<p>hello</p>",
      options: baseOptions,
    });
    render(
      <SessionScopeContext.Provider value="s1">
        <AskPanel />
      </SessionScopeContext.Provider>,
    );
    expect(screen.getByTestId("ask-preview").textContent).toContain("查看预览");
    // 默认折叠：无 iframe
    expect(document.querySelector("iframe")).toBeNull();
    fireEvent.click(screen.getByTestId("ask-preview-toggle"));
    await waitFor(() => {
      expect(screen.getByTestId("ask-preview-frame-host-frame")).not.toBeNull();
    });
    // 允许 → resolveAsk 审批应答
    fireEvent.click(screen.getByTestId("ask-option-allow_once"));
    await waitFor(() => {
      expect(resolveAskMock).toHaveBeenCalledWith("ask-1", {
        kind: "approval",
        optionId: "allow_once",
      });
    });
    cleanup();

    // ---- 无 previewHtml 的审批不显示预览入口 ----
    openApproval({
      kind: "approval",
      title: "写入新文件 main.rs",
      diff: { path: "main.rs", hunks: [] },
      options: baseOptions,
    });
    render(
      <SessionScopeContext.Provider value="s1">
        <AskPanel />
      </SessionScopeContext.Provider>,
    );
    expect(screen.queryByTestId("ask-preview")).toBeNull();
  });
});
