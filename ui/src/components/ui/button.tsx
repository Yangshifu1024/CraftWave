/**
 * shadcn/ui 风格 Button（手写移植，无 radix 依赖）。
 * variant：default 主按钮 / outline 描边 / ghost 幽灵 / danger 危险色描边。
 */
import type { ButtonHTMLAttributes, Ref } from "react";
import { cn } from "./cn";

export type ButtonVariant = "default" | "outline" | "ghost" | "danger";
export type ButtonSize = "default" | "sm";

const VARIANT_CLASS: Record<ButtonVariant, string> = {
  default: "bg-[var(--primary)] text-[var(--primary-foreground)] hover:opacity-90",
  outline:
    "border border-[var(--input)] bg-transparent text-[var(--foreground)] hover:bg-[var(--accent)]",
  ghost: "bg-transparent text-[var(--foreground)] hover:bg-[var(--accent)]",
  danger:
    "border border-[var(--destructive)] bg-transparent text-[var(--destructive)] hover:bg-[var(--destructive)]/10",
};

const SIZE_CLASS: Record<ButtonSize, string> = {
  default: "h-9 px-4",
  sm: "h-8 px-3 text-[13px]",
};

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** React 19：ref 作为普通 prop 透传 */
  ref?: Ref<HTMLButtonElement>;
}

export function Button({
  variant = "default",
  size = "default",
  className,
  type = "button",
  ref,
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      ref={ref}
      className={cn(
        "inline-flex shrink-0 items-center justify-center gap-1.5 rounded-md text-sm font-medium",
        "transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--ring)]",
        "disabled:pointer-events-none disabled:opacity-50",
        VARIANT_CLASS[variant],
        SIZE_CLASS[size],
        className,
      )}
      {...rest}
    />
  );
}
