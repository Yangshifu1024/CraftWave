import { readdirSync, readFileSync } from "node:fs";
import path, { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";
import { toWire } from "../chat/runtime";
import { CW_TOOL_PARAMETERS } from "../chat/toolkit";
import { entriesToExported, messageToEntry, parseSessionJsonl } from "../chat/tree-persistence";
import { COMMANDS, EVENT_KEYS } from "./client";
import type { MessageEntry, StoredEntry, StoredMessageEntry } from "./types";

/**
 * §12.3 契约测试（前端侧，NFR-12 双向扫描 + 跨语言 roundtrip）。
 *
 * 契约测试是什么：前后端对接口（命令名、参数类型、帧、事件、工具）有一份
 * 共同约定，两侧各自产出一份描述，测试比对两侧必须完全一致——谁改了接口
 * 没同步另一侧，这里就变红。
 *
 * 四个测试面：
 * 1. manifest 扫描（manifest：清单文件 ipc-manifest.json，Rust 侧 cargo test
 *    产出，罗列后端全部命令/帧/事件/工具）：client.ts（唯一 invoke 出口）注册
 *    的命令名全集（COMMANDS 注册表 + 直接 invoke 字面量）必须 ⊆
 *    manifest.commands，且各切片命令在 client.ts 全部接线（双向都不许缺）；
 * 2. 键面卫生：ui/src 全源码不得出现绕过唯一出口的未知 invoke 字面量；
 * 3. 覆盖度：manifest.frames 覆盖 §3.4 P0 全集 9 帧；manifest.tools/toolSpecs
 *    覆盖工具注册表 8 工具且与 toolkit 参数表逐字同源（§10.3，14 号票）；
 *    manifest.events 覆盖 §3.5 实装键且与 EVENT_KEYS 一致；
 * 4. 跨语言 fixture（fixture：测试用的固定样例数据文件）：cargo test 产出的
 *    session-tree-fixture.jsonl（Rust serde 序列化落盘字节，即 storage 分段
 *    JSONL 行格式）——TS 镜像期望值逐条目比对 + 树还原校验；
 * 5. 契约生成物（02 号票，C-2 定案）：payload 类型单一来源 = ts-rs 生成物
 *    ui/src/ipc/generated/（Rust ipc_manifest_test 统一导出、进 git）——
 *    manifest 名义类型 ↔ 生成文件正向一致 + 生成文件 ↔ types.ts 再导出面
 *    反向一致（类型删除后遗留的孤儿生成物在这里红）。
 *
 * 两个产物文件由 `cargo test`（src-tauri/）生成；CI 顺序 = cargo test → vitest
 * （§12.3），缺文件即红（契约不齐不允许静默跳过）。
 */

/** src-tauri 的 target 目录：cargo test 产出的契约文件都放在这里。 */
const targetDir = resolve(import.meta.dirname, "../../../src-tauri/target");
/** ui/src 源码根：键面卫生扫描的起点。 */
const uiSrcDir = fileURLToPath(new URL("..", import.meta.url));

/** cargo test 产出的 IPC 清单（结构见文件头第 1 条），beforeAll 时读入一次。 */
let manifest: {
  commands: { name: string; args_type: string; ret_type: string }[];
  frames: string[];
  events: string[];
  tools: string[];
  toolSpecs: { name: string; paramsSchema: unknown }[];
};

beforeAll(() => {
  const manifestPath = resolve(targetDir, "ipc-manifest.json");
  let raw: string;
  try {
    raw = readFileSync(manifestPath, "utf8");
  } catch {
    throw new Error(
      `契约产物缺失：${manifestPath}\n请先在 src-tauri/ 下运行 cargo test 生成（§12.3 CI 顺序：cargo test → vitest）`,
    );
  }
  manifest = JSON.parse(raw) as typeof manifest;
});

/** 读 target 目录下的契约产物文件；缺失时直接报错并提示先跑 cargo test
 *  （而不是静默跳过——契约不齐必须红）。 */
function readArtifact(name: string): string {
  const artifactPath = resolve(targetDir, name);
  try {
    return readFileSync(artifactPath, "utf8");
  } catch {
    throw new Error(
      `契约产物缺失：${artifactPath}\n请先在 src-tauri/ 下运行 cargo test 生成（§12.3 CI 顺序：cargo test → vitest）`,
    );
  }
}

/** client.ts 注册的命令名全集：COMMANDS 注册表值 + 直接 invoke 字面量
 *  （后者也要收进来，防止有人绕过注册表硬写字面量而漏检）。 */
function scannedClientCommandNames(): string[] {
  const source = readFileSync(resolve(import.meta.dirname, "./client.ts"), "utf8");
  const names = new Set<string>(Object.values(COMMANDS));
  for (const match of source.matchAll(/invoke\(\s*"([a-z_]+)"/g)) {
    names.add(match[1]!);
  }
  expect(names.size, "client.ts 应至少注册一个 invoke 命令").toBeGreaterThan(0);
  return [...names];
}

/** 本测试直接扫描的源文件（client.ts 为唯一 invoke 出口）。 */

/** S5 spike 的前端取消桥接为注入式原型；cancel_run 已随核心链路票正式注册。 */
const SPIKE_ONLY_INVOKES = new Set<string>([]);

describe("§12.3 manifest 双向扫描", () => {
  it("client.ts 注册的命令名全部存在于 manifest.commands", () => {
    // 前端用到的每个命令，Rust 清单都必须注册（防止前端调一个后端不存在的命令）
    const commands = new Set(manifest.commands.map((c) => c.name));
    for (const name of scannedClientCommandNames()) {
      expect(commands, `命令 ${name} 未在 manifest 中注册`).toContain(name);
    }
  });

  it("storage/provider/chat 切片命令在 client.ts 全部接线（双向）", () => {
    const wired = new Set(scannedClientCommandNames());
    for (const name of [
      "save_session_tree",
      "load_session_tree",
      "get_config",
      "save_config",
      "save_provider",
      "delete_provider",
      "open_dir",
      "start_chat",
      "cancel_run",
      "inject_run_message",
      "resume_followup_queue",
      "remove_followup",
      "resolve_ask",
      "approve_plan",
      "compact_session",
      "resolve_exit_request",
      "notification_permission_status",
      "open_notification_settings",
    ]) {
      expect(wired, `manifest 命令 ${name} 未在 client.ts 接线（禁止只注册不使用）`).toContain(
        name,
      );
    }
  });

  it("两切片命令的契约类型名与文档锚点一致", () => {
    // 不止命令名要对上：参数/返回值两侧的类型名也必须一致（防止只改了形状没改名）
    const find = (name: string) => {
      const command = manifest.commands.find((c) => c.name === name);
      expect(command, `manifest 缺命令 ${name}`).toBeDefined();
      return command!;
    };
    expect(find("save_session_tree")).toMatchObject({
      args_type: "SaveSessionTreeArgs",
      ret_type: "SaveSessionTreeResult",
    });
    expect(find("load_session_tree")).toMatchObject({
      args_type: "LoadSessionTreeArgs",
      ret_type: "SessionTreeData",
    });
    expect(find("get_config")).toMatchObject({ args_type: "()", ret_type: "AppConfig" });
    expect(find("save_config")).toMatchObject({
      args_type: "SaveConfigArgs",
      ret_type: "AppConfig",
    });
    expect(find("save_provider")).toMatchObject({
      args_type: "SaveProviderArgs",
      ret_type: "ProviderConfig",
    });
    expect(find("delete_provider")).toMatchObject({
      args_type: "DeleteProviderArgs",
      ret_type: "()",
    });
    // 数据目录入口（03 号票，§3.3 #3）
    expect(find("open_dir")).toMatchObject({
      args_type: "OpenDirArgs",
      ret_type: "()",
    });
    // 核心链路两命令（13 号票，§3.3 #18/#19）
    expect(find("start_chat")).toMatchObject({
      args_type: "StartChatArgs",
      ret_type: "StartChatResult",
    });
    expect(find("cancel_run")).toMatchObject({
      args_type: "CancelRunArgs",
      ret_type: "bool",
    });
    // steer/followUp 队列三命令（06 号票，§3.3 #21/#59d/#59e）
    expect(find("inject_run_message")).toMatchObject({
      args_type: "InjectRunMessageArgs",
      ret_type: "InjectRunMessageResult",
    });
    expect(find("resume_followup_queue")).toMatchObject({
      args_type: "ResumeFollowupQueueArgs",
      ret_type: "ResumeFollowupQueueResult",
    });
    expect(find("remove_followup")).toMatchObject({
      args_type: "RemoveFollowupArgs",
      ret_type: "RemoveFollowupResult",
    });
    // ask/计划门命令（14 号票，§3.3 #22/#26c）
    expect(find("resolve_ask")).toMatchObject({
      args_type: "ResolveAskArgs",
      ret_type: "()",
    });
    expect(find("approve_plan")).toMatchObject({
      args_type: "ApprovePlanArgs",
      ret_type: "()",
    });
    // 退出确认与通知引导（17/19 号票，§9.6）
    expect(find("resolve_exit_request")).toMatchObject({
      args_type: "ResolveExitRequestArgs",
      ret_type: "()",
    });
    expect(find("notification_permission_status")).toMatchObject({
      args_type: "()",
      ret_type: "NotificationPermissionStatus",
    });
    expect(find("open_notification_settings")).toMatchObject({
      args_type: "()",
      ret_type: "()",
    });
  });

  it("ui/src 全源码无绕过唯一出口的未知 invoke 字面量", () => {
    // 递归扫描 ui/src 所有 ts/tsx 源码里的 invoke("…") 字面量：
    // 出现 manifest 未登记的命令名即违规（绕过了唯一出口的约束）
    const manifestNames = new Set(manifest.commands.map((c) => c.name));
    const offenders: string[] = [];
    const walk = (dir: string) => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) {
          walk(full);
        } else if (/\.(ts|tsx)$/.test(entry.name) && !entry.name.endsWith(".test.ts")) {
          const source = readFileSync(full, "utf8");
          for (const match of source.matchAll(/invoke[<(]\s*["'`]([^"'`]+)["'`]/g)) {
            const name = match[1];
            if (name && !manifestNames.has(name) && !SPIKE_ONLY_INVOKES.has(name)) {
              offenders.push(`${path.relative(uiSrcDir, full)}: ${name}`);
            }
          }
        }
      }
    };
    walk(uiSrcDir);
    expect(offenders, `未知 invoke 命令：\n${offenders.join("\n")}`).toEqual([]);
  });

  it("manifest.frames 覆盖 §3.4 P0 全集 9 帧 + M2 压缩帧/摘要帧", () => {
    // 帧清单必须恰好是 P0 全集 9 帧 + 04 号票 compactDone + 05 号票 runSnapshot：
    // 一个不能少（且不允许混入未知帧）
    expect(manifest.frames).toHaveLength(11);
    for (const frame of [
      "runStarted",
      "stepStarted",
      "textDelta",
      "reasoningDelta",
      "toolStarted",
      "toolArgs",
      "toolResult",
      "usage",
      "runEnded",
      "compactDone",
      "runSnapshot",
    ]) {
      expect(manifest.frames).toContain(frame);
    }
  });

  it("manifest.tools 覆盖工具注册表 8 工具（§8.3 M1 模型可见子集）", () => {
    expect(manifest.tools).toEqual([
      "read",
      "write",
      "edit",
      "list_files",
      "grep",
      "command",
      "plan",
      "ask",
    ]);
  });

  it("toolkit 参数表与 manifest.toolSpecs 逐字同源（§10.3 镜像声明）", () => {
    // Rust 工具参数 schema 与前端 toolkit 声明必须逐字一致（模型卡渲染依赖它）
    expect(manifest.toolSpecs).toHaveLength(8);
    for (const spec of manifest.toolSpecs) {
      const local = (CW_TOOL_PARAMETERS as Record<string, unknown>)[spec.name];
      expect(local, `toolkit 缺工具 ${spec.name} 的参数声明`).toBeDefined();
      expect(local, `工具 ${spec.name} 参数 schema 与 Rust ToolSpec 不同源`).toStrictEqual(
        spec.paramsSchema,
      );
    }
    // 双向：toolkit 声明不超出注册表
    const manifestNames = new Set(manifest.toolSpecs.map((s) => s.name));
    for (const name of Object.keys(CW_TOOL_PARAMETERS)) {
      expect(manifestNames, `toolkit 多出未注册工具 ${name}`).toContain(name);
    }
  });

  it("manifest.events 覆盖 §3.5 实装 14 键且与 EVENT_KEYS 一致", () => {
    // 事件清单双保险：manifest 恰好 17 键（14/17 号票 10 键 + 04 号票压缩
    // 三键 + 06 号票 run:start/run:inject/run:queue 三键 + 08 号票
    // mcp:status），且与前端 EVENT_KEYS 注册表完全一致
    expect(manifest.events).toEqual([
      "ask:opened",
      "ask:closed",
      "plan:update",
      "run:suggestions",
      "tokens:update",
      "run:done",
      "run:error",
      "run:cancelled",
      "run:start",
      "run:inject",
      "run:queue",
      "notify:activate",
      "app:exit_requested",
      "run:compacting",
      "run:compacted",
      "run:compact_failed",
      "mcp:status",
    ]);
    expect(Object.values(EVENT_KEYS).sort()).toEqual([...manifest.events].sort());
  });

  it("手动压缩命令（04 号票，§3.3 #59b）在两侧接线", () => {
    const command = manifest.commands.find((c) => c.name === "compact_session");
    expect(command).toMatchObject({
      args_type: "CompactSessionArgs",
      ret_type: "CompactSessionResult",
    });
    // 双向：client.ts 已接线（禁止只注册不使用）
    expect(Object.values(COMMANDS)).toContain("compact_session");
  });

  it("MCP 八命令（08 号票，§3.3 #40–47）在两侧接线且契约类型名一致", () => {
    const find = (name: string) => {
      const command = manifest.commands.find((c) => c.name === name);
      expect(command, `manifest 缺命令 ${name}`).toBeDefined();
      return command!;
    };
    // #40 双作用域配置读取（空参名义类型注册，返回 { global, project,
    // projectAvailable }）
    expect(find("mcp_list_config")).toMatchObject({
      args_type: "McpListConfigArgs",
      ret_type: "McpListConfigResult",
    });
    // #41 整份保存一个作用域（未知字段随 flatten round-trip 不丢）
    expect(find("mcp_save_config")).toMatchObject({
      args_type: "McpSaveConfigArgs",
      ret_type: "()",
    });
    // #42–44 连接（触发 TOFU）/断开/重连
    expect(find("mcp_connect")).toMatchObject({ args_type: "McpServerRefArgs", ret_type: "()" });
    expect(find("mcp_disconnect")).toMatchObject({
      args_type: "McpServerRefArgs",
      ret_type: "()",
    });
    expect(find("mcp_reconnect")).toMatchObject({
      args_type: "McpServerRefArgs",
      ret_type: "()",
    });
    // #45 草稿试连（总预算 60s，不进池）
    expect(find("mcp_test")).toMatchObject({ args_type: "McpTestArgs", ret_type: "McpTestResult" });
    // #46 TOFU 决策（remember/revoke，落 trust/mcp.json）
    expect(find("mcp_set_launch_trust")).toMatchObject({
      args_type: "McpSetLaunchTrustArgs",
      ret_type: "()",
    });
    // #47 连接池状态快照（空参名义类型注册）
    expect(find("mcp_snapshot")).toMatchObject({
      args_type: "McpSnapshotArgs",
      ret_type: "McpSnapshotResult",
    });
    // 双向：client.ts 已接线（禁止只注册不使用）
    for (const name of [
      "mcp_list_config",
      "mcp_save_config",
      "mcp_connect",
      "mcp_disconnect",
      "mcp_reconnect",
      "mcp_test",
      "mcp_set_launch_trust",
      "mcp_snapshot",
    ]) {
      expect(Object.values(COMMANDS), `${name} 应在 client.ts 接线`).toContain(name);
    }
    // mcp:status 事件（§3.5 M2 键）已在 EVENT_KEYS 注册（与 manifest 一致性
    // 由上方 events 断言把守，这里防误删）
    expect(Object.values(EVENT_KEYS)).toContain("mcp:status");
  });

  it("帧重放与热重连三命令（05 号票，§3.3 #20/#57/#59c）在两侧接线", () => {
    const find = (name: string) => {
      const command = manifest.commands.find((c) => c.name === name);
      expect(command, `manifest 缺命令 ${name}`).toBeDefined();
      return command!;
    };
    // attach_run（#57）：帧重放重连，幂等空重放语义
    expect(find("attach_run")).toMatchObject({
      args_type: "AttachRunArgs",
      ret_type: "AttachRunResult",
    });
    // session_run_snapshot（#20）：对账快照，无活跃 run → null（Option）
    expect(find("session_run_snapshot")).toMatchObject({
      args_type: "SessionRunSnapshotArgs",
      ret_type: "Option<RunSnapshot>",
    });
    // session_pending_ask（#59c）：待审批恢复查询，无未决 ask → null（Option）
    expect(find("session_pending_ask")).toMatchObject({
      args_type: "SessionPendingAskArgs",
      ret_type: "Option<SessionPendingAsk>",
    });
    // 双向：client.ts 已接线（禁止只注册不使用）
    for (const name of ["attach_run", "session_run_snapshot", "session_pending_ask"]) {
      expect(Object.values(COMMANDS), `${name} 应在 client.ts 接线`).toContain(name);
    }
  });

  it("runSnapshot 摘要帧（05 号票，§3.4）与 RunSnapshot 类型生成物同源", async () => {
    // 帧面 runSnapshot 的负载对齐 §3.2 RunSnapshot（chat/frames.ts 消费面）
    type ChatFrame = import("./generated/ChatFrame").ChatFrame;
    type RunSnapshotFrame = Extract<ChatFrame, { type: "runSnapshot" }>;
    const sample: RunSnapshotFrame = {
      type: "runSnapshot",
      status: "running",
      runId: "r1",
      startedAt: 1,
      turn: 1,
      toolCount: 0,
      usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      lastSeq: 0,
    };
    expect(sample.status).toBe("running");
    // RunSnapshot 契约类型（命令返回 Option<RunSnapshot>，生成物已再导出）
    const snapshot: import("./generated/RunSnapshot").RunSnapshot = {
      status: "running",
      runId: "r1",
      startedAt: 1,
      turn: 1,
      toolCount: 0,
      usage: { input: 1, output: 2, cacheRead: 0, cacheWrite: 0 },
      lastSeq: 0,
    };
    expect(snapshot.lastSeq).toBe(0);
  });
});

// ---- 跨语言 fixture 期望值：与 src-tauri/src/ipc_manifest_test.rs 的 fixture_entries() 逐字段一致 ----

/** TS 侧对 fixture 内容的镜像期望（与 Rust 侧测试里的期望值一字不差，两边同守）。 */
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
  {
    kind: "message",
    id: "run-2:marker",
    parentId: "a3",
    role: "system",
    parts: [
      {
        type: "runMarker",
        runId: "run-2",
        startedAt: 1760000000005,
        endedAt: 1760000000006,
        durationMs: 1200,
        toolCount: 0,
      },
    ],
    createdAt: 1760000000006,
  },
  {
    kind: "compaction",
    id: "manual:compact:1",
    cutEntryId: "a3",
    summary: "早期对话摘要",
    tokensBefore: 9000,
    createdAt: 1760000000007,
  },
  { kind: "meta", activeLeafId: "run-2:marker", revision: 4 },
];

describe("§6.1 跨语言 fixture roundtrip（Rust serde 落盘 ↔ TS 镜像）", () => {
  const fixtureText = readArtifact("session-tree-fixture.jsonl");

  it("Rust 落盘字节解析后与 TS 镜像期望逐条目一致（含 status 对象/可选字段省略）", () => {
    expect(parseSessionJsonl(fixtureText)).toStrictEqual(EXPECTED_ENTRIES);
  });

  it("fixture 还原为 assistant-ui 树：结构/activeLeaf/status/toolCall 合并全部正确", () => {
    const repo = entriesToExported(parseSessionJsonl(fixtureText));

    // 树结构（C-4：headId=activeLeaf，parentId null=根；导出序父先子后）。
    // compaction 条目被跳过（§6.3 定稿：不投影为树节点）——6 条 message
    // 的 exact 断言即防线：混入 compaction 节点立即红。
    expect(repo.headId).toBe("run-2:marker");
    expect(repo.messages.map((m) => m.message.id)).toStrictEqual([
      "u1",
      "a1",
      "u2",
      "a2",
      "a3",
      "run-2:marker",
    ]);
    const parentOf = (id: string): string | null =>
      repo.messages.find((m) => m.message.id === id)?.parentId ?? null;
    expect(parentOf("u1")).toBeNull();
    expect(parentOf("a1")).toBe("u1");
    expect(parentOf("u2")).toBe("a1");
    expect(parentOf("a2")).toBe("u2");
    expect(parentOf("a3")).toBe("u2");
    // runMarker 链尾：a3 → marker（canonical 收口行，前端 custom 载荷保真）
    expect(parentOf("run-2:marker")).toBe("a3");

    // 历史兄弟分支：a2/a3 同父
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

  it("compaction 条目不进树、行可读回（§6.3 不投影定稿的 roundtrip 面）", () => {
    const entries = parseSessionJsonl(fixtureText);
    const compactions = entries.filter(
      (e): e is Extract<StoredEntry, { kind: "compaction" }> => e.kind === "compaction",
    );
    expect(compactions).toHaveLength(1);
    expect(compactions[0]).toMatchObject({
      id: "manual:compact:1",
      cutEntryId: "a3",
      summary: "早期对话摘要",
      tokensBefore: 9000,
    });
    // 树还原后无 compaction 节点（save 载荷与史册无冲突，无 E_SESSION_REV 面）
    const repo = entriesToExported(entries);
    expect(repo.messages.map((m) => m.message.id)).not.toContain("manual:compact:1");
  });

  it("保存载荷形状：messages = 存储条目剥离 kind 后的 MessageEntry（TS→Rust 方向）", () => {
    // 反方向校验：save 载荷的 messages 恰好是存储条目去掉 kind 判别字段后的形态
    const messages = EXPECTED_ENTRIES.filter((e) => e.kind === "message");
    const payload: MessageEntry[] = messages.map((entry) => {
      const { kind: _kind, ...rest } = entry;
      return rest;
    });
    expect(payload).toHaveLength(6);
    expect(payload.every((m) => !("kind" in m))).toBe(true);
  });

  it("往返等价（15 号票防漂移锚点）：条目 → 树还原 → messageToEntry 导出逐字段一致", () => {
    // Rust canonical 条目经前端 import（entryToMessageLike + assistant-ui 规范化）
    // 再导出（messageToEntry）必须与原条目逐字段等价——不等价即 save 全树同步时
    // 同 id 内容漂移 → E_SESSION_REV（保存失败级故障，本断言是唯一自动防线）
    const entries = parseSessionJsonl(fixtureText);
    const repo = entriesToExported(entries);
    const original = entries.filter((e): e is StoredMessageEntry => e.kind === "message");
    const round = repo.messages.map((m) => messageToEntry(m.message, m.parentId));
    expect(round).toStrictEqual(original);
  });

  it("runMarker 节点的 wire 面等价：树上 system 空消息 → toWire 还原 RunMarker part", () => {
    // 活动路径含 marker 节点时（收养后继续对话），toWire 不得把它变空 parts 条目
    const entries = parseSessionJsonl(fixtureText);
    const repo = entriesToExported(entries);
    const wire = toWire(repo.messages.map((m) => m.message));
    const markerWire = wire.find((m) => m.id === "run-2:marker");
    expect(markerWire).toMatchObject({
      id: "run-2:marker",
      role: "system",
      parts: [
        {
          type: "runMarker",
          runId: "run-2",
          startedAt: 1760000000005,
          endedAt: 1760000000006,
          durationMs: 1200,
          toolCount: 0,
        },
      ],
    });
  });
});

// ---- 契约生成物（02 号票，C-2 定案）：payload 类型单一来源 = ts-rs 生成物 ----

describe("§12.3 契约生成物：ts-rs generated/ ↔ manifest ↔ types.ts 再导出面", () => {
  /** 生成物目录（cargo test 统一导出、进 git；types.ts 逐文件再导出）。 */
  const generatedDir = resolve(import.meta.dirname, "./generated");

  /** 递归列出 generated/ 下全部 .ts（posix 相对路径，含 serde_json/JsonValue.ts）。 */
  function listGenerated(dir: string = generatedDir): string[] {
    const out: string[] = [];
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        out.push(...listGenerated(full));
      } else if (entry.name.endsWith(".ts")) {
        out.push(path.relative(generatedDir, full).split(path.sep).join("/"));
      }
    }
    return out.sort();
  }

  /** types.ts 声明的再导出面（export * from "./generated/X" 的 X 集合）。 */
  function reExportedNames(): string[] {
    const source = readFileSync(resolve(import.meta.dirname, "./types.ts"), "utf8");
    return [...source.matchAll(/export \* from "\.\/generated\/(.+?)";/g)].map((m) => m[1]!);
  }

  it("manifest 每条名义 args_type/ret_type 在 generated/ 有同名导出（防生成物缺失）", () => {
    const files = new Set(listGenerated());
    for (const command of manifest.commands) {
      // 名义类型 = 单个标识符；()、bool、Vec<_>、Option<_>、HashMap<_,_> 等
      // 非名义类型无独立生成文件；Value（serde_json::Value 短名）的形状由
      // serde_json/JsonValue.ts 承载。与 Rust 侧 ipc_manifest_test 同口径。
      for (const ty of [command.args_type, command.ret_type]) {
        const trimmed = ty.trim();
        const nominal =
          trimmed !== "" &&
          trimmed !== "bool" &&
          trimmed !== "Value" &&
          /^[A-Za-z_][A-Za-z0-9_]*$/.test(trimmed);
        if (nominal) {
          expect(
            files.has(`${trimmed}.ts`),
            `manifest 命令 ${command.name} 的契约类型 ${trimmed} 缺生成物`,
          ).toBe(true);
        }
      }
    }
  });

  it("generated/ 与 types.ts 再导出面双向一致（孤儿生成物/漏再导出即红）", () => {
    const files = new Set(listGenerated());
    const exported = new Set(reExportedNames());
    expect(exported.size, "types.ts 应逐文件再导出生成物").toBeGreaterThan(0);
    for (const file of files) {
      // serde_json/JsonValue.ts 以 "serde_json/JsonValue" 名义再导出
      expect(
        exported.has(file.replace(/\.ts$/, "")),
        `生成物 ${file} 未被 types.ts 再导出（Rust 类型删除后的孤儿文件？）`,
      ).toBe(true);
    }
    for (const name of exported) {
      expect(files.has(`${name}.ts`), `types.ts 再导出了不存在的生成物 ${name}.ts`).toBe(true);
    }
  });
});
