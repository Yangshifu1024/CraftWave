/**
 * 右栏 Inspector（FR-13-01，17 号票）：M1 仅「用量」页（§10.7）——
 * 计数条三数的明细展开、缓存命中率与上下文进度条。
 *
 * 数据按活跃会话取（useSessionMetrics 分桶）；上下文窗口取当前激活模型
 * 的 contextWindow（get_config 一次拉取；无激活模型时按文档默认 128000）。
 * 头部带「run 结束自动折叠」开关（§10.4/FR-01-10 可配置，ui-state 持久化）。
 */

import { useEffect, useState } from "react";
import { useSessionMetrics } from "@/chat/run-metrics";
import { useSessionScope } from "@/chat/session-scope";
import { getConfig } from "@/ipc/client";
import type { ProviderConfig } from "@/ipc/types";
import { useSessionsStore } from "@/stores/sessions";

/** 技术设计文档 normalize 默认：contextWindow 缺省 128000。 */
const DEFAULT_CONTEXT_WINDOW = 128000;

/** 激活模型的上下文窗口（无激活模型 → 默认值；解析失败不阻塞渲染）。 */
function useContextWindow(): number {
  const [window, setWindow] = useState(DEFAULT_CONTEXT_WINDOW);
  useEffect(() => {
    let cancelled = false;
    void getConfig()
      .then((config) => {
        if (cancelled) return;
        const modelId = config.activeModelId;
        if (modelId === undefined) return;
        for (const provider of config.providers as ProviderConfig[]) {
          const hit = (provider.models ?? []).find((m) => m.id === modelId);
          if (hit?.contextWindow && hit.contextWindow > 0) {
            setWindow(hit.contextWindow);
            return;
          }
        }
      })
      .catch(() => {
        // 纯浏览器/IPC 异常：维持默认窗口值
      });
    return () => {
      cancelled = true;
    };
  }, []);
  return window;
}

export function Inspector() {
  const sessionId = useSessionScope();
  const { rate, turnUsage, sessionUsage, lastRun } = useSessionMetrics(sessionId);
  const contextWindow = useContextWindow();
  const collapseRuns = useSessionsStore((s) => s.collapseRuns);
  const setCollapseRuns = useSessionsStore((s) => s.setCollapseRuns);

  const total = (u: { input: number; output: number }) => u.input + u.output;
  const hitRate =
    sessionUsage.cacheRead > 0
      ? sessionUsage.cacheRead / (sessionUsage.input + sessionUsage.cacheRead)
      : null;
  // 上下文占用：会话累计（输入侧 + 缓存读，§10.5 口径折算）
  const contextUsed = sessionUsage.input + sessionUsage.cacheRead;
  const contextRatio = Math.min(1, contextUsed / contextWindow);

  return (
    <aside
      data-testid="inspector"
      className="flex w-72 shrink-0 flex-col border-l border-[var(--border)] bg-[var(--card)]"
    >
      <div className="flex items-center gap-2 border-b border-[var(--border)] px-3 py-2 text-xs text-[var(--foreground)]">
        <span className="font-medium">用量</span>
        <label className="ml-auto flex cursor-default items-center gap-1 text-[11px] text-[var(--muted-foreground)]">
          <input
            type="checkbox"
            checked={collapseRuns}
            onChange={(e) => setCollapseRuns(e.target.checked)}
          />
          自动折叠
        </label>
      </div>
      <div className="flex flex-col gap-3 p-3 text-xs text-[var(--muted-foreground)]">
        <section>
          <h3 className="mb-1 text-[11px] text-[var(--cw-dim)]">实时</h3>
          <div className="flex justify-between">
            <span>输出速率</span>
            <span data-testid="inspector-rate">
              {rate === null ? "—" : `${rate.toFixed(0)} tok/s`}
            </span>
          </div>
        </section>
        <section>
          <h3 className="mb-1 text-[11px] text-[var(--cw-dim)]">本轮用量</h3>
          <MetricRow label="输入" value={turnUsage.input} />
          <MetricRow label="输出" value={turnUsage.output} />
          <MetricRow label="缓存读" value={turnUsage.cacheRead} />
          <MetricRow label="缓存写" value={turnUsage.cacheWrite} />
          <div className="mt-1 flex justify-between text-[var(--foreground)]">
            <span>合计</span>
            <span data-testid="inspector-turn-total">{total(turnUsage)} tok</span>
          </div>
        </section>
        <section>
          <h3 className="mb-1 text-[11px] text-[var(--cw-dim)]">会话累计</h3>
          <MetricRow label="输入" value={sessionUsage.input} />
          <MetricRow label="输出" value={sessionUsage.output} />
          <div className="flex justify-between">
            <span>缓存命中率</span>
            <span data-testid="inspector-cache">
              {hitRate === null ? "—" : `${(hitRate * 100).toFixed(0)}%`}
            </span>
          </div>
          <div className="mt-1 flex justify-between text-[var(--foreground)]">
            <span>合计</span>
            <span data-testid="inspector-session-total">{total(sessionUsage)} tok</span>
          </div>
        </section>
        <section>
          <h3 className="mb-1 text-[11px] text-[var(--cw-dim)]">上下文进度</h3>
          <div
            className="relative h-2 w-full overflow-hidden rounded bg-[var(--muted)]"
            data-testid="inspector-context"
          >
            <div
              className="h-full bg-[var(--primary)]"
              style={{ width: `${contextRatio * 100}%` }}
            />
            {/* §10.5：0.6 阈值标线（超过即接近压缩线） */}
            <div
              className="absolute top-0 h-full w-px bg-[var(--destructive)]"
              style={{ left: "60%" }}
            />
          </div>
          <div className="mt-1 flex justify-between">
            <span>{contextUsed} tok</span>
            <span>{contextWindow} 窗口</span>
          </div>
        </section>
        {lastRun && (
          <section>
            <h3 className="mb-1 text-[11px] text-[var(--cw-dim)]">最近运行</h3>
            <div className="flex justify-between">
              <span>终态</span>
              <span data-testid="inspector-last-run">
                {lastRun.reason === "done"
                  ? `完成 · ${(lastRun.durationMs / 1000).toFixed(1)}s`
                  : lastRun.reason === "cancelled"
                    ? "已取消"
                    : "出错"}
                {lastRun.reason === "done" ? ` · ${lastRun.toolCount} 次工具` : ""}
              </span>
            </div>
          </section>
        )}
      </div>
    </aside>
  );
}

function MetricRow({ label, value }: { label: string; value: number }) {
  return (
    <div className="flex justify-between">
      <span>{label}</span>
      <span>{value} tok</span>
    </div>
  );
}
