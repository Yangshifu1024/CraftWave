/**
 * shadcn/ui 风格 Badge：供应商 API 方言 / 默认模型标记等小标签。
 */
import type { ReactNode } from "react";
import { cn } from "./cn";

export interface BadgeProps {
  children: ReactNode;
  variant?: "secondary" | "outline";
  className?: string;
}

export function Badge({ children, variant = "secondary", className }: BadgeProps) {
  return (
    <span
      className={cn(
        "inline-flex max-w-full shrink-0 items-center rounded-full px-2 py-0.5 text-xs leading-5",
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
