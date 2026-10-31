/**
 * 产物面板：Inspector「产物」tab——本会话 AI
 * 写过的文件自动登记清单（`<sid>.artifacts.json`，后端 list_artifacts
 * 读面：去重计数 + exists 存在探测 + 存在优先/最近写入倒序）。
 *
 * 行 = 文件名 + 相对路径 + `×runCount` 徽标；exists=false 灰显置底（后端
 * 排序已保证，前端只做渲染）。行操作三个：
 * - 预览：read_workspace_file 只读 Dialog（truncated 提示；二进制/越界等
 *   错误行内展示，不弹窗打断）；
 * - 打开：open_in_editor（探测不到可用编辑器 reject E_NOT_FOUND → 行内提示）；
 * - 定位变更：openChangesPanel(sessionId, path)（changes-store
 *   约定接口——产物页与变更面板的联动接缝）。
 *
 * 刷新时机 = 页签激活（组件挂载/会话切换）+ run 终态（run:done/error/
 * cancelled 事件按会话过滤）。空态给引导文案。
 */

import { FileText, FolderOpen, GitCompareArrows } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { openChangesPanel } from "@/chat/changes-store";
import { Dialog } from "@/components/ui/dialog";
import { useT } from "@/i18n/core";
import { listArtifacts, listenEvent, openInEditor, readWorkspaceFile } from "@/ipc/client";
import type { ArtifactRow } from "@/ipc/types";

/** 行内轻提示（打开/预览失败时展示在行下方，自动消退不留死状态）。 */
interface LineNote {
  path: string;
  text: string;
}

/** 文件名（路径末段；正反斜杠都切）。 */
function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

/** 展示路径：文件名去重后的次要行（超长截中——面板窄，尾段比头段重要）。 */
function displayPath(path: string): string {
  if (path.length <= 64) return path;
  return `${path.slice(0, 28)}…${path.slice(-32)}`;
}

/** 产物面板主体：清单 + 行操作 + 预览 Dialog。 */
export function ArtifactsPanel({ sessionId }: { sessionId: string }) {
  const t = useT();
  const [rows, setRows] = useState<ArtifactRow[] | null>(null);
  const [note, setNote] = useState<LineNote | null>(null);
  const [preview, setPreview] = useState<{
    path: string;
    content: string;
    truncated: boolean;
  } | null>(null);

  const reload = useCallback(() => {
    let cancelled = false;
    void listArtifacts(sessionId)
      .then((result) => {
        if (!cancelled) setRows(result.artifacts);
      })
      .catch(() => {
        // IPC 异常（纯浏览器等）：空态兜底，不打断面板
        if (!cancelled) setRows([]);
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId]);

  // 刷新时机：页签激活（挂载/会话切换）+ run 终态三事件
  useEffect(() => {
    const cancel = reload();
    const unlistens = [
      listenRunEnd("run:done", sessionId, reload),
      listenRunEnd("run:error", sessionId, reload),
      listenRunEnd("run:cancelled", sessionId, reload),
    ];
    return () => {
      cancel();
      for (const un of unlistens) void un.then((fn) => fn()).catch(() => {});
    };
  }, [reload, sessionId]);

  // 行内轻提示自动消退（4s）
  useEffect(() => {
    if (note === null) return;
    const timer = setTimeout(() => setNote(null), 4000);
    return () => clearTimeout(timer);
  }, [note]);

  /** 预览（read_workspace_file）：错误行内展示。 */
  const openPreview = useCallback(
    (path: string) => {
      void readWorkspaceFile(sessionId, path)
        .then((result) =>
          setPreview({ path, content: result.content, truncated: result.truncated }),
        )
        .catch((err: unknown) => {
          const message =
            typeof err === "object" && err !== null && "message" in err
              ? String((err as { message: unknown }).message)
              : String(err);
          setNote({ path, text: t("inspector.artifactsPreviewFailed", { error: message }) });
        });
    },
    [sessionId, t],
  );

  /** 打开编辑器（#36）：探测不到编辑器等失败给行内提示。 */
  const openEditor = useCallback(
    (path: string) => {
      void openInEditor(path)
        .then(() => setNote(null))
        .catch((err: unknown) => {
          const message =
            typeof err === "object" && err !== null && "message" in err
              ? String((err as { message: unknown }).message)
              : String(err);
          setNote({ path, text: t("inspector.artifactsOpenFailed", { error: message }) });
        });
    },
    [t],
  );

  if (rows === null || rows.length === 0) {
    return (
      <div className="p-3 text-ui-sm text-[var(--muted-foreground)]" data-testid="artifacts-empty">
        {rows === null ? t("inspector.artifactsLoading") : t("inspector.artifactsEmpty")}
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto p-3 text-ui-sm text-[var(--muted-foreground)]">
      <ul className="space-y-1" data-testid="artifacts-list">
        {rows.map((row) => {
          const missing = !row.exists;
          return (
            <li key={row.path} data-testid={`artifact-row-${row.path}`}>
              <div
                className={`flex flex-col gap-0.5 rounded-lg border px-2 py-1.5 ${
                  missing
                    ? "border-transparent opacity-50"
                    : "border-transparent hover:border-[var(--border)]"
                }`}
              >
                <span className="flex min-w-0 items-center gap-1.5">
                  <FileText
                    size={12}
                    aria-hidden
                    className={`shrink-0 ${missing ? "text-[var(--muted-foreground)]" : "text-[var(--primary)]"}`}
                  />
                  <span
                    className={`min-w-0 truncate font-medium ${
                      missing ? "line-through" : "text-[var(--foreground)]"
                    }`}
                  >
                    {baseName(row.path)}
                  </span>
                  <span
                    className="ml-auto shrink-0 rounded bg-[var(--muted)] px-1.5 py-0.5 text-[10px] tabular-nums"
                    data-testid="artifact-run-count"
                  >
                    ×{row.runCount}
                  </span>
                </span>
                <span className="truncate font-mono text-[10px]" title={row.path}>
                  {displayPath(row.path)}
                </span>
                {missing && (
                  <span className="text-[10px]" data-testid="artifact-missing">
                    {t("inspector.artifactsDeleted")}
                  </span>
                )}
                {!missing && (
                  <span className="flex items-center gap-1.5 pt-0.5">
                    <button
                      type="button"
                      data-testid={`artifact-preview-${baseName(row.path)}`}
                      className="rounded border border-[var(--border)] px-1.5 py-0.5 text-[10px] text-[var(--foreground)] hover:border-[var(--primary)]"
                      onClick={() => openPreview(row.path)}
                    >
                      {t("inspector.artifactsPreview")}
                    </button>
                    <button
                      type="button"
                      data-testid={`artifact-open-${baseName(row.path)}`}
                      className="flex items-center gap-0.5 rounded border border-[var(--border)] px-1.5 py-0.5 text-[10px] text-[var(--foreground)] hover:border-[var(--primary)]"
                      onClick={() => openEditor(row.path)}
                    >
                      <FolderOpen size={10} aria-hidden />
                      {t("inspector.artifactsOpen")}
                    </button>
                    {/* 变更面板联动接缝（openChangesPanel 约定接口） */}
                    <button
                      type="button"
                      data-testid={`artifact-locate-${baseName(row.path)}`}
                      className="flex items-center gap-0.5 rounded border border-[var(--border)] px-1.5 py-0.5 text-[10px] text-[var(--foreground)] hover:border-[var(--primary)]"
                      onClick={() => openChangesPanel(sessionId, row.path)}
                    >
                      <GitCompareArrows size={10} aria-hidden />
                      {t("inspector.artifactsChanges")}
                    </button>
                  </span>
                )}
                {note?.path === row.path && (
                  <span className="text-[10px] text-red-600 dark:text-red-400" role="alert">
                    {note.text}
                  </span>
                )}
              </div>
            </li>
          );
        })}
      </ul>
      {preview !== null && (
        <ArtifactPreviewDialog
          path={preview.path}
          content={preview.content}
          truncated={preview.truncated}
          onClose={() => setPreview(null)}
        />
      )}
    </div>
  );
}

/** run 终态事件订阅（run:done/run:error/run:cancelled）按会话过滤。 */
function listenRunEnd(
  key: "run:done" | "run:error" | "run:cancelled",
  sessionId: string,
  onEnd: () => void,
): Promise<() => void> {
  return listenEvent<{ sessionId?: string }>(key, (payload) => {
    // 载荷带 sessionId（chat.rs 终态事件口径）；只刷新本会话
    if (payload.sessionId === sessionId) onEnd();
  });
}

/** 只读预览 Dialog：全文渲染（≤512KB 由后端截断）+ truncated 提示行。 */
function ArtifactPreviewDialog({
  path,
  content,
  truncated,
  onClose,
}: {
  path: string;
  content: string;
  truncated: boolean;
  onClose: () => void;
}) {
  const t = useT();
  return (
    <Dialog
      open
      title={baseName(path)}
      onClose={onClose}
      width={720}
      actions={[{ label: t("inspector.close"), variant: "ghost", onClick: onClose }]}
    >
      {truncated && (
        <p
          className="mb-2 rounded bg-[var(--muted)] px-2 py-1 text-ui-sm text-[var(--cw-dim)]"
          data-testid="artifact-preview-truncated"
        >
          {t("inspector.previewTruncatedNote")}
        </p>
      )}
      <pre
        data-testid="artifact-preview-content"
        className="max-h-[60vh] overflow-auto whitespace-pre-wrap break-words rounded-lg bg-[var(--muted)] p-3 font-mono text-ui-sm text-[var(--foreground)]"
      >
        {content}
      </pre>
    </Dialog>
  );
}
