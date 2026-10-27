import { describe, expect, it } from "vitest";
import { echoStream, promptFrom, REPLY_PREFIX, splitReply } from "./runtime";

describe("splitReply", () => {
  it("切成指定段数且拼回原文", () => {
    const reply = "abcdefgh";
    const chunks = splitReply(reply, 3);
    expect(chunks.length).toBeLessThanOrEqual(3);
    expect(chunks.join("")).toBe(reply);
  });

  it("短文本不产生空段", () => {
    for (const chunk of splitReply("ab", 4)) {
      expect(chunk.length).toBeGreaterThan(0);
    }
  });
});

describe("echoStream", () => {
  it("累积 yield，末次包含完整回复", async () => {
    const controller = new AbortController();
    const yields: { content: [{ type: "text"; text: string }] }[] = [];
    for await (const result of echoStream("你好", controller.signal)) {
      yields.push(result);
    }
    expect(yields.length).toBeGreaterThanOrEqual(2);
    expect(yields[0]!.content[0]!.text.length).toBeGreaterThan(0);
    expect(yields.at(-1)!.content[0]!.text).toContain(REPLY_PREFIX);
    expect(yields.at(-1)!.content[0]!.text.endsWith("你好")).toBe(true);
    // 累积语义：每一段都比前一段长
    for (let i = 1; i < yields.length; i++) {
      expect(yields[i]!.content[0]!.text.length).toBeGreaterThan(
        yields[i - 1]!.content[0]!.text.length,
      );
    }
  });

  it("已中止的信号直接结束", async () => {
    const controller = new AbortController();
    controller.abort();
    const yields: unknown[] = [];
    for await (const result of echoStream("你好", controller.signal)) {
      yields.push(result);
    }
    expect(yields).toHaveLength(0);
  });
});

describe("promptFrom", () => {
  it("空历史回退占位文案", () => {
    expect(promptFrom([])).toBe("（空消息）");
  });
});
