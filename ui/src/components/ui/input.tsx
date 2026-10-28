/**
 * shadcn/ui 风格 Input / Textarea（手写移植的单行/多行输入框）。
 * 控件基色走语义令牌（CSS 变量）。聚焦规范（A4，ZCode 同款）：单行
 * 输入只提亮边框不加环，多行文本域才保留 2px 环——安静的聚焦态。
 */
import type { InputHTMLAttributes, TextareaHTMLAttributes } from "react";
import { cn } from "./cn";

/** 输入控件公共类：边框/占位符/聚焦边框/禁用态（Input 与 Textarea 共用）。 */
const FIELD_CLASS =
  "w-full rounded-md border border-[var(--input)] bg-transparent px-3 text-body text-[var(--foreground)] " +
  "placeholder:text-[var(--muted-foreground)] transition-colors " +
  "focus:border-[var(--cw-focus)] focus:outline-none " +
  "disabled:cursor-not-allowed disabled:opacity-50";

/** 单行输入框：透传全部原生 input 属性，className 追加在公共类之后。 */
export function Input({ className, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  return <input className={cn(FIELD_CLASS, "h-9", className)} {...rest} />;
}

/** 多行文本域：透传全部原生 textarea 属性；多行内容面积大，保留聚焦环。 */
export function Textarea({ className, ...rest }: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={cn(
        FIELD_CLASS,
        "py-2 leading-relaxed focus:ring-2 focus:ring-[var(--ring)]/30",
        className,
      )}
      {...rest}
    />
  );
}
