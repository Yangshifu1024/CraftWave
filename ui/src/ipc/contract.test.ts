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
 * 契约测试（前端侧，双向扫描 + 跨语言 roundtrip）。
 *
 * 契约测试是什么：前后端对接口（命令名、参数类型、帧、事件、工具）有一份
 * 共同约定，两侧各自产出一份描述，测试比对两侧必须完全一致——谁改了接口
 * 没同步另一侧，这里就变红。
 *
 * 测试面（原逐命令用例已按域合并为表驱动/剧本，覆盖语义不减）：
 * 1. manifest 双向扫描：client.ts（唯一 invoke 出口）注册的命令名全集
 *    （COMMANDS 注册表 + 直接 invoke 字面量）⊆ manifest.commands，且各切片
 *    命令在 client.ts 全部接线（双向都不许缺）；键面卫生：ui/src 全源码
 *    不得出现绕过唯一出口的未知 invoke 字面量；
 * 2. 命令契约类型名：全部命令（storage/provider/chat、MCP、技能、外部授权、
 *    子代理、git、用量、产物/日志/更新器等）的 args_type/ret_type 与文档锚点
 *    逐条一致，且逐条在 client.ts 接线；
 * 3. 覆盖度：manifest.frames 覆盖核心 9 帧 + 压缩/摘要/子代理/
 *    审计帧；manifest.tools/toolSpecs 覆盖工具注册表 17 工具且与 toolkit
 *    参数表逐字同源；manifest.events 覆盖实装事件键
 *    且与 EVENT_KEYS 一致；
 * 4. 跨语言 fixture（fixture：测试用的固定样例数据文件）：cargo test 产出的
 *    session-tree-fixture.jsonl（Rust serde 序列化落盘字节，即 storage 分段
 *    JSONL 行格式）——TS 镜像期望值逐条目比对 + 树还原 + 往返等价校验；
 * 5. 契约生成物：payload 类型单一来源 = ts-rs 生成物
 *    ui/src/ipc/generated/（Rust ipc_manifest_test 统一导出、进 git）——
 *    manifest 名义类型 ↔ 生成文件正向一致 + 生成文件 ↔ types.ts 再导出面
 *    反向一致（类型删除后遗留的孤儿生成物在这里红）+ 关键生成物键面与
 *    字段级形状（信任/审计/审批/计划门/用量/git/更新器）。
 *
 * 两个产物文件由 `cargo test`（src-tauri/）生成；CI 顺序 = cargo test → vitest，
 * 缺文件即红（契约不齐不允许静默跳过）。
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
      `契约产物缺失：${manifestPath}\n请先在 src-tauri/ 下运行 cargo test 生成（CI 顺序：cargo test → vitest）`,
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
      `契约产物缺失：${artifactPath}\n请先在 src-tauri/ 下运行 cargo test 生成（CI 顺序：cargo test → vitest）`,
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

/** manifest 里查一条命令（缺失即红）。 */
function findCommand(name: string) {
  const command = manifest.commands.find((c) => c.name === name);
  expect(command, `manifest 缺命令 ${name}`).toBeDefined();
  return command!;
}

/** S5 spike 的前端取消桥接为注入式原型；cancel_run 已随核心链路票正式注册。 */
const SPIKE_ONLY_INVOKES = new Set<string>([]);

describe("manifest 双向扫描与键面卫生", () => {
  it("client.ts 注册命令 ⊆ manifest.commands；切片命令反向全部接线；全源码无未知 invoke 字面量", () => {
    // 正向：前端用到的每个命令，Rust 清单都必须注册（防止前端调一个后端不存在的命令）
    const commands = new Set(manifest.commands.map((c) => c.name));
    const wired = new Set(scannedClientCommandNames());
    for (const name of wired) {
      expect(commands, `命令 ${name} 未在 manifest 中注册`).toContain(name);
    }
    // 反向：storage/provider/chat/核心链路/ask/偏好/退出确认/通知/连接测试等切片
    // 命令在 client.ts 全部接线（禁止只注册不使用）
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
      // 会话偏好两命令
      "get_session_prefs",
      "set_session_prefs",
      "resolve_exit_request",
      "notification_permission_status",
      "open_notification_settings",
      // 供应商连接测试
      "test_provider",
    ]) {
      expect(wired, `manifest 命令 ${name} 未在 client.ts 接线（禁止只注册不使用）`).toContain(
        name,
      );
    }
    // 键面卫生：递归扫描 ui/src 所有 ts/tsx 源码里的 invoke("…") 字面量，
    // 出现 manifest 未登记的命令名即违规（绕过唯一出口的约束）
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
});

/** 全部命令的契约类型名（与 manifest 锚点一致）。
 *  [命令名, { args_type, ret_type }]——每个条目同时做「manifest 契约类型名一致」
 *  与「client.ts 已接线（双向）」两断言。 */
const COMMAND_CONTRACTS: [string, { args_type: string; ret_type: string }][] = [
  // storage/provider/chat 切片
  ["save_session_tree", { args_type: "SaveSessionTreeArgs", ret_type: "SaveSessionTreeResult" }],
  ["load_session_tree", { args_type: "LoadSessionTreeArgs", ret_type: "SessionTreeData" }],
  ["get_config", { args_type: "()", ret_type: "AppConfig" }],
  ["save_config", { args_type: "SaveConfigArgs", ret_type: "AppConfig" }],
  ["save_provider", { args_type: "SaveProviderArgs", ret_type: "ProviderConfig" }],
  ["delete_provider", { args_type: "DeleteProviderArgs", ret_type: "()" }],
  ["open_dir", { args_type: "OpenDirArgs", ret_type: "()" }],
  ["start_chat", { args_type: "StartChatArgs", ret_type: "StartChatResult" }],
  ["cancel_run", { args_type: "CancelRunArgs", ret_type: "bool" }],
  // steer/followUp 队列三命令
  ["inject_run_message", { args_type: "InjectRunMessageArgs", ret_type: "InjectRunMessageResult" }],
  [
    "resume_followup_queue",
    { args_type: "ResumeFollowupQueueArgs", ret_type: "ResumeFollowupQueueResult" },
  ],
  ["remove_followup", { args_type: "RemoveFollowupArgs", ret_type: "RemoveFollowupResult" }],
  // ask/计划门命令
  ["resolve_ask", { args_type: "ResolveAskArgs", ret_type: "()" }],
  ["approve_plan", { args_type: "ApprovePlanArgs", ret_type: "()" }],
  // 手动压缩
  ["compact_session", { args_type: "CompactSessionArgs", ret_type: "CompactSessionResult" }],
  // 会话偏好两命令
  ["get_session_prefs", { args_type: "SessionPrefsArgs", ret_type: "GetSessionPrefsResult" }],
  ["set_session_prefs", { args_type: "SetSessionPrefsArgs", ret_type: "()" }],
  // 退出确认与通知引导
  ["resolve_exit_request", { args_type: "ResolveExitRequestArgs", ret_type: "()" }],
  ["notification_permission_status", { args_type: "()", ret_type: "NotificationPermissionStatus" }],
  ["open_notification_settings", { args_type: "()", ret_type: "()" }],
  // 供应商连接测试
  ["test_provider", { args_type: "TestProviderArgs", ret_type: "TestProviderResult" }],
  // MCP 八命令：双作用域配置读取、整份保存、连接（触发 TOFU）/断开/重连、
  // 草稿试连（总预算 60s 不进池）、TOFU 决策（remember/revoke）、连接池状态快照
  ["mcp_list_config", { args_type: "McpListConfigArgs", ret_type: "McpListConfigResult" }],
  ["mcp_save_config", { args_type: "McpSaveConfigArgs", ret_type: "()" }],
  ["mcp_connect", { args_type: "McpServerRefArgs", ret_type: "()" }],
  ["mcp_disconnect", { args_type: "McpServerRefArgs", ret_type: "()" }],
  ["mcp_reconnect", { args_type: "McpServerRefArgs", ret_type: "()" }],
  ["mcp_test", { args_type: "McpTestArgs", ret_type: "McpTestResult" }],
  ["mcp_set_launch_trust", { args_type: "McpSetLaunchTrustArgs", ret_type: "()" }],
  ["mcp_snapshot", { args_type: "McpSnapshotArgs", ret_type: "McpSnapshotResult" }],
  // 技能四命令：多来源合并清单（元素非名义根）、启停切换、重扫全部来源、正文预览
  ["skill_list", { args_type: "()", ret_type: "Vec<SkillListEntry>" }],
  ["skill_toggle", { args_type: "SkillToggleArgs", ret_type: "()" }],
  ["skill_reload", { args_type: "()", ret_type: "()" }],
  ["skill_inspect", { args_type: "SkillInspectArgs", ret_type: "SkillInspectResult" }],
  // 外部目录授权（预检/授权 + Inspector 撤销补充面）
  [
    "check_external_path",
    { args_type: "CheckExternalPathArgs", ret_type: "CheckExternalPathResult" },
  ],
  ["allow_external_dir", { args_type: "AllowExternalDirArgs", ret_type: "ExternalRootsResult" }],
  ["revoke_external_dir", { args_type: "RevokeExternalDirArgs", ret_type: "ExternalRootsResult" }],
  // 技能信任撤销（撤销 UI 落盘面）
  ["skill_revoke_trust", { args_type: "SkillRevokeTrustArgs", ret_type: "()" }],
  // 子代理两命令：级联取消、历史回看
  ["stop_subagent", { args_type: "StopSubagentArgs", ret_type: "()" }],
  [
    "load_subagent_history",
    { args_type: "LoadSubagentHistoryArgs", ret_type: "Vec<MessageEntry>" },
  ],
  // 帧重放与热重连三命令：幂等空重放、
  // 无活跃 run → null（Option）、无未决 ask → null（Option）
  ["attach_run", { args_type: "AttachRunArgs", ret_type: "AttachRunResult" }],
  [
    "session_run_snapshot",
    { args_type: "SessionRunSnapshotArgs", ret_type: "Option<RunSnapshot>" },
  ],
  [
    "session_pending_ask",
    { args_type: "SessionPendingAskArgs", ret_type: "Option<SessionPendingAsk>" },
  ],
  // 用量统计两命令
  ["stats_token", { args_type: "StatsTokenArgs", ret_type: "StatsTokenResult" }],
  ["session_usage", { args_type: "SessionUsageArgs", ret_type: "SessionUsageResult" }],
  // git 只读三命令
  ["git_status", { args_type: "GitStatusArgs", ret_type: "GitStatusResult" }],
  ["git_diff", { args_type: "GitDiffArgs", ret_type: "GitDiffResult" }],
  ["git_recent_log", { args_type: "GitLogArgs", ret_type: "GitLogResult" }],
  // 产物面板与编辑器 + 会话日志三命令
  ["list_artifacts", { args_type: "ListArtifactsArgs", ret_type: "ListArtifactsResult" }],
  ["list_editors", { args_type: "()", ret_type: "Vec<EditorEntry>" }],
  ["open_in_editor", { args_type: "OpenInEditorArgs", ret_type: "()" }],
  ["read_session_log", { args_type: "ReadSessionLogArgs", ret_type: "ReadSessionLogResult" }],
  ["list_log_files", { args_type: "()", ret_type: "Vec<String>" }],
  ["open_logs_dir", { args_type: "()", ret_type: "()" }],
  // 更新器三命令（状态门/安装门/主动暂停）
  ["update_status", { args_type: "()", ret_type: "UpdateStatusResult" }],
  ["update_precheck", { args_type: "()", ret_type: "UpdatePrecheckResult" }],
  [
    "pause_followup_queue",
    { args_type: "PauseFollowupQueueArgs", ret_type: "PauseFollowupQueueResult" },
  ],
];

describe("命令契约类型名（表驱动扫描全部命令）", () => {
  it("每条命令的 args_type/ret_type 与文档锚点一致，且在 client.ts 接线（双向）；session:title 无新命令", () => {
    for (const [name, types] of COMMAND_CONTRACTS) {
      // 不止命令名要对上：参数/返回值两侧的类型名也必须一致（防止只改了形状没改名）
      expect(findCommand(name)).toMatchObject(types);
      // 双向：client.ts 已接线（禁止只注册不使用）
      expect(Object.values(COMMANDS), `${name} 应在 client.ts 接线`).toContain(name);
    }
    // 自动起名（事件键 session:title）：零新增命令
    //（导出为纯前端序列化；起名为后台任务无命令面）
    expect(Object.values(EVENT_KEYS)).toContain("session:title");
    expect(Object.values(COMMANDS)).not.toContain("session_title");
  });
});

describe("覆盖度：帧/工具/事件", () => {
  it("manifest.frames 16 帧恰好、tools 17 工具恰好、toolSpecs 与 toolkit 逐字同源（双向）、events 与 EVENT_KEYS 一致", () => {
    // 帧清单必须恰好是核心 9 帧 + compactDone + runSnapshot +
    // 子代理 4 帧（subStarted/subFrame/subReport/subDone）+
    // toolAudit（灰区命令审计五态）：一个不能少
    //（且不允许混入未知帧）
    expect(manifest.frames).toHaveLength(16);
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
      "subStarted",
      "subFrame",
      "subReport",
      "subDone",
      "toolAudit",
    ]) {
      expect(manifest.frames).toContain(frame);
    }

    // 工具清单（模型可见子集 + skill + subagent + web 三工具
    // + 小工具包四件）
    expect(manifest.tools).toEqual([
      "read",
      "write",
      "edit",
      "list_files",
      "grep",
      "command",
      "plan",
      "ask",
      "skill",
      "subagent",
      "web_fetch",
      "http_request",
      "web_search",
      "batch_read",
      "glob",
      "calculate",
      "render_html",
    ]);

    // Rust 工具参数 schema 与前端 toolkit 声明必须逐字一致（模型卡渲染依赖它），
    // 且双向：toolkit 声明不超出注册表
    expect(manifest.toolSpecs).toHaveLength(17);
    for (const spec of manifest.toolSpecs) {
      const local = (CW_TOOL_PARAMETERS as Record<string, unknown>)[spec.name];
      expect(local, `toolkit 缺工具 ${spec.name} 的参数声明`).toBeDefined();
      expect(local, `工具 ${spec.name} 参数 schema 与 Rust ToolSpec 不同源`).toStrictEqual(
        spec.paramsSchema,
      );
    }
    const specNames = new Set(manifest.toolSpecs.map((s) => s.name));
    for (const name of Object.keys(CW_TOOL_PARAMETERS)) {
      expect(specNames, `toolkit 多出未注册工具 ${name}`).toContain(name);
    }

    // 事件清单双保险：manifest 恰好 24 键（询问/计划/建议/token/run 终态等
    // 10 键 + 压缩三键 + run:start/run:inject/run:queue 三键 +
    // mcp:status + sub:* 六键 + session:title），
    // 且与前端 EVENT_KEYS 注册表完全一致
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
      "sub:spawn",
      "sub:step",
      "sub:report",
      "sub:usage",
      "sub:done",
      "sub:error",
      "session:title",
    ]);
    expect(Object.values(EVENT_KEYS).sort()).toEqual([...manifest.events].sort());
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
  // 带图 user 消息（image part）：行内只含 blobHash/mimeType
  // 引用，不含图片本体（体积不膨胀验收的跨语言锚点）；行序在分支链
  // （a2/a3/marker）之后——父先子后的存储纪律保持
  {
    kind: "message",
    id: "u3",
    parentId: "a2",
    role: "user",
    parts: [
      { type: "text", text: "看这张截图" },
      {
        type: "image",
        blobHash: "a".repeat(32),
        mimeType: "image/png",
      },
    ],
    createdAt: 1760000000008,
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

describe("跨语言 fixture roundtrip（Rust serde 落盘 ↔ TS 镜像）", () => {
  const fixtureText = readArtifact("session-tree-fixture.jsonl");

  it("Rust 落盘字节解析后与 TS 镜像逐条目一致（含 status 对象/可选字段省略）；保存载荷 = 存储条目剥离 kind（TS→Rust 方向）", () => {
    expect(parseSessionJsonl(fixtureText)).toStrictEqual(EXPECTED_ENTRIES);
    // 反方向校验：save 载荷的 messages 恰好是存储条目去掉 kind 判别字段后的形态
    const messages = EXPECTED_ENTRIES.filter((e) => e.kind === "message");
    const payload: MessageEntry[] = messages.map((entry) => {
      const { kind: _kind, ...rest } = entry;
      return rest;
    });
    expect(payload).toHaveLength(7);
    expect(payload.every((m) => !("kind" in m))).toBe(true);
  });

  it("树还原 + 往返等价（防漂移锚点）+ compaction 不投影 + runMarker 的 wire 面等价", () => {
    const entries = parseSessionJsonl(fixtureText);
    const repo = entriesToExported(entries);

    // 树结构（headId=activeLeaf，parentId null=根；导出序父先子后）。
    // compaction 条目被跳过（不投影为树节点）——7 条 message
    // 的 exact 断言即防线：混入 compaction 节点立即红。
    expect(repo.headId).toBe("run-2:marker");
    expect(repo.messages.map((m) => m.message.id)).toStrictEqual([
      "u1",
      "a1",
      "u2",
      "a2",
      "a3",
      "run-2:marker",
      "u3",
    ]);
    const parentOf = (id: string): string | null =>
      repo.messages.find((m) => m.message.id === id)?.parentId ?? null;
    expect(parentOf("u1")).toBeNull();
    expect(parentOf("a1")).toBe("u1");
    expect(parentOf("u2")).toBe("a1");
    expect(parentOf("a2")).toBe("u2");
    expect(parentOf("u3")).toBe("a2");
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

    // compaction 条目可读回（上方 toStrictEqual 已含）、树还原后无 compaction 节点
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
    expect(repo.messages.map((m) => m.message.id)).not.toContain("manual:compact:1");

    // 往返等价（防漂移锚点）：Rust canonical 条目经前端 import 再导出
    // 必须与原条目逐字段等价——不等价即 save 全树同步时同 id 内容漂移 →
    // E_SESSION_REV（保存失败级故障，本断言是唯一自动防线）
    const original = entries.filter((e): e is StoredMessageEntry => e.kind === "message");
    const round = repo.messages.map((m) => messageToEntry(m.message, m.parentId));
    expect(round).toStrictEqual(original);

    // 活动路径含 marker 节点时（收养后继续对话），toWire 不得把它变空 parts 条目
    const wire = toWire(repo.messages.map((m) => m.message));
    expect(wire.find((m) => m.id === "run-2:marker")).toMatchObject({
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

// ---- 契约生成物：payload 类型单一来源 = ts-rs 生成物 ----

describe("契约生成物：ts-rs generated/ ↔ manifest ↔ types.ts 再导出面", () => {
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

  it("manifest 每条名义 args_type/ret_type 在 generated/ 有同名导出；generated/ 与 types.ts 再导出面双向一致", () => {
    // 正向：名义类型必须有同名生成文件（防生成物缺失）；()、bool、Vec<_>、
    // Option<_>、HashMap<_,_> 等非名义类型无独立生成文件；Value（serde_json
    // ::Value 短名）的形状由 serde_json/JsonValue.ts 承载（与 Rust 侧同口径）
    const files = new Set(listGenerated());
    for (const command of manifest.commands) {
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
    // 反向：每个生成文件都被 types.ts 再导出（孤儿生成物即红），且再导出
    // 不指向不存在的文件
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

/** 读生成物文本（断言字段级键面）。 */
function generated(name: string): string {
  return readFileSync(resolve(import.meta.dirname, "./generated", `${name}.ts`), "utf8");
}

describe("生成物键面与字段级形状（信任/审计/审批/计划门/用量/git/更新器）", () => {
  it("生成物键面：信任面 + 审计四面 + 审批/白名单/计划门/选项", () => {
    // 信任面：SessionMeta 生成物含授权字段（会话 roots 快照
    // 持久化字段）；AskPayload 含 externalDir 形态（路径 + 选项）与 projectTrust
    // 的 allow_session 键面（授权口径统一：allow_once → allow_session）
    expect(generated("SessionMeta")).toContain("authorizedExternalDirs");
    const askPayload = generated("AskPayload");
    expect(askPayload).toContain('"kind": "externalDir"');
    expect(askPayload).toContain("allow_session");
    // 灰区命令审计：toolAudit 帧（type 判别 + state/reason）在
    // ChatFrame 生成物；审计五态（running/allow/block/ask/manual）；
    // AskPayload 带 audit 注记（ask/manual 两态 + reason）；审计专用模型配置
    const chatFrame = generated("ChatFrame");
    expect(chatFrame).toContain('"toolAudit"');
    expect(chatFrame).toContain("AuditState");
    const auditState = generated("AuditState");
    for (const state of ["running", "allow", "block", "ask", "manual"]) {
      expect(auditState).toContain(`"${state}"`);
    }
    expect(askPayload).toContain("audit?: AuditNote");
    expect(generated("AgentConfig")).toContain("commandAudit");
    expect(generated("CommandAuditConfig")).toContain("modelId");
    // 审批记忆/白名单/计划门：ApprovalConfig 七字段
    //（四开关 + 白名单 + 计划硬门 requirePlan + git 写策略 gitAllowStageCommit）
    const approvalConfig = generated("ApprovalConfig");
    for (const field of [
      "enabled",
      "autoConfirm",
      "sessionMemoryEnabled",
      "allowlistEnabled",
      "commandAllowlist",
      "requirePlan",
      "gitAllowStageCommit",
    ]) {
      expect(approvalConfig.includes(field), `ApprovalConfig 缺字段 ${field}`).toBe(true);
    }
    // 范围门询问载荷（approval.planScope = 计划外写路径）
    expect(askPayload).toContain("planScope");
    // approval 选项 mode 键面（AskMode/AskOption 生成物）
    expect(generated("AskMode")).toContain('"allow_once" | "always_session" | "deny"');
    const askOption = generated("AskOption");
    for (const field of ["id", "label", "recommended", "mode"]) {
      expect(askOption.includes(field), `AskOption 缺字段 ${field}`).toBe(true);
    }
    // AskAnswer.approval 的 optionId 键面保留（always_session 应答承载）
    expect(generated("AskAnswer")).toContain("optionId");
  });

  it("生成物字段级类型面（runSnapshot/用量/git/产物/日志/更新器/队列暂停）：TS 实例化编译期守形 + 代表字段断言", () => {
    // runSnapshot 摘要帧负载对齐 RunSnapshot 契约（chat/frames.ts 消费面）
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

    // 用量统计：byDay/byModel/total 与生成物同构；
    // legacyTotalTokens 可选（旧数据标量回落，不伪造拆分）
    const day: import("./generated/DayUsage").DayUsage = {
      day: "2026-10-30",
      input: 1,
      output: 2,
      cacheRead: 3,
      cacheWrite: 4,
      total: 10,
    };
    const model: import("./generated/ModelUsage").ModelUsage = {
      modelId: "m1",
      modelName: "gpt-4o",
      input: 1,
      output: 2,
      cacheRead: 3,
      cacheWrite: 4,
      total: 10,
      runs: 2,
    };
    const statsResult: import("./generated/StatsTokenResult").StatsTokenResult = {
      byDay: [day],
      byModel: [model],
      total: { input: 1, output: 2, cacheRead: 3, cacheWrite: 4, total: 10, runs: 2 },
    };
    expect(statsResult.byDay[0]!.day).toBe("2026-10-30");
    expect(statsResult.byModel[0]!.runs).toBe(2);
    const usageResult: import("./generated/SessionUsageResult").SessionUsageResult = {
      sessionId: "s1",
      usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      legacyTotalTokens: 777,
    };
    expect(usageResult.legacyTotalTokens).toBe(777);

    // git 只读三命令：生成物字段级回写形态
    const statusFile: import("./generated/GitStatusFile").GitStatusFile = {
      path: "a.ts",
      status: "M",
    };
    expect(statusFile.status).toBe("M");
    const status: import("./generated/GitStatusResult").GitStatusResult = {
      isRepo: true,
      files: [statusFile],
      user: { name: "tester", email: "tester@test" },
    };
    expect(status.isRepo).toBe(true);
    const patch: import("./generated/DiffPatch").DiffPatch = {
      path: "a.ts",
      addLines: 1,
      delLines: 2,
      binary: false,
      truncated: false,
      hunks: [{ header: "@@ -1,2 +1,2 @@", lines: [{ kind: "add", text: "x" }] }],
    };
    expect(patch.hunks[0]!.lines[0]!.kind).toBe("add");
    const log: import("./generated/GitLogResult").GitLogResult = {
      isRepo: true,
      commits: [{ sha: "a".repeat(40), summary: "init", time: 0 }],
    };
    expect(log.commits[0]!.sha).toHaveLength(40);

    // 会话日志：tail 可缺省 = Option；lines 必有
    const logArgs: import("./generated/ReadSessionLogArgs").ReadSessionLogArgs = {
      sessionId: "s1",
    };
    expect(logArgs.sessionId).toBe("s1");
    const logResult: import("./generated/ReadSessionLogResult").ReadSessionLogResult = {
      lines: ['{"ts":"2026-10-30T00:00:00Z","level":"info","kind":"run","msg":"run 开始"}'],
    };
    expect(logResult.lines).toHaveLength(1);

    // 产物面板与编辑器：ArtifactRow 四字段 + EditorEntry 两字段
    const row: import("./generated/ArtifactRow").ArtifactRow = {
      path: "/proj/a.txt",
      runCount: 2,
      lastSeenAt: 1760000000000,
      exists: true,
    };
    expect(row.runCount).toBe(2);
    expect(row.exists).toBe(true);
    const artifactsResult: import("./generated/ListArtifactsResult").ListArtifactsResult = {
      artifacts: [row],
    };
    expect(artifactsResult.artifacts).toHaveLength(1);
    const editor: import("./generated/EditorEntry").EditorEntry = {
      name: "VS Code",
      path: "/usr/local/bin/code",
    };
    expect(editor.name).toBe("VS Code");

    // 更新器：禁用门两字段 + 安装门六字段 + 队列快照两字段
    const updateStatus: import("./generated/UpdateStatusResult").UpdateStatusResult = {
      pubkeyConfigured: false,
      endpoints: ["https://github.com/o/r/releases/latest/download/latest.json"],
    };
    expect(updateStatus.pubkeyConfigured).toBe(false);
    const precheck: import("./generated/UpdatePrecheckResult").UpdatePrecheckResult = {
      blocked: true,
      reasons: ["有 1 个会话的任务正在运行（或等待审批），安装重启会中断运行"],
      runningCount: 1,
      pendingAskCount: 0,
      runningQueueCount: 0,
      pausedQueueCount: 0,
    };
    expect(precheck.blocked).toBe(true);
    expect(precheck.reasons).toHaveLength(1);
    const paused: import("./generated/PauseFollowupQueueResult").PauseFollowupQueueResult = {
      size: 2,
      paused: true,
    };
    expect(paused.size).toBe(2);
    expect(paused.paused).toBe(true);
    // update 配置域（enabled 缺省 true / intervalHours 缺省 24）
    const update: import("./generated/UpdateConfig").UpdateConfig = {
      enabled: true,
      intervalHours: 24,
    };
    expect(update.enabled).toBe(true);
    expect(update.intervalHours).toBe(24);
  });
});
