/**
 * shadcn/ui 风格 Input / Textarea（手写移植的单行/多行输入框）。
 * 控件基色走语义令牌（CSS 变量），聚焦态用 2px 环。
 */
import type { InputHTMLAttributes, TextareaHTMLAttributes } from "react";
import { cn } from "./cn";

/** 输入控件公共类：边框/占位符/聚焦环/禁用态（Input 与 Textarea 共用）。 */
const FIELD_CLASS =
  "w-full rounded-md border border-[var(--input)] bg-transparent px-3 text-[15px] text-[var(--foreground)] " +
  "placeholder:text-[var(--muted-foreground)] transition-colors " +
  "focus:border-[var(--cw-focus)] focus:outline-none focus:ring-2 focus:ring-[var(--ring)]/40 " +
  "disabled:cursor-not-allowed disabled:opacity-50";

/** 单行输入框：透传全部原生 input 属性，className 追加在公共类之后。 */
export function Input({ className, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  return <input className={cn(FIELD_CLASS, "h-9", className)} {...rest} />;
}

/** 多行文本域：透传全部原生 textarea 属性。 */
export function Textarea({ className, ...rest }: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return <textarea className={cn(FIELD_CLASS, "py-2 leading-relaxed", className)} {...rest} />;
}
