/**
 * 【S1 spike 原型，随时可弃】dev-only mock 帧源。
 *
 * 按剧本定时发帧，替代真实 rig 后端（不需要 API key）。两条实现路径产出同构帧：
 * - 浏览器 / vitest：scriptedFrameSource() 本地剧本生成器；
 * - Tauri 桌面：invoke mock_chat_run，Rust 侧经 Channel 发帧（见 src-tauri/src/mock_chat.rs）。
 */
import type { MirrorFrame } from "./frames";

/** 剧本步骤：等待 delayMs 后发 frame。 */
export type ScriptStep = { delayMs: number; frame: FrameInput };

/** 分配式 Omit：对联合类型逐成员剔除 seq。 */
type DistributiveOmit<T, K extends keyof never> = T extends unknown ? Omit<T, K> : never;
type FrameInput = DistributiveOmit<MirrorFrame, "seq">;

/**
 * 验证剧本（覆盖票面全部判定点）：
 * 1. 正文流式；
 * 2. read_workspace_file：参数分 3 段增量到达，前两段为半截 JSON → 「参数准备中」；
 * 3. 参数到齐但结果未到 → 「运行中」；
 * 4. tool-result → 「完成」；
 * 5. run_command：直接完整参数 → 运行中 → isError 结果 → 「错误」态；
 * 6. 收尾正文 + run-ended。
 */
export const MOCK_SCRIPT: ScriptStep[] = [
  { delayMs: 30, frame: { type: "text-delta", delta: "我来查看一下项目文件。" } },
  {
    delayMs: 60,
    frame: { type: "tool-start", toolCallId: "call-1", toolName: "read_workspace_file" },
  },
  // 半截 JSON 增量 ×2（参数准备中）
  {
    delayMs: 60,
    frame: { type: "tool-progress", toolCallId: "call-1", argsTextDelta: '{"path": "src/' },
  },
  {
    delayMs: 60,
    frame: { type: "tool-progress", toolCallId: "call-1", argsTextDelta: 'main.rs", ' },
  },
  // 参数到齐（运行中）
  {
    delayMs: 60,
    frame: { type: "tool-progress", toolCallId: "call-1", argsTextDelta: '"startLine": 1}' },
  },
  {
    delayMs: 120,
    frame: { type: "tool-result", toolCallId: "call-1", result: { content: "fn main() {}" } },
  },
  { delayMs: 60, frame: { type: "tool-start", toolCallId: "call-2", toolName: "run_command" } },
  {
    delayMs: 60,
    frame: {
      type: "tool-progress",
      toolCallId: "call-2",
      argsTextDelta: '{"command": "cargo test"}',
    },
  },
  {
    delayMs: 120,
    frame: {
      type: "tool-result",
      toolCallId: "call-2",
      result: { exitCode: 101, stderr: "编译失败" },
      isError: true,
    },
  },
  { delayMs: 60, frame: { type: "text-delta", delta: "读取完成，但命令执行失败了。" } },
  { delayMs: 30, frame: { type: "run-ended" } },
];

/** 把剧本变成异步帧生成器（seq 自增）。abortSignal 中止即结束。 */
export async function* scriptedFrameSource(
  steps: readonly ScriptStep[] = MOCK_SCRIPT,
  signal?: AbortSignal,
): AsyncGenerator<MirrorFrame> {
  let seq = 0;
  for (const step of steps) {
    if (signal?.aborted) return;
    await new Promise((resolve) => setTimeout(resolve, step.delayMs));
    if (signal?.aborted) return;
    seq += 1;
    yield { ...step.frame, seq } as MirrorFrame;
  }
}

/** 是否运行在 Tauri webview（决定帧源走本地剧本还是 Rust Channel）。 */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * Tauri 桌面路径：Rust mock_chat_run 命令经 Channel 发帧。
 * 包装成与 scriptedFrameSource 同构的 async generator。
 * 仅在 Tauri 环境调用；vitest/浏览器走 scriptedFrameSource。
 */
export function tauriFrameSource(signal?: AbortSignal): AsyncGenerator<MirrorFrame> {
  const queue: MirrorFrame[] = [];
  let notify: (() => void) | null = null;
  let closed = false;

  const wakeup = () => {
    const n = notify;
    notify = null;
    n?.();
  };

  const enqueue = (frame: MirrorFrame) => {
    queue.push(frame);
    wakeup();
  };

  // 动态 import，避免纯浏览器环境（vitest）加载 @tauri-apps/api 的 IPC 初始化
  void import("@tauri-apps/api/core").then(({ Channel, invoke }) => {
    const channel = new Channel<MirrorFrame>();
    channel.onmessage = enqueue;
    // Channel 无 onclose（2.12.4 实测）：命令返回（剧本发完）即视为流结束。
    invoke("mock_chat_run", { channel })
      .catch((err) => {
        console.error("[mirror-prototype] mock_chat_run 调用失败", err);
      })
      .finally(() => {
        closed = true;
        wakeup();
      });
  });

  return (async function* () {
    while (true) {
      if (signal?.aborted) return;
      const next = queue.shift();
      if (next !== undefined) {
        yield next;
        continue;
      }
      if (closed) return;
      await new Promise<void>((resolve) => {
        notify = resolve;
      });
    }
  })();
}

/** 帧源工厂：adapter 每个 run 调一次。 */
export function createFrameSource(signal?: AbortSignal): AsyncGenerator<MirrorFrame> {
  return isTauri() ? tauriFrameSource(signal) : scriptedFrameSource(MOCK_SCRIPT, signal);
}
