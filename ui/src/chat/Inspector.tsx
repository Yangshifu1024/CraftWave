/**
 * 右栏 Inspector（FR-13-01，17 号票）：M1 仅「用量」页（§10.7）——
 * 计数条三数的明细展开、缓存命中率与上下文进度条。
 *
 * 数据按活跃会话取（useSessionMetrics 分桶）；上下文窗口取当前激活模型
 * 的 contextWindow（get_config 一次拉取；无激活模型时按文档默认 128000）。
 * 「run 结束自动折叠」（§10.4/FR-01-10）行为保留，UI 开关已移除——按
 * ui-state 持久化值（缺省 true）执行。
 */

import { useEffect, useState } from "react";
import { CompactButton } from "@/chat/CompactionUI";
import { type PlanItem, usePlanStateStore } from "@/chat/plan-state";
import { useSessionMetrics } from "@/chat/run-metrics";
import { useSessionScope } from "@/chat/session-scope";
import { getConfig } from "@/ipc/client";
import type { ProviderConfig } from "@/ipc/types";

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

/** 空计划快照（模块级常量：selector 缺桶时返回稳定引用，防 React
 *  「getSnapshot should be cached」无限渲染循环——每次 `?? []` 都会新建
 *  数组引用，zustand 的 Object.is 比较恒不等）。 */
const EMPTY_PLAN: PlanItem[] = [];

/** 右栏用量面板本体：实时速率 / 本轮用量 / 会话累计 / 上下文进度条
 *  + 最近运行终态。宽度受控
 *  （拖宽手柄，min=初始宽 max=半屏）；开合动画走外层宽度过渡，内层定宽
 *  防内容回流；收起后 inert 挡键盘焦点（面板常驻挂载）。 */
export function Inspector({
  width,
  open,
  animate,
}: {
  width: number;
  open: boolean;
  animate: boolean;
}) {
  const sessionId = useSessionScope();
  const { rate, turnUsage, sessionUsage, lastRun } = useSessionMetrics(sessionId);
  const contextWindow = useContextWindow();
  const planItems = usePlanStateStore((s) => s.bySession[sessionId] ?? EMPTY_PLAN);

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
      className={`shrink-0 overflow-hidden ${
        animate ? "transition-[width] duration-200 ease-out" : ""
      }`}
      style={{ width: open ? width : 0 }}
      inert={!open}
    >
      {/* pt-2 + h-12 头行与会话卡头完全同基线；与窗口背景一体化（无底色无边框） */}
      <div className="flex h-full min-h-0 flex-col pt-2" style={{ width }}>
        <div className="flex h-12 shrink-0 items-center gap-2 px-3 text-ui-sm text-[var(--foreground)]">
          <span className="font-medium">用量</span>
        </div>
        <div className="flex flex-col gap-3 p-3 text-ui-sm tabular-nums text-[var(--muted-foreground)]">
          {/* 计划（B 组）：plan:update 驱动的当前计划（内存态，重启清空） */}
          {planItems.length > 0 && (
            <section>
              <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">
                计划（{planItems.filter((i) => i.status === "completed").length}/{planItems.length}
                ）
              </h3>
              <ul className="space-y-0.5" data-testid="inspector-plan">
                {planItems.map((item) => (
                  <li key={item.id} className="flex items-start gap-1.5">
                    <span
                      className={
                        item.status === "completed"
                          ? "text-emerald-600 dark:text-emerald-400"
                          : "text-[var(--muted-foreground)]"
                      }
                    >
                      {item.status === "completed"
                        ? "●"
                        : item.status === "in_progress"
                          ? "◐"
                          : "○"}
                    </span>
                    <span className={item.status === "completed" ? "line-through" : ""}>
                      {item.text}
                    </span>
                  </li>
                ))}
              </ul>
            </section>
          )}
          <section>
            <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">实时</h3>
            <div className="flex justify-between">
              <span>输出速率</span>
              <span data-testid="inspector-rate">
                {rate === null ? "—" : `${rate.toFixed(0)} tok/s`}
              </span>
            </div>
          </section>
          <section>
            <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">本轮用量</h3>
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
            <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">会话累计</h3>
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
            <h3 className="mb-1 flex items-center text-ui-sm text-[var(--cw-dim)]">
              上下文进度
              {/* §10.5 落点：进度条旁的手动压缩（FR-03-05，04 号票）；
                  运行中/压缩中禁用（后端 E_SESSION_RUNNING 兜底） */}
              <CompactButton />
            </h3>
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
              <h3 className="mb-1 text-ui-sm text-[var(--cw-dim)]">最近运行</h3>
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
      </div>
    </aside>
  );
}

/** 单行指标（标签 + 数值 + tok 单位）。 */
function MetricRow({ label, value }: { label: string; value: number }) {
  return (
    <div className="flex justify-between">
      <span>{label}</span>
      <span>{value} tok</span>
    </div>
  );
}
