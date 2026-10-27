/**
 * 【S1 spike 原型，随时可弃】镜像工具卡片。
 *
 * 状态机（票面判定点）：
 *   参数准备中（argsText 仍在流式，半截 JSON）→ 运行中（参数齐、结果未到）→ 完成 / 错误（result 到达）。
 * 数据来源全部官方原语：part props（result/isError）+ useToolArgsStatus（status/argsStatus）。
 *
 * 挂载埋点：window.__mirrorMountLog 记录每个 toolCallId 的 mount/render 计数，
 * 供测试断言「不闪烁不重建」（mount 恰好 1 次 = 不重挂载）。
 */
import { type ToolCallMessagePartProps, useToolArgsStatus } from "@assistant-ui/react";
import { useEffect, useRef } from "react";
import { isArgsTextComplete } from "./frames";

export type MountLogEntry = {
  toolCallId: string;
  kind: "mount" | "unmount" | "render";
};

/** 全局挂载日志（测试断言用；window 挂载，便于跨组件聚合）。 */
declare global {
  interface Window {
    __mirrorMountLog?: MountLogEntry[];
  }
}

export function mountLog(): MountLogEntry[] {
  return window.__mirrorMountLog ?? [];
}

export function resetMountLog(): void {
  window.__mirrorMountLog = [];
}

/** 挂载/渲染埋点 hook：mount/unmount 各记一次，render 全记。 */
function useMountTrace(toolCallId: string): number {
  const renderCount = useRef(0);
  renderCount.current += 1;
  useEffect(() => {
    mountLog().push({ toolCallId, kind: "mount" });
    return () => {
      mountLog().push({ toolCallId, kind: "unmount" });
    };
  }, [toolCallId]);
  return renderCount.current;
}

/** 卡片状态机的单一判定函数（导出供测试直接断言）。 */
export function cardPhase(
  statusType: "running" | "complete" | "incomplete" | "requires-action",
  hasResult: boolean,
  argsComplete: boolean,
): "preparing" | "running" | "done" | "error" {
  if (hasResult) return "done"; // isError 的分支交给展示层
  if (statusType === "running" && !argsComplete) return "preparing";
  return "running";
}

type CardProps = ToolCallMessagePartProps<Record<string, unknown>, unknown> & {
  title: string;
  /** 完成态对 result 的展示（原型用最简 JSON 文本）。 */
  resultLabel?: (result: unknown) => string;
};

export function MirrorToolCard(props: CardProps) {
  const { title, result, isError, argsText, toolCallId } = props;
  // 0.15.31 实测：useToolArgsStatus().status 是字面量联合（非对象），argsStatus 靠
  // part.args 里的 partial-parse 元数据区分 streaming/complete。
  const { status, argsStatus } = useToolArgsStatus();
  const renderCount = useMountTrace(toolCallId);
  const argsDone = argsStatus === "complete" && isArgsTextComplete(argsText);
  const phase =
    result !== undefined && isError ? "error" : cardPhase(status, result !== undefined, argsDone);

  const phaseLabel =
    phase === "preparing"
      ? "参数准备中"
      : phase === "running"
        ? "运行中"
        : phase === "error"
          ? "错误"
          : "完成";

  return (
    <div
      data-testid={`tool-card-${toolCallId}`}
      data-phase={phase}
      className="my-1 rounded-lg border border-neutral-700 bg-neutral-900 px-3 py-2 text-xs"
    >
      <div className="flex items-center justify-between gap-2">
        <span className="font-medium text-neutral-200">
          {title} <span className="text-neutral-500">#{toolCallId}</span>
        </span>
        <span
          className={
            phase === "error"
              ? "rounded bg-red-900/60 px-1.5 py-0.5 text-red-200"
              : phase === "done"
                ? "rounded bg-emerald-900/60 px-1.5 py-0.5 text-emerald-200"
                : "rounded bg-sky-900/60 px-1.5 py-0.5 text-sky-200"
          }
          data-phase-label={phaseLabel}
        >
          {phaseLabel}
        </span>
      </div>
      {/* 参数准备中：半截 JSON 原文照显（票面要求：半截参数增量可见） */}
      <pre className="mt-1 max-h-24 overflow-hidden whitespace-pre-wrap break-all text-neutral-400">
        {argsText || "（等待参数…）"}
      </pre>
      {result !== undefined && (
        <pre
          className={`mt-1 whitespace-pre-wrap break-all ${isError ? "text-red-300" : "text-neutral-300"}`}
        >
          {typeof result === "string" ? result : JSON.stringify(result)}
        </pre>
      )}
      <span className="hidden" data-render-count={String(renderCount)} />
    </div>
  );
}

/** 两个镜像工具的具名包装（toolkit 里引用）。 */
export function ReadWorkspaceFileCard(
  props: ToolCallMessagePartProps<Record<string, unknown>, unknown>,
) {
  return <MirrorToolCard {...props} title="读取文件 read_workspace_file" />;
}

export function RunCommandCard(props: ToolCallMessagePartProps<Record<string, unknown>, unknown>) {
  return <MirrorToolCard {...props} title="执行命令 run_command" />;
}
