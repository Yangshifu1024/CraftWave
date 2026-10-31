/**
 * shadcn/ui 风格 Dialog：居中模态弹框（设置页离开三选 / 供应商编辑共用）。
 * 手写实现（不依赖 radix 组件库）：遮罩点击与 Esc 都触发 onClose，
 * 不拦截按钮区冒泡。模态（modal）指打开时挡住背后页面、同一时刻只操作弹框。
 */

import { X } from "lucide-react";
import { type ReactNode, useEffect } from "react";
import { t } from "@/i18n/core";
import { Button } from "./button";

/** 底部动作区的一个按钮描述：label 文案；variant 按钮变体（默认 outline）；
 *  disabled 禁用态；onClick 点击回调。 */
export interface DialogActionItem {
  label: string;
  variant?: "default" | "outline" | "ghost" | "danger";
  disabled?: boolean;
  onClick: () => void;
}

/** Dialog 的 props：open 是否显示；title 标题；onClose 关闭回调（Esc/遮罩）；
 *  children 正文；actions 底部按钮组；width 最大宽度（默认 440）。 */
export interface DialogProps {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
  /** 底部动作区（不传则只有关闭行为） */
  actions?: DialogActionItem[];
  width?: number;
}

/** 渲染居中模态弹框：标题 + 正文 + 可选按钮组。 */
export function Dialog({ open, title, onClose, children, actions, width = 440 }: DialogProps) {
  // 打开期间监听 Esc 关闭；isComposing 排除输入法组合键中的 Esc
  // （用户正在用输入法打字时按 Esc 是取消候选，不应关掉弹框）
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
        <div className="mb-5 flex items-center justify-between">
          <h2 className="text-ui-lg font-semibold text-[var(--foreground)]">{title}</h2>
          <button
            type="button"
            aria-label={t("inspector.close")}
            onClick={onClose}
            className="flex size-6 items-center justify-center rounded text-[var(--muted-foreground)] hover:bg-[var(--accent)] hover:text-[var(--foreground)]"
          >
            <X size={14} aria-hidden />
          </button>
        </div>
        <div className="text-ui-base leading-relaxed text-[var(--muted-foreground)]">
          {children}
        </div>
        {actions && actions.length > 0 && (
          <div className="mt-6 flex flex-wrap items-center justify-end gap-2">
            {actions.map((action) => (
              <Button
                key={action.label}
                variant={action.variant ?? "outline"}
                disabled={action.disabled}
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
