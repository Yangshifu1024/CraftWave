import { describe, expect, it } from "vitest";
import type { StoredMessageEntry } from "@/ipc/types";
import { entriesToExported } from "../tree-persistence";
import { buildStressEntries, STRESS_MESSAGE_COUNT, STRESS_SESSION_ID } from "./stress-source";

/**
 * 压测数据源的结构不变量（压测 harness）：
 * 合成会话必须与真实会话条目契约同构，且覆盖四类渲染负载
 * （runMarker 分组 / 工具卡 / 长文本 / 图片 part）与压缩边界投影。
 * 这些不变量守住「压测压的是真实管线」——数据形状漂移即红。
 */

describe("buildStressEntries（压测源冒烟）", () => {
  const dataset = buildStressEntries();

  it("默认 2000 条单链线性：parentId 依次相接、meta.activeLeafId 指向链尾、覆盖四类渲染负载与压缩边界投影、可被收养管线还原", () => {
    // ---- 默认生成 2000 条消息（压测口径）----
    expect(STRESS_MESSAGE_COUNT).toBe(2000);
    expect(dataset.messageCount).toBe(2000);
    const messages = dataset.entries.filter((e): e is StoredMessageEntry => e.kind === "message");
    expect(messages.filter((m) => m.role !== "system")).toHaveLength(2000);

    // ---- 单链线性：链头无父，其后每条消息的父都是前一条 ----
    expect(messages[0].parentId).toBeUndefined();
    for (let i = 1; i < messages.length; i++) {
      expect(messages[i].parentId).toBe(messages[i - 1].id);
    }
    const meta = dataset.entries.find((e) => e.kind === "meta");
    expect(meta).toBeDefined();
    if (meta?.kind === "meta") {
      expect(meta.activeLeafId).toBe(messages[messages.length - 1].id);
    }

    // ---- id 全局唯一（树还原的前提）----
    const ids = dataset.entries.map((e) => ("id" in e ? e.id : ""));
    expect(new Set(ids).size).toBe(ids.length);

    // ---- 每 100 条消息插一个 runMarker 系统条目（20 组分组面，runId 彼此不同）----
    const markers = messages.filter((m) => m.role === "system");
    expect(markers).toHaveLength(20);
    for (const marker of markers) {
      expect(marker.parts).toHaveLength(1);
      expect(marker.parts[0]).toMatchObject({ type: "runMarker" });
    }
    const runIds = markers.map((m) => (m.parts[0] as { type: "runMarker"; runId: string }).runId);
    expect(new Set(runIds).size).toBe(20);

    // ---- assistant 消息覆盖长文本/工具卡/推理/短文本四种形态，状态收口 complete ----
    const assistants = messages.filter((m) => m.role === "assistant");
    expect(
      assistants.some((a) => a.parts.some((p) => p.type === "text" && p.text.length >= 400)),
    ).toBe(true);
    expect(
      assistants.some(
        (a) =>
          a.parts.some((p) => p.type === "toolCall") &&
          a.parts.some((p) => p.type === "toolResult"),
      ),
    ).toBe(true);
    expect(assistants.some((a) => a.parts.some((p) => p.type === "reasoning"))).toBe(true);
    expect(
      assistants.some(
        (a) => a.parts.length === 1 && a.parts[0].type === "text" && a.parts[0].text.length < 30,
      ),
    ).toBe(true);
    for (const a of assistants) {
      expect(a.status).toEqual({ type: "complete", reason: "stop" });
    }

    // ---- 图片 part 出现在 user 消息上且只含引用不含本体 ----
    const users = messages.filter((m) => m.role === "user");
    const imageParts = users.flatMap((usr) => usr.parts.filter((p) => p.type === "image"));
    expect(imageParts.length).toBeGreaterThanOrEqual(4);
    for (const part of imageParts) {
      if (part.type === "image") {
        expect(part.blobHash).toHaveLength(32);
        expect(part.mimeType).toBe("image/png");
        // 序列化后的行不含 base64 本体（体积口径与真实契约一致）
        expect(JSON.stringify(part)).not.toContain("data:image");
      }
    }

    // ---- 压缩边界不进消息树，作为独立投影返回且锚点真实存在 ----
    // entries 里没有 compaction 条目（投影面承载）
    expect(dataset.entries.some((e) => e.kind === "compaction")).toBe(false);
    expect(dataset.compactions.length).toBeGreaterThanOrEqual(3);
    const allIds = new Set(dataset.entries.map((e) => ("id" in e ? e.id : "")));
    for (const compaction of dataset.compactions) {
      expect(allIds.has(compaction.cutEntryId)).toBe(true);
      expect(compaction.tokensBefore).toBeGreaterThan(0);
    }

    // ---- 产物可被真实收养管线消费（entriesToExported 还原成功）----
    // 树能从链还原说明契约形状成立；进树条目 = 2000 消息 + 20 marker
    //（meta 只是 headId 来源，不进 messages）
    expect(entriesToExported(dataset.entries).messages.length).toBe(2020);
  });

  it("自定义条数生效且非偶数被拒绝；虚拟 sessionId 不与真实会话冲突", () => {
    expect(() => buildStressEntries(11)).toThrow();
    expect(buildStressEntries(20).messageCount).toBe(20);
    expect(STRESS_SESSION_ID).toBe("stress-perf");
  });
});
