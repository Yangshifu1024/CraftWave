/**
 * 正式工具卡片（§10.3 规格，14 号票）：toolkit 条目的 render 组件。
 *
 * 数据源：part props（args/result/isError）+ useToolArgsStatus（S1 钉死）。
 * 状态机：参数准备中（半截 JSON）→ 运行中 → 完成/错误（result 直写驱动）。
 * M1 结果面 = 帧 uiSummary（一行人话）——详细 data 恢复随 toolres 边车票
 * （load_tool_outcomes #29），故 read 的预览展开 / command 的输出折叠以
 * 票面边界记录不实装，卡片以「参数 + 摘要」承载。
 */

import { type ToolCallMessagePartProps, useToolArgsStatus } from "@assistant-ui/react";

/** 卡片通用状态判定（与 mirror 原型同口径，供测试直接断言）。 */
export function cardPhase(
  statusType: "running" | "complete" | "incomplete" | "requires-action",
  hasResult: boolean,
  argsComplete: boolean,
): "preparing" | "running" | "done" | "error" {
  if (hasResult && statusType === "running") return "done";
  if (hasResult) return "done";
  if (statusType === "running" && !argsComplete) return "preparing";
  return "running";
}

type CardArgs = Record<string, unknown>;

interface CardProps extends ToolCallMessagePartProps<CardArgs, unknown> {
  title: string;
}

/** 通用壳：标题 + 状态徽标 + result 摘要行。 */
function ToolCardShell({ title, children, ...props }: CardProps & { children?: React.ReactNode }) {
  const { toolCallId, result, isError, argsText } = props;
  const { status, argsStatus } = useToolArgsStatus();
  const argsDone = argsStatus === "complete";
  const hasResult = result !== undefined;
  const phase = isError ? "error" : cardPhase(status, hasResult, argsDone);
  const phaseLabel = {
    preparing: "参数准备中",
    running: "运行中",
    error: "错误",
    done: "完成",
  }[phase];

  const summary =
    result !== undefined && typeof result === "object" && result !== null
      ? String((result as { summary?: string }).summary ?? "")
      : "";

  return (
    <div
      data-testid={`tool-card-${toolCallId}`}
      data-phase={phase}
      className="my-1 rounded-lg border border-neutral-700 bg-neutral-900 px-3 py-2 text-xs"
    >
      <div className="flex items-center justify-between gap-2">
        <span className="font-medium text-neutral-200">{title}</span>
        <span
          data-phase-label={phaseLabel}
          className={
            phase === "error"
              ? "rounded bg-red-900/60 px-1.5 py-0.5 text-red-200"
              : phase === "done"
                ? "rounded bg-emerald-900/60 px-1.5 py-0.5 text-emerald-200"
                : "rounded bg-sky-900/60 px-1.5 py-0.5 text-sky-200"
          }
        >
          {phaseLabel}
        </span>
      </div>
      <div className="mt-1 space-y-1">{children}</div>
      {phase === "preparing" && (
        <pre className="max-h-24 overflow-hidden whitespace-pre-wrap break-all text-neutral-400">
          {argsText || "（等待参数…）"}
        </pre>
      )}
      {hasResult && <p className={isError ? "text-red-300" : "text-neutral-400"}>{summary}</p>}
    </div>
  );
}

const str = (args: CardArgs, key: string): string =>
  typeof args[key] === "string" ? (args[key] as string) : "";
const num = (args: CardArgs, key: string): number | undefined =>
  typeof args[key] === "number" ? (args[key] as number) : undefined;
const lineCount = (text: string): number => (text.length > 0 ? text.split("\n").length : 0);

/** read：路径 + 行区间。 */
export function ReadCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const offset = num(args, "offset");
  const limit = num(args, "limit");
  const range =
    offset !== undefined || limit !== undefined
      ? `（${offset ?? 1} 起${limit !== undefined ? ` · ${limit} 行` : ""}）`
      : "";
  return (
    <ToolCardShell {...props} title="读取文件">
      <p className="font-mono text-neutral-300">
        {str(args, "path")}
        <span className="text-neutral-500">{range}</span>
      </p>
    </ToolCardShell>
  );
}

/** write：新文件 + +N 徽标（content 行数）。 */
export function WriteCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const added = lineCount(str(args, "content"));
  return (
    <ToolCardShell {...props} title="写入新文件">
      <p className="font-mono text-neutral-300">
        {str(args, "path")}{" "}
        <span className="rounded bg-emerald-900/60 px-1 text-emerald-200">+{added}</span>
      </p>
    </ToolCardShell>
  );
}

/** edit：路径 + +N/−N 徽标（newText/oldText 行数）。 */
export function EditCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const added = lineCount(str(args, "newText"));
  const removed = lineCount(str(args, "oldText"));
  return (
    <ToolCardShell {...props} title="编辑文件">
      <p className="font-mono text-neutral-300">
        {str(args, "path")}{" "}
        <span className="rounded bg-emerald-900/60 px-1 text-emerald-200">+{added}</span>{" "}
        <span className="rounded bg-red-900/60 px-1 text-red-200">−{removed}</span>
      </p>
    </ToolCardShell>
  );
}

/** list_files / grep：目标 + 计数摘要（命中数在 result.summary）。 */
export function ListFilesCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  return (
    <ToolCardShell {...props} title="列目录">
      <p className="font-mono text-neutral-300">{str(args, "path") || "（项目根）"}</p>
    </ToolCardShell>
  );
}

export function GrepCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  return (
    <ToolCardShell {...props} title="搜索">
      <p className="font-mono text-neutral-300">
        <span className="text-neutral-500">/{str(args, "pattern")}/</span> {str(args, "glob") || ""}
        {str(args, "path") ? ` @ ${str(args, "path")}` : ""}
      </p>
    </ToolCardShell>
  );
}

/** command：命令行 monospace（输出折叠/退出码徽标随 toolres 边车票）。 */
export function CommandCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  return (
    <ToolCardShell {...props} title="执行命令">
      <pre className="overflow-x-auto whitespace-pre-wrap break-all rounded bg-neutral-950 px-2 py-1 font-mono text-neutral-300">
        {str(args, "command") || "（等待参数…）"}
      </pre>
    </ToolCardShell>
  );
}

/** plan：计划卡（FR-06-01 消息内呈现 + 批准按钮冻结快照 §8.6）。 */
export function PlanCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  const items = Array.isArray(args.items) ? (args.items as PlanItemArg[]) : [];
  const statusLabel: Record<string, string> = {
    pending: "待办",
    in_progress: "进行中",
    completed: "已完成",
  };
  return (
    <ToolCardShell {...props} title="任务计划">
      <ul className="space-y-0.5">
        {items.map((item) => (
          <li key={item.id} className="flex items-start gap-1.5">
            <span
              className={
                item.status === "completed"
                  ? "text-emerald-400"
                  : item.status === "in_progress"
                    ? "text-sky-400"
                    : "text-neutral-500"
              }
            >
              {item.status === "completed" ? "●" : item.status === "in_progress" ? "◐" : "○"}
            </span>
            <span className="text-neutral-300">
              {item.text}
              <span className="ml-1 text-neutral-500">
                {statusLabel[item.status] ?? item.status}
              </span>
            </span>
          </li>
        ))}
      </ul>
    </ToolCardShell>
  );
}

interface PlanItemArg {
  id: string;
  text: string;
  status: string;
}

/** ask：极简状态卡（问题本体在 AskPanel 呈现，§10.3 的 M1 落地形态）。 */
export function AskCard(props: ToolCallMessagePartProps<CardArgs, unknown>) {
  const args = (props.args ?? {}) as CardArgs;
  return (
    <ToolCardShell {...props} title="向用户询问">
      <p className="text-neutral-300">{str(args, "title")}</p>
      <p className="text-neutral-500">回答在下方询问面板中进行</p>
    </ToolCardShell>
  );
}
