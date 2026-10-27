/**
 * shadcn/ui 风格 Tag：小标签（普通 / 可切换 CheckableTag 两种形态）。
 * 可切换形态用于模型输入/输出类型的选择。
 */
import type { ReactNode } from "react";
import { cn } from "./cn";

export interface TagProps {
  children: ReactNode;
  className?: string;
}

export function Tag({ children, className }: TagProps) {
  return (
    <span
      className={cn(
        "inline-flex max-w-full shrink-0 items-center gap-1 rounded-md border border-[var(--border)] px-1.5 py-px text-xs leading-5 text-[var(--muted-foreground)]",
        className,
      )}
    >
      {children}
    </span>
  );
}

export interface CheckableTagProps {
  checked: boolean;
  /** 锁定项（如「文本」恒选）不渲染成按钮 */
  locked?: boolean;
  onChange?: (checked: boolean) => void;
  children: ReactNode;
  "aria-label"?: string;
}

export function CheckableTag({ checked, locked, onChange, children, ...aria }: CheckableTagProps) {
  const base = cn(
    "inline-flex max-w-full shrink-0 items-center gap-1 rounded-md border px-1.5 py-px text-xs leading-5 transition-colors",
    checked
      ? "border-[var(--cw-focus)]/50 bg-[var(--accent)] text-[var(--accent-foreground)]"
      : "border-[var(--border)] text-[var(--muted-foreground)]",
  );
  if (locked || !onChange) {
    return <span className={base}>{children}</span>;
  }
  return (
    <button
      type="button"
      className={base}
      aria-pressed={checked}
      onClick={() => onChange(!checked)}
      {...aria}
    >
      {children}
    </button>
  );
}
