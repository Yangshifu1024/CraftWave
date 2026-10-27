/**
 * shadcn/ui 风格 Switch 开关（手写移植：button[role=switch]，不依赖 radix 库）。
 */
import { cn } from "./cn";

/** Switch 的 props：checked 开/关；onCheckedChange 切换回调；
 *  aria-label 无障碍名；disabled 禁用。 */
export interface SwitchProps {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  "aria-label"?: string;
  disabled?: boolean;
}

/** 渲染开关：用 button + role="switch" + aria-checked 保证读屏与键盘可达。 */
export function Switch({ checked, onCheckedChange, disabled, ...aria }: SwitchProps) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      className={cn(
        "inline-flex h-5 w-9 shrink-0 items-center rounded-full border border-transparent p-0.5",
        "transition-colors focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--ring)]",
        "disabled:cursor-not-allowed disabled:opacity-50",
        checked ? "bg-[var(--primary)]" : "bg-[var(--input)]",
      )}
      onClick={() => onCheckedChange(!checked)}
      {...aria}
    >
      <span
        className={cn(
          "block h-4 w-4 rounded-full bg-[var(--background)] shadow transition-transform",
          checked ? "translate-x-4" : "translate-x-0",
        )}
      />
    </button>
  );
}
