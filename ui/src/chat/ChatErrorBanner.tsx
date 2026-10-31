import { ChevronRight, CircleAlert, Copy, Settings } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { useT } from "@/i18n/core";

/** 保留完整错误可复制；模型或认证错误可直接进入供应商设置。 */
export function ChatErrorBanner({
  message,
  onOpenSettings,
}: {
  message: string;
  onOpenSettings?: () => void;
}) {
  const t = useT();
  const [copyState, setCopyState] = useState("");
  return (
    <div
      role="alert"
      className="flex w-full flex-wrap items-start gap-2 rounded-xl border border-[var(--border)] bg-[var(--muted)]/60 px-3 py-2 backdrop-blur-md"
    >
      <details className="group/error min-w-0 flex-1 basis-48 text-ui-base">
        <summary
          aria-label={t("workspace.errorDetails")}
          className="flex min-h-7 cursor-pointer items-center gap-2 rounded-md"
        >
          <CircleAlert size={16} aria-hidden className="shrink-0 text-[var(--destructive)]" />
          <span className="min-w-0 truncate font-medium" title={message}>
            {message.split(/\r?\n/, 1)[0] || t("workspace.runErrorShort")}
          </span>
          <ChevronRight
            size={14}
            aria-hidden
            className="shrink-0 text-[var(--muted-foreground)] transition-transform group-open/error:rotate-90"
          />
        </summary>
        <pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap [overflow-wrap:anywhere] font-mono text-ui-sm text-[var(--muted-foreground)]">
          {message}
        </pre>
      </details>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="ghost"
          size="sm"
          onClick={async () => {
            try {
              await navigator.clipboard.writeText(message);
              setCopyState(t("toolcards.copied"));
            } catch {
              setCopyState(t("toolcards.copyFailed"));
            }
          }}
        >
          <Copy size={14} aria-hidden />
          {t("toolcards.copyError")}
        </Button>
        {onOpenSettings &&
          /模型|供应商|api.?key|认证|鉴权|401|403|E_MODEL|E_PROVIDER/i.test(message) && (
            <Button variant="outline" size="sm" onClick={onOpenSettings}>
              <Settings size={14} aria-hidden />
              {t("workspace.openModelSettings")}
            </Button>
          )}
        <span role="status" className="text-ui-sm text-[var(--muted-foreground)]">
          {copyState}
        </span>
      </div>
    </div>
  );
}
