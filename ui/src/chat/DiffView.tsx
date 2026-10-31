/**
 * diff 渲染公共组件（26 号票，FR-11-02）：从 AskPanel 的审批 diff 预览抽出
 * ——审批卡（AskPanel）与变更面板（ChangesPanel）共用同一套「文件名标题 +
 * 按 hunk 分组的增删行列表」渲染，防两处样式漂移。
 *
 * 形态 = §3.2 AskPayload.approval.diff 的 { path, hunks: [{header,
 * lines: [{kind, text}]}] }（§3.3 #38 DiffPatch 同构复用 hunks 子结构）。
 */

import { useT } from "@/i18n/core";
import type { DiffLine } from "@/ipc/types";

/** diff 行配色：新增绿底、删除红底、上下文行弱化。 */
const diffLineClass: Record<DiffLine["kind"], string> = {
  add: "bg-emerald-950/60 text-emerald-300",
  del: "bg-red-950/60 text-red-300",
  ctx: "text-[var(--muted-foreground)]",
};

/** 单个文件 diff：文件名标题（可附统计/标记行）+ hunk 行列表。 */
export function DiffView({
  diff,
  header,
  maxH = "max-h-64",
  testId = "diff-view",
}: {
  diff: { path: string; hunks: { header: string; lines: DiffLine[] }[] };
  /** 标题右侧的统计/标记文本（如 +12 −3、二进制）；不传不显示。 */
  header?: string;
  /** 行区最大高度（审批卡小窗用默认，变更面板传更大值）。 */
  maxH?: string;
  testId?: string;
}) {
  const t = useT();
  return (
    <div data-testid={testId} className="rounded border border-[var(--border)]">
      <p className="flex items-center justify-between border-b border-[var(--border)] px-2 py-1 font-mono text-ui-sm text-[var(--foreground)]">
        <span className="min-w-0 truncate">{diff.path}</span>
        {header !== undefined && header !== "" && (
          <span className="ml-2 shrink-0 text-[var(--muted-foreground)]">{header}</span>
        )}
      </p>
      <div className={`${maxH} overflow-auto`}>
        {diff.hunks.length === 0 ? (
          <p className="px-2 py-1 text-ui-sm text-[var(--muted-foreground)]">
            {t("inspector.noTextDiff")}
          </p>
        ) : (
          diff.hunks.map((hunk) => (
            <div key={hunk.header}>
              <p className="px-2 py-0.5 font-mono text-ui-sm text-[var(--muted-foreground)]">
                {hunk.header}
              </p>
              {hunk.lines.map((line, index) => (
                <pre
                  // biome-ignore lint/suspicious/noArrayIndexKey: diff 行无稳定 id（同 hunk 可有相同文本的行），序号防 key 撞（ToolCards 同款先例）
                  key={`${line.kind}:${index}`}
                  className={`px-2 font-mono text-ui-sm ${diffLineClass[line.kind]}`}
                >
                  {line.kind === "add" ? "+" : line.kind === "del" ? "−" : " "} {line.text}
                </pre>
              ))}
            </div>
          ))
        )}
      </div>
    </div>
  );
}
