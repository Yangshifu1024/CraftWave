/**
 * 上下文压缩 UI（FR-03-05，04 号票）：两块轻量呈现。
 *
 * - CompactionIndicator：Composer 上方的压缩进行指示条（compacting=
 *   "{t("inspector.compacting")}"，spinner 扫光）；失败反馈已改走顶部 Toast（29 号票
 *   收敛，04 号票挂账清偿），本组件不再有常驻失败行；
 * - CompactionDivider：消息列表顶部的"已压缩早期上下文"分隔提示
 *   （§6.3 定稿：compaction 条目不进消息树，按投影面渲染，非消息体）。
 */

import { LoaderCircle } from "lucide-react";
import { useSessionCompaction } from "@/chat/compaction-store";
import { useRunMetricsStore } from "@/chat/run-metrics";
import { useSessionScope } from "@/chat/session-scope";
import { useT } from "@/i18n/core";
import { compactSession } from "@/ipc/client";

/** Composer 上方的压缩生命周期指示条（仅 compacting 态占一行）。 */
export function CompactionIndicator() {
  const t = useT();
  const sessionId = useSessionScope();
  const { phase } = useSessionCompaction(sessionId);
  if (phase !== "compacting") return null;
  return (
    <div
      data-testid="compaction-indicator"
      className="cw-pop-in mb-2 flex items-center gap-2 rounded-lg border border-[var(--border)] bg-[var(--card)] px-3 py-1.5 text-ui-sm text-[var(--muted-foreground)]"
      role="status"
    >
      <LoaderCircle size={12} aria-hidden className="shrink-0 animate-spin" />
      {t("inspector.compacting")}
    </div>
  );
}

/** 消息列表顶部的压缩边界分隔提示（非消息体，§6.3）。 */
export function CompactionDivider() {
  const t = useT();
  const sessionId = useSessionScope();
  const { compactions } = useSessionCompaction(sessionId);
  if (compactions.length === 0) return null;
  return (
    <div
      data-testid="compaction-divider"
      className="flex items-center gap-3 py-1 text-ui-sm text-[var(--muted-foreground)]"
    >
      <span className="h-px flex-1 bg-[var(--border)]" aria-hidden />
      <span>{t("inspector.compactedDivider", { count: compactions.length })}</span>
      <span className="h-px flex-1 bg-[var(--border)]" aria-hidden />
    </div>
  );
}

/** 上下文进度条旁的"压缩"按钮（Inspector §10.5 落点）：运行中禁用
 * （后端 E_SESSION_RUNNING 兜底）；压缩进行中同样禁用防重复提交。 */
export function CompactButton() {
  const t = useT();
  const sessionId = useSessionScope();
  const phase = useSessionCompaction(sessionId).phase;
  const running = useRunMetricsStore((s) => s.bySession[sessionId]?.currentRunId) != null;
  const disabled = phase === "compacting" || running;
  return (
    <button
      type="button"
      data-testid="compact-button"
      disabled={disabled}
      className="ml-2 shrink-0 rounded border border-[var(--border)] px-2 py-0.5 text-ui-xs text-[var(--muted-foreground)] transition-colors hover:text-[var(--foreground)] disabled:cursor-not-allowed disabled:opacity-50"
      onClick={() => {
        void compactSession(sessionId).catch(() => {
          // 失败经 run:compact_failed 事件呈现（命令 reject 静默兜底）
        });
      }}
    >
      {t("inspector.compactButton")}
    </button>
  );
}
