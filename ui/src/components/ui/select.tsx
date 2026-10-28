/**
 * shadcn/ui 风格 Select：原生 select + 右侧下箭头（appearance-none 去掉系统样式）。
 * 用原生下拉保证键盘可达与输入法兼容；选项渲染交由调用方（children 传 <option>）。
 */
import type { SelectHTMLAttributes } from "react";
import { ChevronDownIcon } from "@/settings/icons";
import { cn } from "./cn";

/** 在原生 select 属性之上加 width 宽度档。 */
export interface SelectProps extends SelectHTMLAttributes<HTMLSelectElement> {
  /** 选项宽度档（照抄 CodeWave 三档：窄 180 / 中 240 / 宽 360），默认自适应 */
  width?: "narrow" | "mid" | "wide";
}

/** 各宽度档的像素类。 */
const WIDTH_CLASS = {
  narrow: "w-[180px] max-w-full",
  mid: "w-[240px] max-w-full",
  wide: "w-[360px] max-w-full",
} as const;

/** 渲染下拉框：外层 div 负责定宽与箭头定位，箭头 pointer-events-none 不挡点击。 */
export function Select({ width, className, children, ...rest }: SelectProps) {
  return (
    <div className={cn("relative inline-flex", width && WIDTH_CLASS[width], className)}>
      <select
        className={
          "h-9 w-full appearance-none rounded-md border border-[var(--input)] bg-transparent pl-3 pr-8 text-body " +
          "text-[var(--foreground)] transition-colors focus:border-[var(--cw-focus)] focus:outline-none " +
          "focus:ring-2 focus:ring-[var(--ring)]/40 disabled:cursor-not-allowed disabled:opacity-50"
        }
        {...rest}
      >
        {children}
      </select>
      <ChevronDownIcon className="pointer-events-none absolute top-1/2 right-2.5 h-4 w-4 -translate-y-1/2 text-[var(--muted-foreground)]" />
    </div>
  );
}
