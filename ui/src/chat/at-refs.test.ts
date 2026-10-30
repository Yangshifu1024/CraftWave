// at-refs 纯逻辑测试（28 号票，§10.6 发送合成引用段）：token 提取（引号/
// 普通双形态）、相对路径准入（绝对/穿越剔除）、去重与上限 20、引用段拼装。
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  AT_REF_LIMIT,
  assembleRefSection,
  buildRefSection,
  extractAtTokenSpans,
  extractAtTokens,
  isRefCandidate,
} from "./at-refs";

afterEach(() => {
  vi.restoreAllMocks();
});

describe("extractAtTokens（@token 提取）", () => {
  it("普通形态：@ 后到空白前", () => {
    expect(extractAtTokens("看 @src/main.rs 和 @docs/a.md")).toEqual(["src/main.rs", "docs/a.md"]);
  });

  it('引号形态：@"..." 整段（含空格）', () => {
    expect(extractAtTokens('看 @"a b/c.txt" 说明')).toEqual(["a b/c.txt"]);
  });

  it("双形态混排互不干扰", () => {
    expect(extractAtTokens('@src/a.ts 与 @"b c/d.ts" 与 @docs/e.md')).toEqual([
      "src/a.ts",
      "b c/d.ts",
      "docs/e.md",
    ]);
  });

  it("未闭合引号形态不产出 token（输入中态）", () => {
    expect(extractAtTokens('看 @"a b')).toEqual([]);
  });

  it("无 @ → 空表", () => {
    expect(extractAtTokens("普通消息没有引用")).toEqual([]);
  });
});

describe("extractAtTokenSpans（区间版，chip 高亮渲染面）", () => {
  it("区间覆盖 @ 与 token，普通文本段在区间外", () => {
    const text = "看 @src/a.ts 尾部";
    const spans = extractAtTokenSpans(text);
    expect(spans).toEqual([{ token: "src/a.ts", start: 2, end: 11 }]);
    expect(text.slice(spans[0].start, spans[0].end)).toBe("@src/a.ts");
  });

  it('引号形态区间含 @" 与闭合引号整体', () => {
    const text = '看 @"a b/c.txt" 尾部';
    const spans = extractAtTokenSpans(text);
    expect(text.slice(spans[0].start, spans[0].end)).toBe('@"a b/c.txt"');
  });

  it("引号内的 @ 不产出区间（不嵌套）", () => {
    const text = '看 @"a@b/c.txt"';
    expect(extractAtTokenSpans(text)).toEqual([{ token: "a@b/c.txt", start: 2, end: 14 }]);
  });

  it("区间按位置升序（渲染切分依赖）", () => {
    const spans = extractAtTokenSpans("@b 后 @a");
    expect(spans.map((s) => s.start)).toEqual([...spans.map((s) => s.start)].sort((x, y) => x - y));
  });
});

describe("isRefCandidate（相对路径准入）", () => {
  it("相对路径通过", () => {
    expect(isRefCandidate("src/main.rs")).toBe(true);
    expect(isRefCandidate("a b/c.txt")).toBe(true);
  });

  it("绝对路径拒绝（拖拽形态，原样保留正文）", () => {
    expect(isRefCandidate("/Users/x/proj/a.txt")).toBe(false);
    expect(isRefCandidate("C:\\proj\\a.txt")).toBe(false);
  });

  it("目录穿越拒绝", () => {
    expect(isRefCandidate("../secret.txt")).toBe(false);
    expect(isRefCandidate("src/../../etc/passwd")).toBe(false);
  });

  it("空 token 拒绝", () => {
    expect(isRefCandidate("")).toBe(false);
  });
});

describe("buildRefSection（引用段格式）", () => {
  it("正文尾部空行 + 标题行 + 逐行 `- @<路径>`", () => {
    expect(buildRefSection(["a.ts", "b c.md"])).toBe("\n\n[引用文件]\n- @a.ts\n- @b c.md");
  });
});

describe("assembleRefSection（发送变换）", () => {
  /** 测试消息的最小形状（textsOf 注入，免构造 ThreadMessage）。 */
  type Msg = { id: string; text: string };
  const textsOf = (m: Msg) => m.text;

  it("命中引用：正文尾部追加引用段（仅替换最后一条）", () => {
    const messages: Msg[] = [
      { id: "m1", text: "上一条" },
      { id: "m2", text: "看 @src/a.ts 和 @docs/b.md" },
    ];
    const out = assembleRefSection(messages, textsOf);
    expect(out).not.toBe(messages);
    expect(out[0]).toBe(messages[0]); // 前序消息原样（同一引用）
    expect(out[1].text).toBe("看 @src/a.ts 和 @docs/b.md\n\n[引用文件]\n- @src/a.ts\n- @docs/b.md");
  });

  it("多路径去重（同 token 只进引用段一次）", () => {
    const messages: Msg[] = [{ id: "m1", text: "@a.ts 和 @a.ts 再 @a.ts" }];
    const out = assembleRefSection(messages, textsOf);
    expect(out[0].text).toBe("@a.ts 和 @a.ts 再 @a.ts\n\n[引用文件]\n- @a.ts");
  });

  it("越界 token 剔除（绝对路径不进引用段，正文原样保留）", () => {
    const messages: Msg[] = [{ id: "m1", text: "看 @/tmp/x.txt 与 @../y.txt 和 @ok.txt" }];
    const out = assembleRefSection(messages, textsOf);
    expect(out[0].text.endsWith("\n\n[引用文件]\n- @ok.txt")).toBe(true);
  });

  it("上限 20：超出截断并 console.warn", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const tokens = Array.from({ length: 25 }, (_, i) => `@f${i}.txt`).join(" ");
    const messages: Msg[] = [{ id: "m1", text: tokens }];
    const out = assembleRefSection(messages, textsOf);
    const section = out[0].text.slice(out[0].text.indexOf("[引用文件]"));
    expect(section.split("\n").length - 1).toBe(AT_REF_LIMIT);
    expect(warn).toHaveBeenCalled();
  });

  it("无 @token → 原样返回（零开销路径，元素同一引用）", () => {
    const messages: Msg[] = [{ id: "m1", text: "普通消息" }];
    const out = assembleRefSection(messages, textsOf);
    expect(out[0]).toBe(messages[0]);
    expect(out[0].text).toBe("普通消息");
  });

  it("只有越界 token → 不追加引用段（正文原样）", () => {
    const messages: Msg[] = [{ id: "m1", text: "看 @/abs/path.txt" }];
    const out = assembleRefSection(messages, textsOf);
    expect(out[0].text).toBe("看 @/abs/path.txt");
  });

  it("空消息列表 → 原样返回", () => {
    expect(assembleRefSection([], textsOf)).toEqual([]);
  });
});
