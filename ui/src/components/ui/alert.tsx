/**
 * shadcn/ui 风格 Alert：设置页顶部错误/警告条。
 */
import type { ReactNode } from "react";
import { cn } from "./cn";

export interface AlertProps {
  title: ReactNode;
  variant?: "warning" | "error" | "info";
  className?: string;
}

const VARIANT_CLASS = {
  warning: "border-[var(--warning)]/40 bg-[var(--warning)]/10 text-[var(--warning)]",
  error: "border-[var(--destructive)]/40 bg-[var(--destructive)]/10 text-[var(--destructive)]",
  info: "border-[var(--border)] bg-[var(--muted)] text-[var(--muted-foreground)]",
} as const;

export function Alert({ title, variant = "info", className }: AlertProps) {
  return (
    <div
      role="alert"
      className={cn(
        "rounded-lg border px-4 py-2.5 text-[13px] leading-relaxed",
        VARIANT_CLASS[variant],
        className,
      )}
    >
      {title}
    </div>
  );
}
