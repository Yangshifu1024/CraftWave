/**
 * shadcn/ui 风格 Alert：设置页顶部错误/警告提示条。
 * shadcn/ui 是社区流行的组件风格规范，本项目按其视觉口径手写实现。
 */
import type { ReactNode } from "react";
import { cn } from "./cn";

/** Alert 的 props：title 提示文案；variant 视觉变体；className 追加类名。 */
export interface AlertProps {
  title: ReactNode;
  /** warning 警告 / error 错误 / info 中性信息（默认） */
  variant?: "warning" | "error" | "info";
  className?: string;
}

/** 各变体的配色类（边框 + 背景 + 文字，全部走语义色变量）。 */
const VARIANT_CLASS = {
  warning: "border-[var(--warning)]/40 bg-[var(--warning)]/10 text-[var(--warning)]",
  error: "border-[var(--destructive)]/40 bg-[var(--destructive)]/10 text-[var(--destructive)]",
  info: "border-[var(--border)] bg-[var(--muted)] text-[var(--muted-foreground)]",
} as const;

/** 渲染一条带语义配色的提示条（role="alert" 让读屏软件立即播报）。 */
export function Alert({ title, variant = "info", className }: AlertProps) {
  return (
    <div
      role="alert"
      className={cn(
        "rounded-lg border px-4 py-2.5 text-ui-caption leading-relaxed",
        VARIANT_CLASS[variant],
        className,
      )}
    >
      {title}
    </div>
  );
}
