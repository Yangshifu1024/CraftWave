/**
 * shadcn/ui 风格 Button（手写移植，不依赖 radix 组件库）。
 * variant：default 主按钮 / outline 描边 / ghost 幽灵（无边无底，悬停显底）/
 * danger 危险色描边（删除等破坏性操作）。
 */
import type { ButtonHTMLAttributes, Ref } from "react";
import { cn } from "./cn";

/** 视觉变体：default 实底主色 / outline 描边 / ghost 幽灵 / danger 危险描边。 */
export type ButtonVariant = "default" | "outline" | "ghost" | "danger";
/** 尺寸档：default 常规 / sm 小号。 */
export type ButtonSize = "default" | "sm";

/** 各变体的配色类。 */
const VARIANT_CLASS: Record<ButtonVariant, string> = {
  default: "bg-[var(--primary)] text-[var(--primary-foreground)] hover:opacity-90",
  outline:
    "border border-[var(--input)] bg-transparent text-[var(--foreground)] hover:bg-[var(--accent)]",
  ghost: "bg-transparent text-[var(--foreground)] hover:bg-[var(--accent)]",
  danger:
    "border border-[var(--destructive)] bg-transparent text-[var(--destructive)] hover:bg-[var(--destructive)]/10",
};

/** 各尺寸档的高宽类。 */
const SIZE_CLASS: Record<ButtonSize, string> = {
  default: "h-9 px-4",
  sm: "h-8 px-3 text-[13px]",
};

/** Button 的 props：在原生 button 属性之上加 variant / size 两档。 */
export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** React 19：ref 作为普通 prop 透传 */
  ref?: Ref<HTMLButtonElement>;
}

/** 渲染按钮。默认 type="button"（防止在表单里被当成提交按钮）。 */
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
