import { CircleAlert, Copy, Settings } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";

/** 保留完整错误可复制；模型或认证错误可直接进入供应商设置。 */
export function ChatErrorBanner({
  message,
  onOpenSettings,
}: {
  message: string;
  onOpenSettings?: () => void;
}) {
  const [copyState, setCopyState] = useState("");
  return (
    <div
      role="alert"
      className="mt-2 flex w-full items-start gap-2 rounded-xl border border-[var(--destructive)]/30 bg-[var(--destructive)]/5 px-3 py-2.5"
    >
      <CircleAlert size={16} aria-hidden className="mt-0.5 shrink-0 text-[var(--destructive)]" />
      <div className="min-w-0 flex-1 text-ui-sm">
        <p className="font-medium text-[var(--destructive)]">运行出错</p>
        <p className="mt-1 whitespace-pre-wrap break-words text-[var(--muted-foreground)]">
          {message}
        </p>
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <Button
            variant="ghost"
            size="sm"
            onClick={async () => {
              try {
                await navigator.clipboard.writeText(message);
                setCopyState("已复制");
              } catch {
                setCopyState("复制失败，请选择错误文本复制");
              }
            }}
          >
            <Copy size={14} aria-hidden />
            复制错误
          </Button>
          {onOpenSettings &&
            /模型|供应商|api.?key|认证|鉴权|401|403|E_MODEL|E_PROVIDER/i.test(message) && (
              <Button variant="outline" size="sm" onClick={onOpenSettings}>
                <Settings size={14} aria-hidden />
                打开模型设置
              </Button>
            )}
          <span role="status" className="text-[var(--muted-foreground)]">
            {copyState}
          </span>
        </div>
      </div>
    </div>
  );
}
