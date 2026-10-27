/**
 * shadcn/ui 风格 Tag：小标签，两种形态——
 *  Tag 普通展示标签；CheckableTag 可点选的开关标签（模型输入/输出类型选择用）。
 */
import type { ReactNode } from "react";
import { cn } from "./cn";

/** Tag 的 props：children 标签内容；className 追加类名。 */
export interface TagProps {
  children: ReactNode;
  className?: string;
}

/** 渲染一个普通展示标签（纯文本小标记）。 */
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

/** CheckableTag 的 props：checked 选中态；locked 锁定（不可点）；onChange 点选回调；
 *  children 内容；aria-label 无障碍名。 */
export interface CheckableTagProps {
  checked: boolean;
  /** 锁定项（如「文本」恒选）不渲染成按钮 */
  locked?: boolean;
  onChange?: (checked: boolean) => void;
  children: ReactNode;
  "aria-label"?: string;
}

/** 渲染可点选标签：锁定或无回调时退化为纯展示；否则渲染成带 aria-pressed 的按钮。 */
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
