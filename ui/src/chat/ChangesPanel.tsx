/**
 * 变更面板（26 号票，FR-11-02 逐字 + §10.7）：大尺寸 Dialog——左列文件
 * 列表（状态徽标 M/A/D/U/untracked + ±N 增删统计，点选切换）、右列聚合
 * diff；顶部 recent log 折叠条；手动刷新按钮；非仓库空态「当前项目不是
 * git 仓库」。Inspector「变更」tab 打开；产物面板（28 号票）经
 * openChangesPanel(path?) 联动打开并定位文件 diff。
 *
 * 数据源 = §3.3 #37 git_status / #38 git_diff / #39 git_recent_log（只读）；
 * 超长文件折叠 = diff 行区限高滚动（虚拟化不做，票面边界）。
 */

import { GitBranch, RefreshCw } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useChangesStore } from "@/chat/changes-store";
import { DiffView } from "@/chat/DiffView";
import { t, useT } from "@/i18n/core";
import { gitDiff, gitRecentLog, gitStatus } from "@/ipc/client";
import type { DiffPatch, GitStatusFile, GitStatusResult } from "@/ipc/types";

/** 最近提交条数（折叠条展示面；§3.3 #39 缺省 20 由后端兜底）。 */
const LOG_LIMIT = 10;

/**
 * 文件列表行 = 状态（#37）与增删统计（#38）按路径合并（纯函数，测试面）：
 * status 恒有（status 列表是全集）；diff 统计可能缺失（如截断/二进制之外的
 * 边缘——二进制仍有 hunks=[] 但统计为 0，此处合并只做并集兜底）。
 */
export function mergeChanges(
  files: GitStatusFile[],
  patches: DiffPatch[],
): { path: string; status: string; addLines: number | null; delLines: number | null }[] {
  const statsByPath = new Map(patches.map((p) => [p.path, p]));
  return files.map((f) => {
    const patch = statsByPath.get(f.path);
    return {
      path: f.path,
      status: f.status,
      addLines: patch ? patch.addLines : null,
      delLines: patch ? patch.delLines : null,
    };
  });
}

/** 状态徽标配色与符号（M/A/D/U/untracked）。 */
function statusBadge(status: string): { label: string; className: string } {
  switch (status) {
    case "M":
      return { label: "M", className: "bg-amber-500/15 text-amber-600 dark:text-amber-400" };
    case "A":
      return { label: "A", className: "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400" };
    case "D":
      return { label: "D", className: "bg-red-500/15 text-red-600 dark:text-red-400" };
    case "U":
      return { label: "U", className: "bg-red-500/20 text-red-700 dark:text-red-300" };
    default:
      return { label: "??", className: "bg-[var(--muted)] text-[var(--muted-foreground)]" };
  }
}

/** 面板入口：store 开合态驱动（全局单例）。 */
export function ChangesPanel() {
  const open = useChangesStore((s) => s.open);
  const sessionId = useChangesStore((s) => s.sessionId);
  const focusPath = useChangesStore((s) => s.focusPath);
  const closePanel = useChangesStore((s) => s.closePanel);
  if (!open) return null;
  return <ChangesPanelBody sessionId={sessionId} focusPath={focusPath} onClose={closePanel} />;
}

/** 面板本体：数据拉取 + 双列布局。 */
function ChangesPanelBody({
  sessionId,
  focusPath,
  onClose,
}: {
  sessionId: string;
  focusPath: string | null;
  onClose: () => void;
}) {
  const t = useT();
  const [status, setStatus] = useState<GitStatusResult | null>(null);
  const [patches, setPatches] = useState<DiffPatch[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // 刷新代数：手动刷新与 sessionId 切换共用同一拉取入口
  const [reloadTick, setReloadTick] = useState(0);

  // 打开 / 切换会话 / 手动刷新 → 三命令并发拉取（只读面，失败降级空态）。
  // 本体只在 open=true 时挂载（ChangesPanel 条件渲染），无需再判开合。
  // reloadTick 刻意作为触发依赖（点「刷新」即重拉），effect 内不读值。
  // biome-ignore lint/correctness/useExhaustiveDependencies: reloadTick 为手动刷新触发依赖（点击即重拉），effect 内不读值（QuotaPanel 周期刷新同款先例）
  useEffect(() => {
    let cancelled = false;
    setLoadError(null);
    void Promise.all([
      gitStatus({ sessionId }),
      gitDiff({ sessionId }),
      gitRecentLog({ sessionId, limit: LOG_LIMIT }),
    ])
      .then(([statusResult, diffResult]) => {
        if (cancelled) return;
        setStatus(statusResult);
        setPatches(diffResult.patches);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        console.error("[changes] git 状态拉取失败", err);
        setLoadError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId, reloadTick]);

  // 合并文件列表（状态 + 增删统计），路径排序稳定展示
  const rows = useMemo(
    () => mergeChanges(status?.files ?? [], patches).sort((a, b) => a.path.localeCompare(b.path)),
    [status, patches],
  );
  const patchByPath = useMemo(() => new Map(patches.map((p) => [p.path, p])), [patches]);

  // 定位与缺省选中：focusPath 命中列表 → 选中它；否则缺省第一个文件
  useEffect(() => {
    if (rows.length === 0) {
      setSelected(null);
      return;
    }
    if (focusPath !== null && rows.some((r) => r.path === focusPath)) {
      setSelected(focusPath);
      return;
    }
    setSelected((prev) =>
      prev !== null && rows.some((r) => r.path === prev) ? prev : rows[0].path,
    );
  }, [rows, focusPath]);

  const selectedPatch = selected !== null ? (patchByPath.get(selected) ?? null) : null;

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: 复用 Dialog 遮罩点击关闭的模态标准交互，键盘路径由 Esc 承担
    <div
      className="settings-dialog-mask"
      role="presentation"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className="settings-dialog"
        style={{ maxWidth: 920, width: "92vw" }}
        role="dialog"
        aria-modal="true"
        aria-label={t("inspector.changesPanelAria")}
        data-testid="changes-panel"
      >
        {/* 标题行：标题 + 刷新 + 关闭 */}
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-ui-lg font-semibold text-[var(--foreground)]">
            {t("inspector.changesTitle")}
          </h2>
          <div className="flex items-center gap-2">
            <button
              type="button"
              data-testid="changes-refresh"
              aria-label={t("inspector.refresh")}
              title={t("inspector.refreshChangesTitle")}
              onClick={() => setReloadTick((t) => t + 1)}
              className="flex items-center gap-1 rounded px-2 py-1 text-ui-sm text-[var(--muted-foreground)] hover:bg-[var(--accent)] hover:text-[var(--foreground)]"
            >
              <RefreshCw size={13} aria-hidden />
              {t("inspector.refresh")}
            </button>
            <button
              type="button"
              aria-label={t("inspector.close")}
              onClick={onClose}
              className="rounded px-2 py-1 text-ui-sm text-[var(--muted-foreground)] hover:bg-[var(--accent)] hover:text-[var(--foreground)]"
            >
              {t("inspector.close")}
            </button>
          </div>
        </div>

        {loadError !== null ? (
          <p
            data-testid="changes-error"
            className="py-8 text-center text-ui-sm text-[var(--destructive)]"
          >
            {t("inspector.changesLoadFailed", { error: loadError })}
          </p>
        ) : status === null ? (
          <p
            data-testid="changes-loading"
            className="py-8 text-center text-ui-sm text-[var(--muted-foreground)]"
          >
            {t("inspector.changesLoading")}
          </p>
        ) : !status.isRepo ? (
          // 非 git 仓库空态（FR-11-02 验收第三条：不报错）
          <p
            data-testid="changes-empty"
            className="py-10 text-center text-ui-base text-[var(--muted-foreground)]"
          >
            {t("inspector.changesNotRepo")}
          </p>
        ) : rows.length === 0 ? (
          <p
            data-testid="changes-clean"
            className="py-10 text-center text-ui-base text-[var(--muted-foreground)]"
          >
            {t("inspector.changesClean")}
          </p>
        ) : (
          <div className="flex flex-col gap-3">
            {/* recent log 折叠条（#39） */}
            <RecentLogSection sessionId={sessionId} reloadTick={reloadTick} />
            {/* 双列：左文件列表（定宽）右聚合 diff */}
            <div className="flex min-h-0 gap-3" style={{ height: "58vh" }}>
              <ul
                data-testid="changes-file-list"
                className="w-64 shrink-0 space-y-0.5 overflow-y-auto rounded border border-[var(--border)] p-1"
              >
                {rows.map((row) => {
                  const badge = statusBadge(row.status);
                  const active = selected === row.path;
                  const hasStat =
                    row.addLines !== null && (row.addLines > 0 || (row.delLines ?? 0) > 0);
                  return (
                    <li key={row.path}>
                      <button
                        type="button"
                        data-testid={`changes-file-${row.path}`}
                        onClick={() => setSelected(row.path)}
                        className={`flex w-full items-center gap-1.5 rounded px-1.5 py-1 text-left font-mono text-ui-sm ${
                          active
                            ? "bg-[var(--accent)] text-[var(--foreground)]"
                            : "text-[var(--muted-foreground)] hover:bg-[var(--accent)]/60"
                        }`}
                      >
                        <span
                          className={`shrink-0 rounded px-1 text-[10px] font-medium ${badge.className}`}
                        >
                          {badge.label}
                        </span>
                        <span className="min-w-0 flex-1 truncate" title={row.path}>
                          {row.path}
                        </span>
                        {hasStat && (
                          <span className="shrink-0 text-[10px] tabular-nums">
                            <span className="text-emerald-600 dark:text-emerald-400">
                              +{row.addLines}
                            </span>{" "}
                            <span className="text-red-600 dark:text-red-400">
                              −{row.delLines ?? 0}
                            </span>
                          </span>
                        )}
                      </button>
                    </li>
                  );
                })}
              </ul>
              <div className="min-w-0 flex-1 overflow-y-auto">
                {selectedPatch !== null ? (
                  <DiffView
                    diff={{ path: selectedPatch.path, hunks: selectedPatch.hunks }}
                    header={`${selectedPatch.binary ? t("inspector.binaryMark") : ""}+${selectedPatch.addLines} −${selectedPatch.delLines}${
                      selectedPatch.truncated ? t("inspector.truncatedMark") : ""
                    }`}
                    maxH="max-h-full"
                    testId="changes-diff"
                  />
                ) : selected !== null ? (
                  <p className="py-8 text-center text-ui-sm text-[var(--muted-foreground)]">
                    {t("inspector.noTextDiffFile")}
                  </p>
                ) : null}
              </div>
            </div>
            {/* user info（FR-11-01）：当前 git 身份的弱化展示行 */}
            <p
              className="flex items-center gap-1.5 text-ui-sm text-[var(--muted-foreground)]"
              data-testid="changes-user"
            >
              <GitBranch size={12} aria-hidden />
              {status.user?.name ?? t("inspector.noGitUser")}
              {status.user?.email ? ` <${status.user.email}>` : ""}
            </p>
          </div>
        )}
      </div>
    </div>
  );
}

/** recent log 折叠条（#39）：原生 details 折叠 + 提交列表（sha 前 7 位 /
 *  summary / 相对时间）。随面板刷新联动重拉。 */
function RecentLogSection({ sessionId, reloadTick }: { sessionId: string; reloadTick: number }) {
  const t = useT();
  const [commits, setCommits] = useState<{ sha: string; summary: string; time: number }[]>([]);
  // reloadTick 刻意作为触发依赖（面板手动刷新联动重拉），effect 内不读值。
  // biome-ignore lint/correctness/useExhaustiveDependencies: reloadTick 为手动刷新触发依赖，effect 内不读值
  useEffect(() => {
    let cancelled = false;
    void gitRecentLog({ sessionId, limit: LOG_LIMIT })
      .then((result) => {
        if (!cancelled) setCommits(result.commits);
      })
      .catch(() => {
        if (!cancelled) setCommits([]);
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId, reloadTick]);
  return (
    <details data-testid="changes-log" className="rounded border border-[var(--border)] px-2 py-1">
      <summary className="cursor-pointer text-ui-sm text-[var(--muted-foreground)] select-none">
        {t("inspector.recentCommits", { count: commits.length })}
      </summary>
      <ul className="space-y-0.5 py-1">
        {commits.map((c) => (
          <li key={c.sha} className="flex items-baseline gap-2 text-ui-sm">
            <code className="shrink-0 text-[var(--muted-foreground)]">{c.sha.slice(0, 7)}</code>
            <span className="min-w-0 flex-1 truncate text-[var(--foreground)]">{c.summary}</span>
            <time className="shrink-0 text-[10px] text-[var(--muted-foreground)]">
              {formatTime(c.time)}
            </time>
          </li>
        ))}
        {commits.length === 0 && (
          <li className="py-1 text-ui-sm text-[var(--muted-foreground)]">
            {t("inspector.noCommits")}
          </li>
        )}
      </ul>
    </details>
  );
}

/** Unix 秒 → 本地相对时间（折叠条弱化展示；不做完整日期格式化）。 */
function formatTime(unixSecs: number): string {
  const diffMs = Date.now() - unixSecs * 1000;
  const minutes = Math.floor(diffMs / 60000);
  if (minutes < 1) return t("workspace.justNow");
  if (minutes < 60) return t("workspace.minutesAgo", { n: minutes });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return t("workspace.hoursAgo", { n: hours });
  const days = Math.floor(hours / 24);
  if (days < 30) return t("workspace.daysAgo", { n: days });
  return new Date(unixSecs * 1000).toLocaleDateString();
}
