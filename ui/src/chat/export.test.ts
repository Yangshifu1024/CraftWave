// @vitest-environment happy-dom
/**
 * 会话导出序列化测试（27 号票，FR-01-12/§10.7 形态定稿）：
 * - Markdown：标题 + 导出时间 + 逐条 user/assistant 段 + 工具列表行
 *   （边车 uiSummary 优先、帧面 summary 降级、ok/error 标注）；
 * - reasoning 不导出；runMarker 内部条目不进 Markdown；空会话；
 * - JSONL：活动路径逐行、仅剔 reasoning part、toolCall/toolResult 保留、
 *   图片只含 blobHash 引用；
 * - 文件名清洗（非法字符替换 `-`、空标题兜底、日期段）；
 * - exportSession 编排（树/边车加载与下载触发、边车失败降级空表）。
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { MessageEntry, MessageRole, ToolOutcome, WirePart } from "@/ipc/types";
import {
  downloadText,
  exportFileName,
  exportSession,
  formatTimestamp,
  toJsonl,
  toMarkdown,
  toolLinesOf,
} from "./export";

// IPC 命令桩（exportSession 编排的两个数据源）
const loadSessionTreeMock = vi.fn<(sessionId: string) => Promise<unknown>>();
const loadToolOutcomesMock = vi.fn<(sessionId: string, callIds: string[]) => Promise<unknown>>();

vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    loadSessionTree: (sessionId: string) => loadSessionTreeMock(sessionId),
    loadToolOutcomes: (sessionId: string, callIds: string[]) =>
      loadToolOutcomesMock(sessionId, callIds),
  };
});

beforeEach(() => {
  vi.resetAllMocks();
});

// ---- 样例构造 ----

const text = (t: string): WirePart => ({ type: "text", text: t });
const reasoning = (t: string): WirePart => ({ type: "reasoning", text: t });
const toolCall = (callId: string, name: string): WirePart => ({
  type: "toolCall",
  callId,
  name,
  argsJson: "{}",
});
const toolResult = (callId: string, ok: boolean, summary: string): WirePart => ({
  type: "toolResult",
  callId,
  ok,
  summary,
});
const image = (hash: string): WirePart => ({
  type: "image",
  blobHash: hash,
  mimeType: "image/png",
});
const runMarker = (): WirePart => ({ type: "runMarker", runId: "r1", startedAt: 0 });

const entry = (
  id: string,
  role: MessageRole,
  parts: WirePart[],
  createdAt = 1_735_689_600_000, // 2025-01-01 08:00 (UTC+8)
): MessageEntry => ({ id, role, parts, createdAt });

const sidecar = (uiSummary: string, ok = true): ToolOutcome => ({
  ok,
  uiSummary,
});

// 固定导出时刻（时间面确定性）
const FIXED_NOW = 1_735_689_600_000;

describe("Markdown 序列化（toMarkdown）", () => {
  it("标题 + 导出时间行 + user/assistant 段与时间", () => {
    const md = toMarkdown(
      { title: "调试登录页" },
      [entry("u1", "user", [text("帮我修登录")]), entry("a1", "assistant", [text("好的")])],
      {},
      FIXED_NOW,
    );
    expect(md).toContain("# 调试登录页");
    expect(md).toContain(`- 导出时间：${formatTimestamp(FIXED_NOW)}`);
    expect(md).toContain("## user");
    expect(md).toContain("帮我修登录");
    expect(md).toContain("## assistant");
    expect(md).toContain("好的");
    expect(md).toContain(`- 时间：${formatTimestamp(1_735_689_600_000)}`);
  });

  it("工具调用列表行附在 assistant 段内：边车 uiSummary 优先 + ok/error 标注", () => {
    const messages = [
      entry("a1", "assistant", [
        reasoning("思考不导出"),
        text("我来读文件"),
        toolCall("c1", "read"),
        toolResult("c1", true, "帧面摘要"),
        toolCall("c2", "command"),
        toolResult("c2", false, "执行失败"),
      ]),
    ];
    // c1 有边车（uiSummary 优先）、c2 缺文件（帧面 summary 降级，#29 语义）
    const md = toMarkdown({ title: "t" }, messages, { c1: sidecar("边车明细摘要") }, FIXED_NOW);
    expect(md).not.toContain("思考不导出");
    expect(md).toContain("- ⚙ read — 边车明细摘要（ok）");
    expect(md).toContain("- ⚙ command — 执行失败（error）");
  });

  it("边车与帧结果都缺 → 工具名兜底；toolLinesOf 的 ok 取值链", () => {
    const e = entry("a1", "assistant", [toolCall("c1", "read")]);
    expect(toolLinesOf(e, {})).toEqual([{ name: "read", uiSummary: "read", ok: true }]);
    const failed = entry("a2", "assistant", [
      toolCall("c2", "command"),
      toolResult("c2", false, "被拦"),
    ]);
    expect(toolLinesOf(failed, {})[0]).toEqual({ name: "command", uiSummary: "被拦", ok: false });
  });

  it("runMarker 内部条目不进归档；空会话只剩标题头", () => {
    const md = toMarkdown(
      { title: "空会话" },
      [entry("m1", "system", [runMarker()])],
      {},
      FIXED_NOW,
    );
    expect(md).not.toContain("## user");
    expect(md).not.toContain("## assistant");
    expect(md).toContain("# 空会话");
  });
});

describe("JSONL 序列化（toJsonl）", () => {
  it("活动路径逐行 JSON、仅剔 reasoning part、toolCall/toolResult 保留", () => {
    const messages = [
      entry("u1", "user", [text("第一问")]),
      entry("a1", "assistant", [
        reasoning("内部思考"),
        text("答复"),
        toolCall("c1", "read"),
        toolResult("c1", true, "读了"),
      ]),
    ];
    const lines = toJsonl(messages).trimEnd().split("\n");
    expect(lines).toHaveLength(2);
    const first = JSON.parse(lines[0]) as MessageEntry;
    expect(first.id).toBe("u1");
    expect(first.parts).toEqual([text("第一问")]);
    const second = JSON.parse(lines[1]) as MessageEntry;
    // reasoning 被剔除，toolCall/toolResult parts 保留（机器可读取）
    expect(second.parts.map((p) => p.type)).toEqual(["text", "toolCall", "toolResult"]);
  });

  it("图片 part 只导 blobHash 引用不含本体（不宣称跨机器完整回放）", () => {
    const messages = [entry("u1", "user", [text("看图"), image("abc123")])];
    const parsed = JSON.parse(toJsonl(messages).trim()) as MessageEntry;
    expect(parsed.parts[1]).toEqual({ type: "image", blobHash: "abc123", mimeType: "image/png" });
    expect(JSON.stringify(parsed)).not.toContain("data:image");
  });
});

describe("文件名（exportFileName）", () => {
  it("非法文件名字符替换为 -（连续折叠单个），日期段 YYYYMMDD", () => {
    const name = exportFileName('帮/我:修*复?"文<件>|', "md", FIXED_NOW);
    // 2025-01-01 → 20250101；连续非法字符折叠为单个减号
    expect(name).toBe("帮-我-修-复-文-件-20250101.md");
  });

  it("空标题与全非法字符标题兜底「会话」", () => {
    expect(exportFileName("", "jsonl", FIXED_NOW)).toBe("会话-20250101.jsonl");
    expect(exportFileName("///", "md", FIXED_NOW)).toBe("会话-20250101.md");
  });

  it("首尾点与空白清理（Windows 结尾点非法）", () => {
    expect(exportFileName("  标题.  ", "md", FIXED_NOW)).toBe("标题-20250101.md");
  });
});

describe("exportSession 编排", () => {
  it("加载树 + 批量拉边车 → 触发下载（md 文件名与内容面）", async () => {
    const messages = [
      entry("u1", "user", [text("问题")]),
      entry("a1", "assistant", [text("答"), toolCall("c1", "read"), toolResult("c1", true, "x")]),
    ];
    loadSessionTreeMock.mockResolvedValue({
      sessionId: "s1",
      messages,
      compactions: [],
      activeLeafId: "a1",
      revision: 1,
    });
    loadToolOutcomesMock.mockResolvedValue({ c1: sidecar("边车摘要") });
    vi.stubGlobal("URL", {
      ...URL,
      createObjectURL: () => "blob:mock",
      revokeObjectURL: () => {},
    });
    const appendSpy = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    const downloadSpy = vi.fn();
    Object.defineProperty(HTMLAnchorElement.prototype, "download", {
      set: downloadSpy,
      configurable: true,
    });
    vi.spyOn(document.body, "appendChild").mockImplementation((node) => node);

    const filename = await exportSession({ id: "s1", title: "调试会话" }, "markdown", FIXED_NOW);
    expect(filename).toBe("调试会话-20250101.md");
    expect(loadSessionTreeMock).toHaveBeenCalledWith("s1");
    // callIds 从活动路径 toolCall parts 收集
    expect(loadToolOutcomesMock).toHaveBeenCalledWith("s1", ["c1"]);
    expect(downloadSpy).toHaveBeenCalledWith("调试会话-20250101.md");
    expect(appendSpy).toHaveBeenCalled();
    vi.unstubAllGlobals();
    appendSpy.mockRestore();
    vi.restoreAllMocks();
  });

  it("边车加载失败降级空表（不阻塞导出，工具行走帧面）", async () => {
    loadSessionTreeMock.mockResolvedValue({
      sessionId: "s1",
      messages: [
        entry("a1", "assistant", [toolCall("c1", "read"), toolResult("c1", true, "帧面")]),
      ],
      compactions: [],
      revision: 1,
    });
    loadToolOutcomesMock.mockRejectedValue(new Error("boom"));
    vi.stubGlobal("URL", {
      ...URL,
      createObjectURL: () => "blob:mock",
      revokeObjectURL: () => {},
    });
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    vi.spyOn(document.body, "appendChild").mockImplementation((node) => node);
    const filename = await exportSession({ id: "s1", title: "会话" }, "jsonl", FIXED_NOW);
    expect(filename).toBe("会话-20250101.jsonl");
    expect(clickSpy).toHaveBeenCalled();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });
});

describe("downloadText（下载原语）", () => {
  it("Blob URL + a[download] 点击后 revoke 清理", () => {
    const revoke = vi.fn();
    vi.stubGlobal("URL", {
      ...URL,
      createObjectURL: () => "blob:xyz",
      revokeObjectURL: revoke,
    });
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    const appendSpy = vi.spyOn(document.body, "appendChild").mockImplementation((node) => node);
    downloadText("a.md", "内容", "text/markdown");
    expect(clickSpy).toHaveBeenCalled();
    expect(revoke).toHaveBeenCalledWith("blob:xyz");
    vi.unstubAllGlobals();
    appendSpy.mockRestore();
    clickSpy.mockRestore();
  });
});
