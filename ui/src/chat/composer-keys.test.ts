// composer-keys 纯逻辑测试（17 号票）：@ 触发检测与插入。
import { describe, expect, it } from "vitest";
import { detectAtQuery, insertAtCompletion } from "./composer-keys";

describe("detectAtQuery（@ 路径触发检测，§10.6 光标锚定）", () => {
  it("光标前未闭合的 @ 返回插入位与查询词", () => {
    const text = "看一下 @src/ma";
    expect(detectAtQuery(text, text.length)).toEqual({ at: 4, query: "src/ma" });
  });

  it("@ 与光标间隔空白 → 非触发态", () => {
    const text = "看看 @src 之后";
    expect(detectAtQuery(text, text.length)).toBeNull();
  });

  it("无 @ 或光标在 @ 前 → null", () => {
    expect(detectAtQuery("普通文本", 4)).toBeNull();
    expect(detectAtQuery("@src", 0)).toBeNull();
  });

  it("取最近的 @（多个触发位时）", () => {
    const text = "@a 起 @b";
    expect(detectAtQuery(text, text.length)).toEqual({ at: 5, query: "b" });
  });
});

describe("insertAtCompletion（补全项插入）", () => {
  it("替换查询词为完整路径并留尾空格", () => {
    const text = "读 @src/ma";
    expect(insertAtCompletion(text, 2, text.length, "src/main.rs")).toBe("读 @src/main.rs ");
  });
});
