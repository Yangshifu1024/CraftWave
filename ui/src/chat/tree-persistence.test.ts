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

type NodeShape = {
  parentId: string | null;
  role: string;
  parts: ReturnType<typeof simplifyPart>[];
  status: unknown;
  createdAt: number;
};

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

function assertTreeEqual(a: ExportedMessageRepository, b: ExportedMessageRepository): void {
  expect(normalizeRepo(b)).toStrictEqual(normalizeRepo(a));
}

// ---- roundtrip 主流程：导出 → §6.1 条目 → JSON 落盘 → 读回 → 重载还原 ----

const tempDirs: string[] = [];
afterAll(() => {
  for (const dir of tempDirs) rmSync(dir, { recursive: true, force: true });
});

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

describe("S4 roundtrip：线性会话", () => {
  it("两轮对话导出→§6.1 JSON 落盘→重载后树逐节点一致（含 activeLeaf）", async () => {
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
  });
});

describe("S4 roundtrip：编辑已发消息（新分支）", () => {
  it("编辑产生兄弟分支，roundtrip 保留分支结构与 activeLeaf 指向新分支", async () => {
    const thread = createRuntime([
      { text: splitChunks("原始回复", 2) },
      { text: splitChunks("编辑后回复", 2) },
    ]);
    await send(thread, "原始问题");
    const state = thread.getState();
    const userV1 = state.messages[0]!.id;

    await editUserMessage(thread, userV1, "编辑后的问题");

    const exported = thread.export();
    // user 两版本（同 parentId=null）+ 各自 assistant
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
  });
});

describe("S4 roundtrip：重生成（reload）", () => {
  it("reload 产生 assistant 兄弟分支，roundtrip 保留", async () => {
    const thread = createRuntime([
      { text: splitChunks("重生成前", 2) },
      { text: splitChunks("重生成后", 2) },
    ]);
    await send(thread, "同一个问题");
    const assistantV1 = thread.getState().messages[1]!.id;

    thread.getMessageById(assistantV1).reload();
    await waitForIdle(thread);

    const exported = thread.export();
    expect(exported.messages).toHaveLength(3); // user + assistant×2（兄弟）
    const assistants = exported.messages.filter((m) => m.message.role === "assistant");
    expect(assistants).toHaveLength(2);
    for (const item of assistants) {
      expect(item.parentId).toBe(exported.messages[0]?.message.id);
    }
    expect(exported.headId).not.toBe(assistantV1);

    const { restored } = roundtripViaFile(exported, "sess-s4-reload", 2);
    assertTreeEqual(exported, restored);
  });
});

describe("S4 roundtrip：分支切换（switchToBranch）", () => {
  it("切回旧分支后 head 指向旧分支叶，roundtrip 保留 activeLeaf", async () => {
    const thread = createRuntime([
      { text: splitChunks("第一版", 2) },
      { text: splitChunks("第二版", 2) },
    ]);
    await send(thread, "问题");
    const assistantV1 = thread.getState().messages[1]!.id;
    thread.getMessageById(assistantV1).reload();
    await waitForIdle(thread);
    const assistantV2 = thread.export().headId!;
    expect(assistantV2).not.toBe(assistantV1);

    // 切回旧分支
    thread.getMessageById(assistantV1).switchToBranch({ branchId: assistantV1 });
    const exported = thread.export();
    expect(exported.headId).toBe(assistantV1);
    expect(thread.getState().messages[1]?.content[0]).toMatchObject({ text: "第一版" });

    const { restored } = roundtripViaFile(exported, "sess-s4-switch", 4);
    assertTreeEqual(exported, restored);

    const reloaded = createRuntime([]);
    reloaded.import(restored);
    // 重载后 activeLeaf 是旧分支叶，可见消息是第一版
    assertTreeEqual(exported, reloaded.export());
    expect(reloaded.getState().messages[1]?.content[0]).toMatchObject({ text: "第一版" });
  });
});

describe("§6.1 存储条目形状（schema 符合性）", () => {
  it("条目顺序与字段面符合设计：session 首、meta 尾、message 居中、epoch 毫秒时间戳", async () => {
    const thread = createRuntime([{ text: splitChunks("回复", 2) }]);
    await send(thread, "问题");
    const exported = thread.export();
    const entries = exportToEntries(exported, "sess-s4-schema", 7);

    expect(entries[0]).toStrictEqual({
      kind: "session",
      v: 1,
      id: "sess-s4-schema",
      createdAt: expect.any(Number),
    });
    const meta = entries.at(-1) as StoredMetaEntry;
    expect(meta).toStrictEqual({
      kind: "meta",
      revision: 7,
      activeLeafId: exported.headId,
    });
    const messages = entries.slice(1, -1) as StoredMessageEntry[];
    expect(messages).toHaveLength(2);
    for (const entry of messages) {
      expect(entry.kind).toBe("message");
      expect(typeof entry.id).toBe("string");
      expect(entry.createdAt).toEqual(expect.any(Number));
      expect(Number.isInteger(entry.createdAt)).toBe(true);
      expect(Array.isArray(entry.parts)).toBe(true);
      expect(["user", "assistant", "system"]).toContain(entry.role);
    }
    // 根消息无 parentId 字段（可选字段省略而非 null）
    expect(messages[0]?.parentId).toBeUndefined();
    expect(messages[1]?.parentId).toBe(messages[0]?.id);
    // assistant 条目保留 status 与 runId（runId 未设置则省略）
    expect(messages[1]?.status).toStrictEqual({ type: "complete", reason: "unknown" });
    expect(messages[1]?.runId).toBeUndefined();
    // user 条目无 status
    expect(messages[0]?.status).toBeUndefined();
  });

  it("reasoning part 映射为 {type:'reasoning'} 且 roundtrip 保真", async () => {
    const thread = createRuntime([
      { reasoning: splitChunks("先想一下", 2), text: splitChunks("想完了", 2) },
    ]);
    await send(thread, "难题");
    const exported = thread.export();
    const assistant = exported.messages.find((m) => m.message.role === "assistant");
    expect(assistant?.message.content.map((p) => p.type)).toStrictEqual(["reasoning", "text"]);

    const entries = exportToEntries(exported, "sess-s4-reasoning", 1);
    const assistantEntry = entries.find(
      (e): e is StoredMessageEntry => e.kind === "message" && e.role === "assistant",
    );
    expect(assistantEntry?.parts.map((p) => p.type)).toStrictEqual(["reasoning", "text"]);

    const restored = entriesToExported(entries);
    assertTreeEqual(exported, restored);
  });
});

describe("toolCall/toolResult 拆分与合并（§3.2 WirePart 口径）", () => {
  it("assistant-ui 单一 tool-call part 导出为 toolCall+toolResult 两条，还原时合并回单 part", () => {
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
  });

  it("无结果的 toolCall（挂起工具）只落 toolCall 一条", () => {
    const repo = ExportedMessageRepositoryFactory.fromBranchableArray([
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
    const entries = exportToEntries(repo, "sess-s4-tool-pending", 1);
    const assistantEntry = entries.find(
      (e): e is StoredMessageEntry => e.kind === "message" && e.role === "assistant",
    );
    expect(assistantEntry?.parts).toStrictEqual([
      { type: "toolCall", callId: "call_x", name: "write_file", argsJson: '{"path":"a"}' },
    ]);
  });
});

describe("落盘形态", () => {
  it("JSONL 行格式与 JSON 文件承载同一份条目（生产 append-only 分段 JSONL 预演）", () => {
    const entries: StoredEntry[] = [
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
    const jsonl = serializeSessionJsonl(entries);
    expect(jsonl.split("\n").filter((line) => line.length > 0)).toHaveLength(3);
    expect(parseSessionJsonl(jsonl)).toStrictEqual(entries);
  });

  it("空树 roundtrip：headId 为 null 不落 activeLeafId", () => {
    const entries = exportToEntries({ headId: null, messages: [] }, "sess-s4-empty", 0);
    expect(entries).toHaveLength(2); // session + meta
    const meta = entries.at(-1) as StoredMetaEntry;
    expect(meta.activeLeafId).toBeUndefined();
    const restored = entriesToExported(entries);
    expect(restored.messages).toHaveLength(0);
    expect(restored.headId ?? null).toBeNull();
  });
});

describe("C-4 实证：0.15.31 导出结构字段面", () => {
  it("ExportedMessageRepository = { headId?, messages:[{message,parentId,runConfig?}] }", async () => {
    const thread = createRuntime([{ text: ["回"] }]);
    await send(thread, "问");
    const repo = thread.export();
    // 顶层字段面
    expect(Object.keys(repo).sort()).toStrictEqual(["headId", "messages"]);
    expect(typeof repo.headId).toBe("string");
    // 条目字段面：恰为 message/parentId（runConfig 未设置则不出现）
    for (const item of repo.messages) {
      expect(Object.keys(item).sort()).toStrictEqual(["message", "parentId"]);
      expect(item.message.id).toEqual(expect.any(String));
      expect(item.message.createdAt).toBeInstanceOf(Date);
    }
    // headId 即叶节点 id（activeLeaf 对应物）
    const nodeIds = new Set(repo.messages.map((m) => m.message.id));
    expect(nodeIds.has(repo.headId!)).toBe(true);
    const headItem = repo.messages.find((m) => m.message.id === repo.headId);
    expect(nodeIds.has(headItem!.parentId ?? "")).toBe(headItem!.parentId !== null);
  });
});

// ---- deriveRunSummaries（17 号票折叠分组数据面） ----

describe("deriveRunSummaries（runMarker 收口摘要推导）", () => {
  it("正常收口与崩溃（endedAt 空）的 run 各自入表", () => {
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
  });

  it("非 system 条目与无 marker 的条目不入表", () => {
    const summaries = deriveRunSummaries([
      { id: "u1", role: "user", createdAt: 1, parts: [{ type: "text", text: "问" }] },
      { id: "s1", role: "system", createdAt: 2, parts: [{ type: "text", text: "" }] },
    ]);
    expect(summaries.size).toBe(0);
  });
});
