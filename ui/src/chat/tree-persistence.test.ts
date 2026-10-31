/**
 * tree-persistence 双向转换测试（S4 spike，15/17 号票增量）。
 *
 * 测试面：用无界面（headless）的 LocalRuntime 跑剧本化对话，验证
 * 「assistant-ui 消息树 ↔ 存储条目 ↔ JSON/JSONL 落盘」三个方向往返后
 * 树结构逐节点一致（含分支编辑、历史分支切换、runMarker 收口摘要），
 * 并核对存储条目 schema 形状与 0.15.31 导出结构的字段面。
 */
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { AssistantRuntimeImpl, LocalRuntimeCore } from "@assistant-ui/core/internal";
import type {
  AssistantRuntime,
  ExportedMessageRepository,
  ThreadMessage,
} from "@assistant-ui/react";
import { ExportedMessageRepository as ExportedMessageRepositoryFactory } from "@assistant-ui/react";
import { afterAll, describe, expect, it, vi } from "vitest";
import type { StoredEntry, StoredMessageEntry, StoredMetaEntry, WirePart } from "../ipc/types";
import { createScriptedAdapter, type ReplyScript, splitChunks } from "./scripted-backend";
import {
  deriveRunSummaries,
  entriesToExported,
  exportToEntries,
  parseSession,
  parseSessionJsonl,
  serializeSession,
  serializeSessionJsonl,
} from "./tree-persistence";

// ---- S4 spike 测试设施：headless LocalRuntime（不走 React 渲染，S6 同款累积假流） ----

type Thread = AssistantRuntime["thread"];

/** 建一个无界面的 runtime 线程：适配器用剧本假后端（不渲染、不依赖 React）。 */
function createRuntime(scripts: readonly ReplyScript[]): Thread {
  const core = new LocalRuntimeCore(
    { adapters: { chatModel: createScriptedAdapter(scripts) } },
    undefined,
  );
  return new AssistantRuntimeImpl(core).thread;
}

/** 等待当前 run 结束：最后一条为 assistant 且 status=complete、线程不在运行 */
async function waitForIdle(thread: Thread): Promise<void> {
  await vi.waitFor(
    () => {
      const state = thread.getState();
      const last = state.messages.at(-1);
      if (last?.role !== "assistant") {
        throw new Error("assistant 回复尚未生成");
      }
      if (state.isRunning) {
        throw new Error("run 仍在进行");
      }
      if (last.status?.type !== "complete") {
        throw new Error(`assistant 未完成：${JSON.stringify(last.status)}`);
      }
    },
    { timeout: 5000, interval: 20 },
  );
}

/** 发一条用户消息并等待回复 run 结束。 */
async function send(thread: Thread, text: string): Promise<void> {
  // parentId 缺省 = 追加到当前活动路径尾部（null 才是"新建根分支"）
  thread.append({ role: "user", content: [{ type: "text", text }], startRun: true });
  await waitForIdle(thread);
}

/** 编辑已发消息：UI 编辑路径的真实 API（beginEdit → setText → send） */
async function editUserMessage(thread: Thread, messageId: string, newText: string): Promise<void> {
  const message = thread.getMessageById(messageId);
  message.composer.beginEdit();
  message.composer.setText(newText);
  message.composer.send();
  await waitForIdle(thread);
}

// ---- 树结构语义化归一（roundtrip 逐节点比对口径） ----

/** 归一后用于逐节点比对的树节点形状（只保留要比对的字段）。 */
type NodeShape = {
  parentId: string | null;
  role: string;
  parts: ReturnType<typeof simplifyPart>[];
  status: unknown;
  createdAt: number;
};

/** part 归一：只留类型与内容字段，isError 的 undefined/false 归一为布尔，
 *  避免往返转换中可选字段缺省造成的假差异。 */
function simplifyPart(part: ThreadMessage["content"][number]): Record<string, unknown> {
  switch (part.type) {
    case "text":
      return { type: "text", text: part.text };
    case "reasoning":
      return { type: "reasoning", text: part.text };
    case "tool-call":
      return {
        type: "tool-call",
        toolCallId: part.toolCallId,
        toolName: part.toolName,
        args: part.args,
        result: part.result,
        // isError 经 roundtrip 从 undefined 归一为 false（语义等价：非错误），比对时归一
        isError: part.isError === true,
      };
    default:
      return { type: part.type };
  }
}

/** 导出树 → 可严格比较的普通对象：节点表 + 活动路径（从 head 沿父指针走到根）。 */
function normalizeRepo(repo: ExportedMessageRepository): {
  headId: string | null;
  nodes: Record<string, NodeShape>;
  activePath: string[];
} {
  const nodes: Record<string, NodeShape> = {};
  for (const { message, parentId } of repo.messages) {
    nodes[message.id] = {
      parentId,
      role: message.role,
      parts: message.content.map(simplifyPart),
      status: message.role === "assistant" ? message.status : undefined,
      createdAt: message.createdAt.getTime(),
    };
  }
  // 活动路径：从 head（activeLeaf）沿 parentId 走到根
  const activePath: string[] = [];
  let cursor = repo.headId ?? null;
  while (cursor !== null) {
    activePath.unshift(cursor);
    cursor = nodes[cursor]?.parentId ?? null;
  }
  return { headId: repo.headId ?? null, nodes, activePath };
}

/** 两棵导出树归一后必须完全一致（roundtrip 断言核心）。 */
function assertTreeEqual(a: ExportedMessageRepository, b: ExportedMessageRepository): void {
  expect(normalizeRepo(b)).toStrictEqual(normalizeRepo(a));
}

// ---- roundtrip 主流程：导出 → §6.1 条目 → JSON 落盘 → 读回 → 重载还原 ----

/** 临时目录登记表：用例结束统一清理，避免残留。 */
const tempDirs: string[] = [];
afterAll(() => {
  for (const dir of tempDirs) rmSync(dir, { recursive: true, force: true });
});

/** roundtrip 全流程辅助：导出 → 条目 → 写临时 JSON 文件 → 读回解析 → 还原树。 */
function roundtripViaFile(
  repo: ExportedMessageRepository,
  sessionId: string,
  revision: number,
): { entries: StoredEntry[]; restored: ExportedMessageRepository } {
  const entries = exportToEntries(repo, sessionId, revision);
  // spike 期：真实落盘单 JSON 文件（生产为 append-only 分段 JSONL，行格式另测）
  const dir = mkdtempSync(join(tmpdir(), "craftwave-s4-"));
  tempDirs.push(dir);
  const file = join(dir, "0001.json");
  writeFileSync(file, serializeSession(entries), "utf8");
  const loaded = parseSession(readFileSync(file, "utf8"));
  return { entries: loaded, restored: entriesToExported(loaded) };
}

describe("tree-persistence（S4 spike）", () => {
  it("线性会话 roundtrip：两轮对话逐节点一致（含 activeLeaf）、reasoning 映射、§6.1 条目 schema、C-4 导出结构字段面", async () => {
    // ---- 两轮对话导出→§6.1 JSON 落盘→重载后树逐节点一致 ----
    const thread = createRuntime([
      { text: splitChunks("第一轮回复", 2) },
      { text: splitChunks("第二轮回复", 2) },
    ]);
    await send(thread, "你好");
    await send(thread, "继续");

    const exported = thread.export();
    expect(exported.messages).toHaveLength(4); // user/assistant × 2

    const { entries, restored } = roundtripViaFile(exported, "sess-s4-linear", 3);
    assertTreeEqual(exported, restored);

    // 重载进新 runtime：活动路径与可见消息序列一致
    const reloaded = createRuntime([]);
    reloaded.import(restored);
    assertTreeEqual(exported, reloaded.export());
    expect(reloaded.getState().messages.map((m) => m.role)).toStrictEqual([
      "user",
      "assistant",
      "user",
      "assistant",
    ]);
    expect(entries.filter((e) => e.kind === "message")).toHaveLength(4);

    // ---- C-4 实证：0.15.31 导出结构字段面 ----
    // ExportedMessageRepository = { headId?, messages:[{message,parentId,runConfig?}] }
    expect(Object.keys(exported).sort()).toStrictEqual(["headId", "messages"]);
    expect(typeof exported.headId).toBe("string");
    // 条目字段面：恰为 message/parentId（runConfig 未设置则不出现）
    for (const item of exported.messages) {
      expect(Object.keys(item).sort()).toStrictEqual(["message", "parentId"]);
      expect(item.message.id).toEqual(expect.any(String));
      expect(item.message.createdAt).toBeInstanceOf(Date);
    }
    // headId 即叶节点 id（activeLeaf 对应物）
    const nodeIds = new Set(exported.messages.map((m) => m.message.id));
    expect(nodeIds.has(exported.headId!)).toBe(true);
    const headItem = exported.messages.find((m) => m.message.id === exported.headId);
    expect(nodeIds.has(headItem!.parentId ?? "")).toBe(headItem!.parentId !== null);

    // ---- reasoning part 映射为 {type:'reasoning'} 且 roundtrip 保真 ----
    const reasoningThread = createRuntime([
      { reasoning: splitChunks("先想一下", 2), text: splitChunks("想完了", 2) },
    ]);
    await send(reasoningThread, "难题");
    const reasoningExported = reasoningThread.export();
    const assistant = reasoningExported.messages.find((m) => m.message.role === "assistant");
    expect(assistant?.message.content.map((p) => p.type)).toStrictEqual(["reasoning", "text"]);
    const reasoningEntries = exportToEntries(reasoningExported, "sess-s4-reasoning", 1);
    const assistantEntry = reasoningEntries.find(
      (e): e is StoredMessageEntry => e.kind === "message" && e.role === "assistant",
    );
    expect(assistantEntry?.parts.map((p) => p.type)).toStrictEqual(["reasoning", "text"]);
    assertTreeEqual(reasoningExported, entriesToExported(reasoningEntries));

    // ---- §6.1 存储条目形状（schema 符合性）：session 首、meta 尾、message 居中、epoch 毫秒时间戳 ----
    const schemaEntries = exportToEntries(reasoningExported, "sess-s4-schema", 7);
    expect(schemaEntries[0]).toStrictEqual({
      kind: "session",
      v: 1,
      id: "sess-s4-schema",
      createdAt: expect.any(Number),
    });
    const meta = schemaEntries.at(-1) as StoredMetaEntry;
    expect(meta).toStrictEqual({
      kind: "meta",
      revision: 7,
      activeLeafId: reasoningExported.headId,
    });
    const schemaMessages = schemaEntries.slice(1, -1) as StoredMessageEntry[];
    expect(schemaMessages).toHaveLength(2);
    for (const item of schemaMessages) {
      expect(item.kind).toBe("message");
      expect(typeof item.id).toBe("string");
      expect(item.createdAt).toEqual(expect.any(Number));
      expect(Number.isInteger(item.createdAt)).toBe(true);
      expect(Array.isArray(item.parts)).toBe(true);
      expect(["user", "assistant", "system"]).toContain(item.role);
    }
    // 根消息无 parentId 字段（可选字段省略而非 null）
    expect(schemaMessages[0]?.parentId).toBeUndefined();
    expect(schemaMessages[1]?.parentId).toBe(schemaMessages[0]?.id);
    // assistant 条目保留 status 与 runId（runId 未设置则省略）；user 条目无 status
    expect(schemaMessages[1]?.status).toStrictEqual({ type: "complete", reason: "unknown" });
    expect(schemaMessages[1]?.runId).toBeUndefined();
    expect(schemaMessages[0]?.status).toBeUndefined();
  });

  it("分支 roundtrip：编辑产生兄弟分支（activeLeaf 指向新分支）、切回旧分支保留 activeLeaf", async () => {
    // ---- 编辑已发消息（新分支）：user 两版本（同 parentId=null）+ 各自 assistant ----
    const thread = createRuntime([
      { text: splitChunks("原始回复", 2) },
      { text: splitChunks("编辑后回复", 2) },
    ]);
    await send(thread, "原始问题");
    const userV1 = thread.getState().messages[0]!.id;
    await editUserMessage(thread, userV1, "编辑后的问题");

    const exported = thread.export();
    expect(exported.messages).toHaveLength(4);
    const roots = exported.messages.filter((m) => m.parentId === null);
    expect(roots).toHaveLength(2);
    const userV1Item = exported.messages.find((m) => m.message.id === userV1);
    const userV2Item = roots.find((m) => m.message.id !== userV1);
    expect(userV1Item).toBeDefined();
    expect(userV2Item).toBeDefined();
    const assistantOfV1 = exported.messages.find((m) => m.parentId === userV1);
    const assistantOfV2 = exported.messages.find((m) => m.parentId === userV2Item!.message.id);
    expect(assistantOfV1?.message.content[0]).toMatchObject({ type: "text", text: "原始回复" });
    expect(assistantOfV2?.message.content[0]).toMatchObject({ type: "text", text: "编辑后回复" });
    // activeLeaf 指向新分支叶（编辑后的 assistant）
    expect(exported.headId).toBe(assistantOfV2?.message.id);

    const { restored } = roundtripViaFile(exported, "sess-s4-edit", 5);
    assertTreeEqual(exported, restored);
    const reloaded = createRuntime([]);
    reloaded.import(restored);
    assertTreeEqual(exported, reloaded.export());
    // 重载后可见的是活动分支（编辑后），而非旧分支
    expect(reloaded.getState().messages).toHaveLength(2);
    expect(reloaded.getState().messages[1]?.content[0]).toMatchObject({ text: "编辑后回复" });

    // ---- 分支切换（switchToBranch）：旧版本落盘的兄弟回复仍可恢复 ----
    const assistantV1 = assistantOfV1!.message.id;
    const first = exported.messages.find((item) => item.message.id === assistantV1)!;
    const assistantV2 = "legacy-assistant-v2";
    thread.import({
      messages: [
        ...exported.messages,
        {
          ...first,
          message: {
            ...first.message,
            id: assistantV2,
            content: [{ type: "text", text: "第二版" }],
          },
        },
      ],
      headId: assistantV2,
    });
    expect(thread.getState().messages.at(-1)?.content[0]).toMatchObject({
      type: "text",
      text: "第二版",
    });
    const legacy = roundtripViaFile(thread.export(), "sess-s4-legacy", 3);
    assertTreeEqual(thread.export(), legacy.restored);
    expect(assistantV2).not.toBe(assistantV1);

    // 切回旧分支：head 指向旧分支叶
    thread.getMessageById(assistantV1).switchToBranch({ branchId: assistantV1 });
    const switched = thread.export();
    expect(switched.headId).toBe(assistantV1);
    expect(thread.getState().messages[1]?.content[0]).toMatchObject({
      type: "text",
      text: "原始回复",
    });

    const { restored: switchedRestored } = roundtripViaFile(switched, "sess-s4-switch", 4);
    assertTreeEqual(switched, switchedRestored);
    const switchedRuntime = createRuntime([]);
    switchedRuntime.import(switchedRestored);
    // 重载后 activeLeaf 是旧分支叶，可见消息是第一版
    assertTreeEqual(switched, switchedRuntime.export());
    expect(switchedRuntime.getState().messages[1]?.content[0]).toMatchObject({
      type: "text",
      text: "原始回复",
    });
  });

  it("toolCall/toolResult 拆分与合并（§3.2 WirePart 口径）、JSONL 行格式、空树 roundtrip、deriveRunSummaries（17 号票）", () => {
    // ---- assistant-ui 单一 tool-call part 导出为 toolCall+toolResult 两条，还原时合并回单 part ----
    const repo = ExportedMessageRepositoryFactory.fromBranchableArray([
      {
        parentId: null,
        message: {
          id: "u1",
          role: "user",
          content: [{ type: "text", text: "读一下清单" }],
        },
      },
      {
        parentId: "u1",
        message: {
          id: "a1",
          role: "assistant",
          status: { type: "complete", reason: "unknown" },
          content: [
            {
              type: "tool-call",
              toolCallId: "call_1",
              toolName: "read_workspace_file",
              args: { path: "Cargo.toml" },
              result: '[package]\nname = "x"',
            },
            { type: "text", text: "读完了" },
          ],
        },
      },
    ]);
    const entries = exportToEntries(repo, "sess-s4-tool", 1);
    const assistantEntry = entries.find(
      (e): e is StoredMessageEntry => e.kind === "message" && e.role === "assistant",
    );
    const toolCall = assistantEntry?.parts.find(
      (p): p is Extract<WirePart, { type: "toolCall" }> => p.type === "toolCall",
    );
    const toolResult = assistantEntry?.parts.find(
      (p): p is Extract<WirePart, { type: "toolResult" }> => p.type === "toolResult",
    );
    expect(toolCall).toStrictEqual({
      type: "toolCall",
      callId: "call_1",
      name: "read_workspace_file",
      argsJson: JSON.stringify({ path: "Cargo.toml" }),
    });
    expect(toolResult).toStrictEqual({
      type: "toolResult",
      callId: "call_1",
      ok: true,
      summary: '[package]\nname = "x"',
    });
    // toolCall 在 toolResult 之前（拓扑序）
    expect(assistantEntry?.parts.indexOf(toolCall!)).toBeLessThan(
      assistantEntry?.parts.indexOf(toolResult!) ?? -1,
    );
    const restored = entriesToExported(entries);
    assertTreeEqual(repo, restored);
    const assistantNode = restored.messages.find((m) => m.message.id === "a1");
    const callPart = assistantNode?.message.content.find((p) => p.type === "tool-call");
    expect(callPart).toMatchObject({
      type: "tool-call",
      toolCallId: "call_1",
      toolName: "read_workspace_file",
      args: { path: "Cargo.toml" },
      result: '[package]\nname = "x"',
    });

    // ---- 无结果的 toolCall（挂起工具）只落 toolCall 一条 ----
    const pendingRepo = ExportedMessageRepositoryFactory.fromBranchableArray([
      {
        parentId: null,
        message: {
          id: "a1",
          role: "assistant",
          status: { type: "requires-action", reason: "tool-calls" },
          content: [
            {
              type: "tool-call",
              toolCallId: "call_x",
              toolName: "write_file",
              args: { path: "a" },
            },
          ],
        },
      },
    ]);
    const pendingEntries = exportToEntries(pendingRepo, "sess-s4-tool-pending", 1);
    const pendingEntry = pendingEntries.find(
      (e): e is StoredMessageEntry => e.kind === "message" && e.role === "assistant",
    );
    expect(pendingEntry?.parts).toStrictEqual([
      { type: "toolCall", callId: "call_x", name: "write_file", argsJson: '{"path":"a"}' },
    ]);

    // ---- 落盘形态：JSONL 行格式与 JSON 文件承载同一份条目（生产 append-only 分段 JSONL 预演）----
    const stored: StoredEntry[] = [
      { kind: "session", v: 1, id: "sess-s4-jsonl", createdAt: 1730000000000 },
      {
        kind: "message",
        id: "m1",
        role: "user",
        parts: [{ type: "text", text: "你好" }],
        createdAt: 1730000000001,
      },
      { kind: "meta", revision: 1, activeLeafId: "m1" },
    ];
    const jsonl = serializeSessionJsonl(stored);
    expect(jsonl.split("\n").filter((line) => line.length > 0)).toHaveLength(3);
    expect(parseSessionJsonl(jsonl)).toStrictEqual(stored);

    // ---- 空树 roundtrip：headId 为 null 不落 activeLeafId ----
    const emptyEntries = exportToEntries({ headId: null, messages: [] }, "sess-s4-empty", 0);
    expect(emptyEntries).toHaveLength(2); // session + meta
    const emptyMeta = emptyEntries.at(-1) as StoredMetaEntry;
    expect(emptyMeta.activeLeafId).toBeUndefined();
    const emptyRestored = entriesToExported(emptyEntries);
    expect(emptyRestored.messages).toHaveLength(0);
    expect(emptyRestored.headId ?? null).toBeNull();

    // ---- deriveRunSummaries（runMarker 收口摘要推导）：正常收口与崩溃（endedAt 空）各自入表 ----
    const summaries = deriveRunSummaries([
      {
        id: "run-1:marker",
        role: "system",
        createdAt: 1,
        parts: [
          {
            type: "runMarker",
            runId: "run-1",
            startedAt: 10,
            endedAt: 90,
            durationMs: 80,
            toolCount: 3,
          },
        ],
      },
      {
        id: "run-2:marker",
        role: "system",
        createdAt: 2,
        parts: [{ type: "runMarker", runId: "run-2", startedAt: 100 }],
      },
      { id: "u1", role: "user", createdAt: 3, parts: [{ type: "text", text: "问" }] },
    ]);
    expect(summaries.get("run-1")).toEqual({
      runId: "run-1",
      endedAt: 90,
      durationMs: 80,
      toolCount: 3,
    });
    // 崩溃修复行（endedAt 缺省）：渲染层据此标「已中断」
    expect(summaries.get("run-2")?.endedAt).toBeUndefined();
    expect(summaries.size).toBe(2);
    // 非 system 条目与无 marker 的条目不入表
    const noSummaries = deriveRunSummaries([
      { id: "u1", role: "user", createdAt: 1, parts: [{ type: "text", text: "问" }] },
      { id: "s1", role: "system", createdAt: 2, parts: [{ type: "text", text: "" }] },
    ]);
    expect(noSummaries.size).toBe(0);
  });
});
