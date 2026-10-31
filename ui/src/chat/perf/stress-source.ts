/**
 * NFR-03 压测数据源（34 号票，dev-only）：合成一条 2000 条消息的长会话。
 *
 * 条目形状与真实会话完全同构（storage 分段 JSONL 的 StoredEntry 契约类型），
 * 经 tree-persistence 的 entriesToExported 灌入真实渲染管线——合成的是数据，
 * 消费它的是生产路径（树还原 → assistant-ui import → Thread 渲染），不是
 * 捷径渲染器。多样性覆盖票面点名四类：runMarker 分组、工具卡、长文本、
 * 图片 part；压缩边界条目单独返回（§6.3：compaction 不进消息树，投影走
 * compaction-store，与 SessionWorkspace 的收养路径一致）。
 *
 * 仅 dev 构建可达：唯一入口在 main.tsx 的 `import.meta.env.DEV` 分支内，
 * 生产构建常量折叠后该分支与动态 import 一并剔除。
 */

import type { StoredCompactionEntry, StoredEntry, StoredMessageEntry } from "@/ipc/types";

/** 压测会话使用的虚拟 sessionId（只进前端内存态，不触碰 Rust 会话存储）。 */
export const STRESS_SESSION_ID = "stress-perf";

/** 合成会话的默认消息条数（PRD NFR-03 前半口径：单会话 ≥2000 条）。 */
export const STRESS_MESSAGE_COUNT = 2000;

/** runMarker 分组粒度：每 100 条消息收口一个 run（共 20 组）。 */
const MESSAGES_PER_RUN = 100;

/** 压缩边界条数：按 0.6 阈值口径 2000 条长会话会触发多次压缩的示意投影。 */
const COMPACTION_COUNT = 3;

/** 构造一段指定字数的中文长文本（重复填充，内容不影响渲染性能特征——
 *  性能由长度与 Markdown 结构决定）。 */
function longText(id: string, chars: number): string {
  const sentence = "这是压测合成的长文本段落，用来把单条消息的渲染成本推到接近真实长回复的水平。";
  const head = `### 压测消息 ${id}\n\n`;
  let body = "";
  while (body.length < chars) {
    body += sentence;
  }
  // 末尾补一个列表与代码块，让流式 Markdown 的结构化渲染路径也被覆盖
  return `${head}${body}\n\n- 列表项一\n- 列表项二\n\n\`​\`\`​ts\nconst stress = ${chars};\n\`​\`\`​\n`;
}

/** assistant 消息的四种形态轮转（覆盖长文本/工具卡/混合/短文本的渲染面）。 */
const SHAPE_KINDS = 4;

/** 单条合成消息的构造结果（收窄到 message 变体，便于访问 id）。 */
interface SyntheticPair {
  /** user 条目（轮次提问）。 */
  user: StoredMessageEntry;
  /** assistant 条目（回复）。 */
  assistant: StoredMessageEntry;
}

/** 构造第 n 轮（n 从 0 起）的一对消息；parent 为上一条消息的 id。 */
function buildRound(n: number, parent: string | null): SyntheticPair {
  const userId = `s-u${n}`;
  const assistantId = `s-a${n}`;
  const createdAt = 1_760_000_000_000 + n * 2;
  // 每 400 条消息（200 轮）放一张图片 part，2000 条共 4 张：走 image part
  // 渲染管线（真实会话里经 read_image_blob 取数；压测环境 blob 不存在 →
  // 灰显占位，仍是渲染负载的一部分）
  const withImage = n % 200 === 0 && n > 0;
  // 链头（n=0）无父：parentId 省略字段（契约口径 null→省略，§6.3）
  const user: StoredMessageEntry = {
    kind: "message",
    id: userId,
    ...(parent ? { parentId: parent } : {}),
    role: "user",
    parts: [
      { type: "text", text: `压测第 ${n + 1} 轮提问${withImage ? "（附截图）" : ""}` },
      ...(withImage
        ? [{ type: "image" as const, blobHash: "0".repeat(32), mimeType: "image/png" }]
        : []),
    ],
    createdAt,
  };

  // 形态轮转：0=长文本、1=工具卡×2、2=reasoning+工具卡+长文本、3=短文本
  const shape = n % SHAPE_KINDS;
  const runId = `stress-run-${Math.floor(n / (MESSAGES_PER_RUN / 2)) + 1}`;
  const assistantParts: Array<
    | { type: "text"; text: string }
    | { type: "reasoning"; text: string }
    | { type: "toolCall"; callId: string; name: string; argsJson: string }
    | { type: "toolResult"; callId: string; ok: boolean; summary: string }
  > = [];
  if (shape === 0) {
    assistantParts.push({ type: "reasoning", text: `第 ${n + 1} 轮的思考过程占位。` });
    assistantParts.push({ type: "text", text: longText(assistantId, 800) });
  } else if (shape === 1) {
    for (const k of [0, 1]) {
      assistantParts.push({
        type: "toolCall",
        callId: `s-call-${n}-${k}`,
        name: k === 0 ? "read_workspace_file" : "run_command",
        argsJson: k === 0 ? '{"path":"README.md"}' : '{"command":"cargo test"}',
      });
      assistantParts.push({
        type: "toolResult",
        callId: `s-call-${n}-${k}`,
        ok: k === 0,
        summary: k === 0 ? "# README\n压测占位内容。" : "test result: ok. 42 passed",
      });
    }
    assistantParts.push({ type: "text", text: `第 ${n + 1} 轮工具调用完成，结论见上方卡片。` });
  } else if (shape === 2) {
    assistantParts.push({ type: "reasoning", text: `第 ${n + 1} 轮的混合形态思考占位。` });
    assistantParts.push({
      type: "toolCall",
      callId: `s-call-${n}-mix`,
      name: "search_workspace_paths",
      argsJson: '{"query":"perf"}',
    });
    assistantParts.push({
      type: "toolResult",
      callId: `s-call-${n}-mix`,
      ok: true,
      summary: "src/chat/perf/stress-source.ts",
    });
    assistantParts.push({ type: "text", text: longText(assistantId, 400) });
  } else {
    assistantParts.push({ type: "text", text: `第 ${n + 1} 轮收到。` });
  }

  const assistant: StoredMessageEntry = {
    kind: "message",
    id: assistantId,
    parentId: userId,
    role: "assistant",
    parts: assistantParts,
    createdAt: createdAt + 1,
    runId,
    status: { type: "complete", reason: "stop" },
  };
  return { user, assistant };
}

/** 合成会话的完整产物。 */
export interface StressDataset {
  /** 会话条目全集（消息 + runMarker + meta），按存储序线性排列。 */
  entries: StoredEntry[];
  /** 压缩边界投影（§6.3 不进树，走 compaction-store）。 */
  compactions: StoredCompactionEntry[];
  /** 消息条目总数（user + assistant，不含 runMarker/meta）。 */
  messageCount: number;
}

/**
 * 生成 2000 条消息的合成会话。
 *
 * 链形 = 线性（真实长会话的主路径；分支树展开另有 roundtrip 单测覆盖）；
 * runMarker 系统条目每 100 条消息收口一次（20 组，17 号票折叠分组的分组面）；
 * meta 的 activeLeafId 指向最后一条消息（整链活跃）。
 */
export function buildStressEntries(messageCount: number = STRESS_MESSAGE_COUNT): StressDataset {
  if (messageCount % 2 !== 0) {
    throw new Error(`合成会话消息数必须为偶数（user/assistant 成对），收到 ${messageCount}`);
  }
  const rounds = messageCount / 2;
  const entries: StoredEntry[] = [];
  let parent: string | null = null;
  let lastNodeId: string | null = null;
  const runMarkers: { runId: string; cutAfter: string }[] = [];

  for (let n = 0; n < rounds; n++) {
    const { user, assistant } = buildRound(n, parent);
    entries.push(user, assistant);
    parent = assistant.id;
    lastNodeId = assistant.id;
    // 每 100 条消息（50 轮）插一个 runMarker 系统条目：role=system、链上
    // 节点（15 号票 canonical 口径——marker 也是树节点，占位不渲染正文，
    // 承担 17 号票折叠分组的锚点），继续把链尾接到 marker 上
    const messagesSoFar = (n + 1) * 2;
    if (messagesSoFar % MESSAGES_PER_RUN === 0) {
      const runIndex = Math.floor(messagesSoFar / MESSAGES_PER_RUN);
      const runId = `stress-run-${runIndex}`;
      const markerId = `${runId}:marker`;
      entries.push({
        kind: "message",
        id: markerId,
        parentId: parent,
        role: "system",
        parts: [
          {
            type: "runMarker",
            runId,
            startedAt: 1_760_000_000_000 + n * 2,
            endedAt: 1_760_000_000_000 + n * 2 + 1,
            durationMs: 30_000 + runIndex * 1000,
            toolCount: MESSAGES_PER_RUN / 4,
          },
        ],
        createdAt: 1_760_000_000_000 + n * 2 + 1,
      });
      parent = markerId;
      lastNodeId = markerId;
      runMarkers.push({ runId, cutAfter: markerId });
    }
  }

  // meta 收尾：activeLeafId 指向链尾节点（整链活跃，与真实单线会话一致；
  // 末组 marker 是叶子，同样合法）
  entries.push({
    kind: "meta",
    revision: messageCount,
    ...(lastNodeId ? { activeLeafId: lastNodeId } : {}),
  });

  // 压缩边界投影：三条落在 runMarker 之后（cutEntryId 指向真实存在的
  // marker id），tokensBefore 沿 0.6 阈值口径逐次抬升
  const compactions: StoredCompactionEntry[] = [];
  for (let c = 0; c < COMPACTION_COUNT; c++) {
    const marker = runMarkers[Math.min(c * 6, runMarkers.length - 1)];
    if (!marker) break;
    compactions.push({
      id: `auto:compact:${c + 1}`,
      cutEntryId: marker.cutAfter,
      summary: `压测压缩边界 ${c + 1}：早期 ${messagesBeforeCut(c)} 条消息已摘要。`,
      tokensBefore: 40_000 * (c + 1),
      createdAt: 1_760_000_000_000 + 1000 * (c + 1),
    });
  }

  return { entries, compactions, messageCount };
}

/** 压缩摘要文案用的条数辅助（第 c 次压缩覆盖到第几个 marker 为止）。 */
function messagesBeforeCut(compactionIndex: number): number {
  return Math.min((compactionIndex + 1) * 6, 19) * MESSAGES_PER_RUN;
}
