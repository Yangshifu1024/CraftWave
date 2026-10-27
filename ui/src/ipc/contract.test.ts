import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { entriesToExported, parseSessionJsonl } from "../chat/tree-persistence";
import type { MessageEntry, StoredEntry } from "./types";

/**
 * §12.3 契约测试（前端侧，NFR-12 双向扫描 + 跨语言 roundtrip）：
 *
 * 1. manifest 扫描：client.ts 中每个 invoke 命令名 ∈ ipc-manifest.json 的 commands；
 * 2. 跨语言 fixture：cargo test 产出的 session-tree-fixture.jsonl（Rust serde 落盘
 *    字节即 storage 分段 JSONL 行格式）——TS 镜像期望值逐条目比对 + 树还原校验。
 *
 * 两个产物文件由 `cargo test`（src-tauri/）生成；CI 顺序 = cargo test → vitest（§12.3），
 * 缺文件即红（契约不齐不允许静默跳过）。
 */

const targetDir = resolve(import.meta.dirname, "../../../src-tauri/target");

function readArtifact(name: string): string {
  const path = resolve(targetDir, name);
  try {
    return readFileSync(path, "utf8");
  } catch {
    throw new Error(
      `契约产物缺失：${path}\n请先在 src-tauri/ 下运行 cargo test 生成（§12.3 CI 顺序：cargo test → vitest）`,
    );
  }
}

type ManifestCommand = { name: string; args_type: string; ret_type: string };

function readManifestCommands(): ManifestCommand[] {
  const manifest = JSON.parse(readArtifact("ipc-manifest.json")) as {
    commands: ManifestCommand[];
  };
  expect(Array.isArray(manifest.commands)).toBe(true);
  return manifest.commands;
}

/** 扫描 client.ts 源码中全部 invoke("命令名") 字面量（§12.3：前端 invoke 名单扫描）。 */
function scannedInvokeNames(): string[] {
  const source = readFileSync(resolve(import.meta.dirname, "./client.ts"), "utf8");
  const names: string[] = [];
  for (const match of source.matchAll(/invoke\(\s*"([a-z_]+)"/g)) {
    names.push(match[1]!);
  }
  expect(names.length, "client.ts 应至少注册一个 invoke 命令").toBeGreaterThan(0);
  return names;
}

// ---- 跨语言 fixture 期望值：与 src-tauri/src/ipc_manifest_test.rs 的 fixture_entries() 逐字段一致 ----

const EXPECTED_ENTRIES: StoredEntry[] = [
  { kind: "session", v: 1, id: "sess-fixture", createdAt: 1760000000000 },
  {
    kind: "message",
    id: "u1",
    role: "user",
    parts: [{ type: "text", text: "你好" }],
    createdAt: 1760000000001,
  },
  {
    kind: "message",
    id: "a1",
    parentId: "u1",
    role: "assistant",
    parts: [
      { type: "reasoning", text: "思考中" },
      { type: "text", text: "你好！我是 CraftWave。" },
    ],
    createdAt: 1760000000002,
    status: { type: "complete", reason: "unknown" },
  },
  {
    kind: "message",
    id: "u2",
    parentId: "a1",
    role: "user",
    parts: [{ type: "text", text: "读一下 README" }],
    createdAt: 1760000000003,
  },
  {
    kind: "message",
    id: "a2",
    parentId: "u2",
    role: "assistant",
    parts: [
      {
        type: "toolCall",
        callId: "call_1",
        name: "read_workspace_file",
        argsJson: '{"path":"README.md"}',
      },
      { type: "toolResult", callId: "call_1", ok: true, summary: "# CraftWave\n第一版说明。" },
      { type: "text", text: "README 读完了。" },
    ],
    createdAt: 1760000000004,
    runId: "run-1",
    status: { type: "complete", reason: "stop" },
  },
  {
    kind: "message",
    id: "a3",
    parentId: "u2",
    role: "assistant",
    parts: [{ type: "text", text: "重试：README 共 3 个章节。" }],
    createdAt: 1760000000005,
    runId: "run-2",
    status: { type: "complete", reason: "unknown" },
  },
  { kind: "meta", activeLeafId: "a3", revision: 3 },
];

describe("§12.3 manifest 双向扫描", () => {
  it("client.ts 的每个 invoke 名都在 manifest.commands 内", () => {
    const commands = readManifestCommands().map((c) => c.name);
    for (const name of scannedInvokeNames()) {
      expect(commands).toContain(name);
    }
  });

  it("存储两命令的契约类型名与文档锚点一致", () => {
    const commands = readManifestCommands();
    const save = commands.find((c) => c.name === "save_session_tree");
    const load = commands.find((c) => c.name === "load_session_tree");
    expect(save).toMatchObject({
      args_type: "SaveSessionTreeArgs",
      ret_type: "SaveSessionTreeResult",
    });
    expect(load).toMatchObject({
      args_type: "LoadSessionTreeArgs",
      ret_type: "SessionTreeData",
    });
  });
});

describe("§6.1 跨语言 fixture roundtrip（Rust serde 落盘 ↔ TS 镜像）", () => {
  const fixtureText = readArtifact("session-tree-fixture.jsonl");

  it("Rust 落盘字节解析后与 TS 镜像期望逐条目一致（含 status 对象/可选字段省略）", () => {
    expect(parseSessionJsonl(fixtureText)).toStrictEqual(EXPECTED_ENTRIES);
  });

  it("fixture 还原为 assistant-ui 树：结构/activeLeaf/status/toolCall 合并全部正确", () => {
    const repo = entriesToExported(parseSessionJsonl(fixtureText));

    // 树结构（C-4：headId=activeLeaf，parentId null=根；导出序父先子后）
    expect(repo.headId).toBe("a3");
    expect(repo.messages.map((m) => m.message.id)).toStrictEqual(["u1", "a1", "u2", "a2", "a3"]);
    const parentOf = (id: string): string | null =>
      repo.messages.find((m) => m.message.id === id)?.parentId ?? null;
    expect(parentOf("u1")).toBeNull();
    expect(parentOf("a1")).toBe("u1");
    expect(parentOf("u2")).toBe("a1");
    expect(parentOf("a2")).toBe("u2");
    expect(parentOf("a3")).toBe("u2");

    // 重生成兄弟分支：a2/a3 同父
    expect(parentOf("a2")).toBe(parentOf("a3"));

    // status 对象保真（complete/unknown 与 complete/stop 两种 reason），user 无 status
    const messageOf = (id: string) => repo.messages.find((m) => m.message.id === id)!.message;
    expect(messageOf("a1").status).toStrictEqual({ type: "complete", reason: "unknown" });
    expect(messageOf("a2").status).toStrictEqual({ type: "complete", reason: "stop" });
    expect(messageOf("u1")).not.toHaveProperty("status");

    // runId 保真（metadata.runId，assistant 变体）与 createdAt epoch 毫秒保真
    const a2 = repo.messages.find((m) => m.message.id === "a2")!.message;
    if (a2.role !== "assistant") throw new Error("a2 应为 assistant 消息");
    expect(a2.metadata.runId).toBe("run-1");
    expect(messageOf("a1").createdAt.getTime()).toBe(1760000000002);

    // reasoning part 保真
    expect(messageOf("a1").content.map((p) => p.type)).toStrictEqual(["reasoning", "text"]);

    // toolCall+toolResult 合并回单一 tool-call part（argsJson 解析、result/isError 口径）
    const callPart = messageOf("a2").content.find((p) => p.type === "tool-call");
    expect(callPart).toMatchObject({
      type: "tool-call",
      toolCallId: "call_1",
      toolName: "read_workspace_file",
      args: { path: "README.md" },
      result: "# CraftWave\n第一版说明。",
      isError: false,
    });
  });

  it("保存载荷形状：messages = 存储条目剥离 kind 后的 MessageEntry（TS→Rust 方向）", () => {
    const messages = EXPECTED_ENTRIES.filter((e) => e.kind === "message");
    const payload: MessageEntry[] = messages.map((entry) => {
      const { kind: _kind, ...rest } = entry;
      return rest;
    });
    expect(payload).toHaveLength(5);
    expect(payload.every((m) => !("kind" in m))).toBe(true);
  });
});
