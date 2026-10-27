/**
 * shadcn/ui 风格 Input / Textarea（手写移植）。
 * 控件基色走语义令牌，聚焦态用 2px 环。
 */
import type { InputHTMLAttributes, TextareaHTMLAttributes } from "react";
import { cn } from "./cn";

const FIELD_CLASS =
  "w-full rounded-md border border-[var(--input)] bg-transparent px-3 text-sm text-[var(--foreground)] " +
  "placeholder:text-[var(--muted-foreground)] transition-colors " +
  "focus:border-[var(--cw-focus)] focus:outline-none focus:ring-2 focus:ring-[var(--ring)]/40 " +
  "disabled:cursor-not-allowed disabled:opacity-50";

export function Input({ className, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  return <input className={cn(FIELD_CLASS, "h-9", className)} {...rest} />;
}

export function Textarea({ className, ...rest }: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return <textarea className={cn(FIELD_CLASS, "py-2 leading-relaxed", className)} {...rest} />;
}
