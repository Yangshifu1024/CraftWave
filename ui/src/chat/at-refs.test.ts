// at-refs 纯逻辑测试（发送合成引用段）：token 提取（引号/
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

describe("at-refs", () => {
  it("extractAtTokens 与 extractAtTokenSpans：普通/引号双形态、未闭合不产出、区间覆盖与升序", () => {
    // 普通形态：@ 后到空白前
    expect(extractAtTokens("看 @src/main.rs 和 @docs/a.md")).toEqual(["src/main.rs", "docs/a.md"]);
    // 引号形态：@"..." 整段（含空格）；双形态混排互不干扰
    expect(extractAtTokens('看 @"a b/c.txt" 说明')).toEqual(["a b/c.txt"]);
    expect(extractAtTokens('@src/a.ts 与 @"b c/d.ts" 与 @docs/e.md')).toEqual([
      "src/a.ts",
      "b c/d.ts",
      "docs/e.md",
    ]);
    // 未闭合引号形态不产出 token（输入中态）；无 @ → 空表
    expect(extractAtTokens('看 @"a b')).toEqual([]);
    expect(extractAtTokens("普通消息没有引用")).toEqual([]);

    // 区间版（chip 高亮渲染面）：覆盖 @ 与 token，普通文本段在区间外
    const text = "看 @src/a.ts 尾部";
    const spans = extractAtTokenSpans(text);
    expect(spans).toEqual([{ token: "src/a.ts", start: 2, end: 11 }]);
    expect(text.slice(spans[0].start, spans[0].end)).toBe("@src/a.ts");
    // 引号形态区间含 @" 与闭合引号整体；引号内的 @ 不产出区间（不嵌套）
    const quoted = '看 @"a b/c.txt" 尾部';
    expect(quoted.slice(...spanBounds(extractAtTokenSpans(quoted)[0]))).toBe('@"a b/c.txt"');
    expect(extractAtTokenSpans('看 @"a@b/c.txt"')).toEqual([
      { token: "a@b/c.txt", start: 2, end: 14 },
    ]);
    // 区间按位置升序（渲染切分依赖）
    const ordered = extractAtTokenSpans("@b 后 @a");
    expect(ordered.map((s) => s.start)).toEqual(
      [...ordered.map((s) => s.start)].sort((x, y) => x - y),
    );
  });

  it("isRefCandidate 相对路径准入（绝对/穿越/空拒绝）+ buildRefSection 引用段格式", () => {
    // 相对路径通过（含空格路径）
    expect(isRefCandidate("src/main.rs")).toBe(true);
    expect(isRefCandidate("a b/c.txt")).toBe(true);
    // 绝对路径拒绝（拖拽形态，原样保留正文）：POSIX 与 Windows 盘符
    expect(isRefCandidate("/Users/x/proj/a.txt")).toBe(false);
    expect(isRefCandidate("C:\\proj\\a.txt")).toBe(false);
    // 目录穿越拒绝；空 token 拒绝
    expect(isRefCandidate("../secret.txt")).toBe(false);
    expect(isRefCandidate("src/../../etc/passwd")).toBe(false);
    expect(isRefCandidate("")).toBe(false);

    // 引用段格式：正文尾部空行 + 标题行 + 逐行 `- @<路径>`
    expect(buildRefSection(["a.ts", "b c.md"])).toBe("\n\n[引用文件]\n- @a.ts\n- @b c.md");
  });

  it("assembleRefSection 发送变换：追加/去重/越界剔除/上限 20/零开销路径", () => {
    /** 测试消息的最小形状（textsOf 注入，免构造 ThreadMessage）。 */
    type Msg = { id: string; text: string };
    const textsOf = (m: Msg) => m.text;
    const one = (text: string): Msg[] => [{ id: "m1", text }];

    // 命中引用：正文尾部追加引用段（仅替换最后一条，前序消息同一引用）
    const messages: Msg[] = [
      { id: "m1", text: "上一条" },
      { id: "m2", text: "看 @src/a.ts 和 @docs/b.md" },
    ];
    const out = assembleRefSection(messages, textsOf);
    expect(out[0]).toBe(messages[0]);
    expect(out[1].text).toBe("看 @src/a.ts 和 @docs/b.md\n\n[引用文件]\n- @src/a.ts\n- @docs/b.md");

    // 多路径去重（同 token 只进引用段一次）
    expect(assembleRefSection(one("@a.ts 和 @a.ts 再 @a.ts"), textsOf)[0].text).toBe(
      "@a.ts 和 @a.ts 再 @a.ts\n\n[引用文件]\n- @a.ts",
    );

    // 越界 token 剔除（绝对/穿越路径不进引用段，正文原样保留）
    expect(assembleRefSection(one("看 @/tmp/x.txt 与 @../y.txt 和 @ok.txt"), textsOf)[0].text).toBe(
      "看 @/tmp/x.txt 与 @../y.txt 和 @ok.txt\n\n[引用文件]\n- @ok.txt",
    );
    // 只有越界 token → 不追加引用段（正文原样）
    expect(assembleRefSection(one("看 @/abs/path.txt"), textsOf)[0].text).toBe("看 @/abs/path.txt");

    // 上限 20：超出截断并 console.warn
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const tokens = Array.from({ length: 25 }, (_, i) => `@f${i}.txt`).join(" ");
    const capped = assembleRefSection(one(tokens), textsOf);
    const section = capped[0].text.slice(capped[0].text.indexOf("[引用文件]"));
    expect(section.split("\n").length - 1).toBe(AT_REF_LIMIT);
    expect(warn).toHaveBeenCalled();

    // 无 @token → 原样返回（零开销路径，元素同一引用）；空消息列表 → 原样返回
    const plain = one("普通消息");
    const untouched = assembleRefSection(plain, textsOf);
    expect(untouched[0]).toBe(plain[0]);
    expect(untouched[0].text).toBe("普通消息");
    expect(assembleRefSection([], textsOf)).toEqual([]);
  });
});

/** span 边界取值简写（slice 两参展开用）。 */
function spanBounds(span: { start: number; end: number }): [number, number] {
  return [span.start, span.end];
}
