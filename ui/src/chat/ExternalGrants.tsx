/**
 * Inspector「授权」页（撤销 UI 的会话面）：当前会话已授权
 * 的外部目录列表（SessionMeta.authorizedExternalDirs，会话 roots 快照）
 * + 行内「移除授权」按钮（revoke_external_dir，移除后下次访问重新询问）。
 *
 * 数据面说明：授权列表随会话索引（list_sessions）下发，本组件从 sessions
 * store 读当前会话的 meta——移除成功后触发 store.refresh() 拉回权威列表
 * （后端 SessionMeta 是唯一事实源，前端不本地维护副本）。授权仅本会话
 * 有效：临时会话/无授权时给空态说明。
 */

import { FolderOpen, X } from "lucide-react";
import { useSessionScope } from "@/chat/session-scope";
import { useT } from "@/i18n/core";
import { revokeExternalDir } from "@/ipc/client";
import { useSessionsStore } from "@/stores/sessions";

/** 「授权」页面板本体。 */
export function ExternalGrantsPanel() {
  const t = useT();
  const sessionId = useSessionScope();
  const sessions = useSessionsStore((s) => s.sessions);
  const refresh = useSessionsStore((s) => s.refresh);
  const meta = sessions.find((m) => m.id === sessionId);
  const dirs = meta?.authorizedExternalDirs ?? [];

  /** 移除授权（行内按钮）：后端移除成功后刷新会话索引回填权威列表；
   *  失败静默保留（下次刷新会看到真实状态——授权移除不是高危操作）。 */
  const revoke = (dir: string) => {
    revokeExternalDir({ sessionId, dir })
      .then(() => refresh())
      .catch((err: unknown) => {
        console.error("[grants] 授权移除失败", err);
      });
  };

  return (
    <div
      className="flex min-h-0 flex-1 flex-col gap-2 overflow-auto px-4 py-3"
      data-testid="external-grants"
    >
      <p className="text-ui-sm text-[var(--muted-foreground)]">{t("inspector.grantsIntro")}</p>
      {dirs.length === 0 ? (
        <p
          className="settings-section-card px-5 py-8 text-center text-ui-base text-[var(--muted-foreground)]"
          data-testid="external-grants-empty"
        >
          {t("inspector.grantsEmpty")}
        </p>
      ) : (
        <div className="settings-section-card py-1">
          {dirs.map((dir) => (
            <div
              key={dir}
              data-testid="external-grant-row"
              className="flex items-center gap-3 border-t border-[var(--border)] px-4 py-3 first:border-t-0"
            >
              <FolderOpen size={14} aria-hidden className="shrink-0 text-[var(--cw-dim)]" />
              <code
                className="min-w-0 flex-1 truncate font-mono text-ui-sm text-[var(--foreground)]"
                title={dir}
              >
                {dir}
              </code>
              <button
                type="button"
                data-testid={`external-grant-revoke-${dir}`}
                title={t("inspector.grantsRemoveTitle")}
                onClick={() => revoke(dir)}
                className="shrink-0 rounded-full border border-[var(--border)] p-1 text-[var(--muted-foreground)] hover:border-[var(--destructive)] hover:text-[var(--destructive)]"
              >
                <X size={12} aria-hidden />
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
