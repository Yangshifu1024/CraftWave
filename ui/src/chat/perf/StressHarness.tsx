/**
 * NFR-03 长会话压测 harness（34 号票，dev-only）。
 *
 * 目的：在真实硬件上量化「单会话 ≥2000 条消息（含压缩后）滚动与发送不卡顿」
 * （PRD NFR-03 前半口径）。做法：合成 2000 条消息灌入**真实渲染管线**
 * （entriesToExported → assistant-ui thread.import → 生产 ChatThread，含
 * Markdown 流式组件、工具卡、折叠分组、压缩分隔投影），两种负载各跑一遍：
 *
 * 1. 滚动负载：滚动容器首尾往返 10 次；
 * 2. 发送负载：ScriptedBackend 流式一轮（reasoning + 文本增量分片），
 *    在 2000 条会话尾部追加渲染。
 *
 * 指标：rAF 采样帧间隔，>100ms 渲染长帧计数 = 0 为通过线（与 NFR-02 同
 * 量级；该量化口径 PRD 未给数值，34 号票定为评审可调注记，见
 * docs/specs/m2-release/nfr-checklist.md 的 NFR-03 行）。
 *
 * 跑法两态：
 * - 手动：URL ?stress=1 进入后点控制条按钮（35 号用户自验/复测用）；
 * - 无人值守：URL ?stress=1&auto=1 挂载后自动跑完整序列并把报告 POST 回
 *   dev server（vite.config.ts 的 dev-only 中间件落盘 target/stress-report.json）。
 *
 * 仅 dev 构建可达：唯一入口在 main.tsx 的 `import.meta.env.DEV` 分支，
 * 生产构建常量折叠后模块整体剔除（已在 34 号票实测产物零残留）。
 */

import type { ChatModelAdapter } from "@assistant-ui/react";
import { AssistantRuntimeProvider, AuiConfig, Tools, useLocalRuntime } from "@assistant-ui/react";
import { useCallback, useEffect, useRef, useState } from "react";
import { CompactionDivider } from "../CompactionUI";
import { useCompactionStore } from "../compaction-store";
import type { ReplyScript } from "../scripted-backend";
import { RunSummariesContext, SessionScopeContext } from "../session-scope";
import { ChatThread } from "../Thread";
import { cwToolkit } from "../toolkit";
import { deriveRunSummaries, entriesToExported } from "../tree-persistence";
import { type FrameSampleReport, FrameSampler } from "./frame-sampler";
import { buildStressEntries, STRESS_MESSAGE_COUNT, STRESS_SESSION_ID } from "./stress-source";

/** 生产同款工具卡注册（压测页与主界面共用同一渲染管线）。 */
const stressAuiConfig = AuiConfig({ tools: Tools({ toolkit: cwToolkit }) });

/** 滚动压测往返次数（票面口径：首尾往返 10 次）。 */
const SCROLL_ROUNDS = 10;

/** 发送负载剧本：reasoning 10 段 + 文本 60 段（每段 8~14 字），模拟一次
 *  正常长度的流式回复（64ms 节流后的帧节奏远快于增量段数，采样能覆盖
 *  渲染侧真实帧间隔）。 */
const SEND_SCRIPT: ReplyScript = {
  reasoning: Array.from({ length: 10 }, (_, i) => `思考第 ${i + 1} 步：定位渲染热点。`),
  text: Array.from(
    { length: 60 },
    (_, i) => `这是发送压测的第 ${i + 1} 段流式增量，用于驱动逐字渲染管线。`,
  ),
};

/** 下一个动画帧（Promise 包装）。 */
function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

/** 等渲染沉淀：双 rAF（一帧提交 React 更新、一帧等浏览器完成绘制）。 */
async function settle(): Promise<void> {
  await nextFrame();
  await nextFrame();
}

/**
 * 压测流式 adapter：剧本段间等待两帧再产出下一段——真实链路的 64ms 节流
 * 保证增量间有宏任务间隙，ScriptedBackend 的同步 yield 会把整个 run 压进
 * 一个宏任务（rAF 采样不到渲染帧），此处还原真实节奏让采样有帧可记。
 */
function createStressStreamAdapter(script: ReplyScript): ChatModelAdapter {
  return {
    async *run({ abortSignal }) {
      let reasoningAcc = "";
      for (const chunk of script.reasoning ?? []) {
        if (abortSignal.aborted) return;
        await settle();
        reasoningAcc += chunk;
        yield { content: [{ type: "reasoning", text: reasoningAcc }] };
      }
      let textAcc = "";
      for (const chunk of script.text) {
        if (abortSignal.aborted) return;
        await settle();
        textAcc += chunk;
        yield {
          content: reasoningAcc
            ? [
                { type: "reasoning", text: reasoningAcc },
                { type: "text", text: textAcc },
              ]
            : [{ type: "text", text: textAcc }],
        };
      }
    },
  };
}

/** 上报给 dev server 的完整压测报告（含环境元信息，写入 target/stress-report.json）。 */
export interface StressRunPayload {
  capturedAt: string;
  userAgent: string;
  hardwareConcurrency: number;
  messageCount: number;
  /** 预热往返（首屏冷渲染）参考数据，不计通过线。 */
  warmup: FrameSampleReport | null;
  scroll: FrameSampleReport | null;
  send: FrameSampleReport | null;
  /** 发送负载诊断面（异常与 append 前后消息数）。 */
  sendDiag: {
    sendError: string | null;
    messagesBefore: number;
    messagesAfter: number;
  } | null;
}

/** 报告回传 dev server（vite.config.ts 的 dev-only 中间件落盘）；失败静默——
 *  页面内徽标与 console 始终是第一现场。 */
async function reportToDevServer(payload: StressRunPayload): Promise<void> {
  try {
    await fetch("/__stress_report", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(payload, null, 2),
    });
  } catch (err) {
    console.warn("[stress] 报告回传失败（页面内结果仍有效）", err);
  }
}

/** harness 挂载体：runtime + 控制面板 + 真实 ChatThread。 */
export function StressHarness() {
  const adapter = useRef(createStressStreamAdapter(SEND_SCRIPT));
  const runtime = useLocalRuntime(adapter.current);
  // 合成数据集（挂载期生成一次，避免每次渲染重建）
  const [dataset] = useState(() => buildStressEntries(STRESS_MESSAGE_COUNT));
  const [summaries] = useState(() =>
    deriveRunSummaries(dataset.entries.flatMap((e) => (e.kind === "message" ? [e] : []))),
  );
  const [scrollReport, setScrollReport] = useState<FrameSampleReport | null>(null);
  const [sendReport, setSendReport] = useState<FrameSampleReport | null>(null);
  const [busy, setBusy] = useState<null | "scroll" | "send">(null);
  const imported = useRef(false);
  // 最新报告的 ref 镜像：自动序列收尾时读（state 异步，回调内不可靠）
  const scrollReportRef = useRef<FrameSampleReport | null>(null);
  const sendReportRef = useRef<FrameSampleReport | null>(null);
  // 预热往返（首屏冷渲染）的参考数据
  const warmupRef = useRef<FrameSampleReport | null>(null);
  // 发送负载诊断（异常与消息数变化，定位 append 未触发 run 的情况）
  const sendDiagRef = useRef<{
    sendError: string | null;
    messagesBefore: number;
    messagesAfter: number;
  } | null>(null);

  /** 滚动负载：预热 1 次（首屏冷渲染不计入——压测口径是稳态滚动）+
   *  首尾往返 10 次正式采样；预热数据单独留档供参考。 */
  const runScrollStress = useCallback(async () => {
    const viewport = document.querySelector<HTMLElement>(".cw-scroll-viewport");
    if (!viewport) {
      window.alert("未找到滚动容器（.cw-scroll-viewport）——渲染管线未就绪");
      return;
    }
    setBusy("scroll");
    // 关掉平滑滚动：压测目标是渲染帧间隔，程序化跳变贴近拖动滚动条
    viewport.style.scrollBehavior = "auto";
    // 预热往返：触发一次全高布局与滚动处理器路径，单独采样留档
    const warmup = new FrameSampler(100);
    warmup.start();
    viewport.scrollTop = viewport.scrollHeight;
    await settle();
    viewport.scrollTop = 0;
    await settle();
    await settle();
    warmupRef.current = warmup.stop();
    // 正式采样：10 次往返
    const sampler = new FrameSampler(100);
    sampler.start();
    for (let i = 0; i < SCROLL_ROUNDS; i++) {
      viewport.scrollTop = viewport.scrollHeight;
      await settle();
      viewport.scrollTop = 0;
      await settle();
    }
    // 收尾再等一帧：让最后一次跳变的绘制进入采样窗口
    await settle();
    const report = sampler.stop();
    scrollReportRef.current = report;
    setScrollReport(report);
    setBusy(null);
  }, []);

  /** 发送负载：ScriptedBackend 流式一轮，全程 rAF 采样；异常与消息数变化
   *  一并入报告（append 未触发 run 时采样只有零帧，靠诊断字段定位）。 */
  const runSendStress = useCallback(async () => {
    setBusy("send");
    const sampler = new FrameSampler(100);
    sampler.start();
    let sendError: string | null = null;
    let messagesBefore = -1;
    let messagesAfter = -1;
    try {
      messagesBefore = runtime.thread.getState().messages.length;
      await runtime.thread.append({
        content: [{ type: "text", text: "压测：请在 2000 条会话上流式回复" }],
      });
      // append 在「消息提交」时即 resolve，不等流式 run 结束——继续等
      // isRunning 回落（run 收口）才算采样完成；30 秒兜底防挂死
      const deadline = Date.now() + 30_000;
      while (runtime.thread.getState().isRunning && Date.now() < deadline) {
        await nextFrame();
      }
      messagesAfter = runtime.thread.getState().messages.length;
    } catch (err) {
      sendError = err instanceof Error ? `${err.name}: ${err.message}` : String(err);
      console.warn("[stress] 发送压测异常", err);
    }
    const report = sampler.stop() ?? {
      totalFrames: 0,
      longFrames: 0,
      maxGapMs: 0,
      avgGapMs: 0,
      thresholdMs: 100,
      durationMs: 0,
      pass: sendError === null,
      gaps: [],
    };
    if (sendError) report.pass = false;
    sendReportRef.current = report;
    sendDiagRef.current = { sendError, messagesBefore, messagesAfter };
    setSendReport(report);
    setBusy(null);
  }, [runtime]);

  /** 自动序列：等渲染稳定 → 滚动压测 → 发送压测 → 上报。 */
  const runAutoSequence = useCallback(async () => {
    // 等 import 渲染稳定（首帧长渲染不计入压测窗口——压测的是稳态滚动）
    await new Promise((r) => setTimeout(r, 2000));
    await runScrollStress();
    await new Promise((r) => setTimeout(r, 500));
    await runSendStress();
    const payload: StressRunPayload = {
      capturedAt: new Date().toISOString(),
      userAgent: navigator.userAgent,
      hardwareConcurrency: navigator.hardwareConcurrency ?? 0,
      messageCount: dataset.messageCount,
      warmup: warmupRef.current,
      scroll: scrollReportRef.current,
      send: sendReportRef.current,
      sendDiag: sendDiagRef.current,
    };
    console.warn("[stress] 压测报告", payload);
    await reportToDevServer(payload);
  }, [dataset.messageCount, runScrollStress, runSendStress]);

  // 挂载即收养：合成树 import + 压缩投影（与 SessionWorkspace 的 adoptTree
  // 同一条消费路径）；URL 带 auto=1 时自动跑完整序列（无人值守测量用）
  useEffect(() => {
    if (imported.current) return;
    imported.current = true;
    runtime.thread.import(entriesToExported(dataset.entries));
    useCompactionStore.getState().setCompactions(STRESS_SESSION_ID, dataset.compactions);
    if (new URLSearchParams(window.location.search).get("auto") === "1") {
      void runAutoSequence();
    }
  }, [runtime, dataset, runAutoSequence]);

  return (
    <AssistantRuntimeProvider runtime={runtime} config={stressAuiConfig}>
      {/* sessionId 走正式 session-scope 通道：CompactionDivider / ChatThread
          内部的 useSessionScope 与主界面同源（压缩投影按此键落桶） */}
      <SessionScopeContext.Provider value={STRESS_SESSION_ID}>
        <RunSummariesContext.Provider value={summaries}>
          <div className="fixed inset-x-0 top-0 z-50 border-b border-[var(--border)] bg-[var(--card)] px-4 py-2 text-[var(--foreground)] shadow">
            <div className="mx-auto flex max-w-6xl flex-wrap items-center gap-3">
              <span className="text-ui-sm font-medium">
                NFR-03 压测 harness（dev-only，{dataset.messageCount} 条合成消息）
              </span>
              <button
                type="button"
                data-testid="stress-scroll"
                disabled={busy !== null}
                onClick={() => void runScrollStress()}
                className="rounded-lg border border-[var(--border)] px-3 py-1 text-ui-sm hover:border-[var(--ring)] disabled:opacity-50"
              >
                {busy === "scroll" ? "滚动压测中…" : `滚动压测（往返 ${SCROLL_ROUNDS} 次）`}
              </button>
              <button
                type="button"
                data-testid="stress-send"
                disabled={busy !== null}
                onClick={() => void runSendStress()}
                className="rounded-lg border border-[var(--border)] px-3 py-1 text-ui-sm hover:border-[var(--ring)] disabled:opacity-50"
              >
                {busy === "send" ? "发送压测中…" : "发送压测（流式一轮）"}
              </button>
              <div className="ml-auto flex items-center gap-6">
                <ReportBadge label="scroll" report={scrollReport} />
                <ReportBadge label="send" report={sendReport} />
              </div>
            </div>
          </div>
          {/* 顶部给控制条让位 */}
          <div className="h-12" />
          <div className="relative flex h-[calc(100dvh-3rem)] min-h-0">
            <div className="flex h-full min-h-0 w-full flex-col text-[var(--foreground)]">
              <CompactionDivider />
              <ChatThread />
            </div>
          </div>
        </RunSummariesContext.Provider>
      </SessionScopeContext.Provider>
    </AssistantRuntimeProvider>
  );
}

/** 单项负载的报告徽标（通过线：>100ms 长帧 = 0）。 */
function ReportBadge({ label, report }: { label: string; report: FrameSampleReport | null }) {
  if (!report) {
    return (
      <span
        className="text-ui-sm text-[var(--muted-foreground)]"
        data-testid={`stress-report-${label}`}
      >
        {label}：未运行
      </span>
    );
  }
  return (
    <span
      className={`text-ui-sm font-medium ${report.pass ? "text-[var(--foreground)]" : "text-[var(--warning)]"}`}
      data-testid={`stress-report-${label}`}
    >
      {label}：{report.pass ? "通过" : "不通过"} · 帧数 {report.totalFrames} · 长帧{" "}
      {report.longFrames} · 最大间隔 {report.maxGapMs.toFixed(1)}ms
    </span>
  );
}
