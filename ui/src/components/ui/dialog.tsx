/**
 * shadcn/ui 风格 Dialog：设置页离开三选 / 供应商编辑共用的居中模态。
 * 手写实现（无 radix）：遮罩点击与 Esc 都触发 onClose，不拦截按钮区冒泡。
 */
import { type ReactNode, useEffect } from "react";
import { Button } from "./button";

export interface DialogActionItem {
  label: string;
  variant?: "default" | "outline" | "ghost" | "danger";
  onClick: () => void;
}

export interface DialogProps {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
  /** 底部动作区（不传则只有关闭行为） */
  actions?: DialogActionItem[];
  width?: number;
}

export function Dialog({ open, title, onClose, children, actions, width = 440 }: DialogProps) {
  useEffect(() => {
    if (!open) return;
    const onKeydown = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.isComposing) {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKeydown, true);
    return () => window.removeEventListener("keydown", onKeydown, true);
  }, [open, onClose]);

  if (!open) return null;
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: 遮罩点击关闭是模态标准交互，键盘路径由 Esc 承担
    <div
      className="settings-dialog-mask"
      role="presentation"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className="settings-dialog"
        style={{ maxWidth: width }}
        role="dialog"
        aria-modal="true"
        aria-label={title}
      >
        <h2 className="mb-2 text-base font-semibold text-[var(--foreground)]">{title}</h2>
        <div className="text-sm leading-relaxed text-[var(--muted-foreground)]">{children}</div>
        {actions && actions.length > 0 && (
          <div className="mt-6 flex flex-wrap items-center justify-end gap-2">
            {actions.map((action) => (
              <Button
                key={action.label}
                variant={action.variant ?? "outline"}
                onClick={action.onClick}
              >
                {action.label}
              </Button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
