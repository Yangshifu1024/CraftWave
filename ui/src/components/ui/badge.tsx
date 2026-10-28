/**
 * shadcn/ui 风格 Badge：小标签（供应商 API 格式 / 模型能力等场景的辅助标记）。
 */
import type { ReactNode } from "react";
import { cn } from "./cn";

/** Badge 的 props：children 标签内容；variant 视觉变体；className 追加类名。 */
export interface BadgeProps {
  children: ReactNode;
  /** secondary 实底（默认）/ outline 描边 */
  variant?: "secondary" | "outline";
  className?: string;
}

/** 渲染一个圆角胶囊小标签。 */
export function Badge({ children, variant = "secondary", className }: BadgeProps) {
  return (
    <span
      className={cn(
        "inline-flex max-w-full shrink-0 items-center rounded-full px-2 py-0.5 text-ui-sm leading-5",
        variant === "secondary"
          ? "bg-[var(--accent)] text-[var(--accent-foreground)]"
          : "border border-[var(--border)] text-[var(--muted-foreground)]",
        className,
      )}
    >
      {children}
    </span>
  );
}
